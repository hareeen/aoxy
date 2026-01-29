use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use crate::Args;
use crate::handler::proxy_handler;
use crate::models::{AppState, Limiter};
use crate::utils::parse_default_headers;

fn create_rate_limiter(args: &Args) -> Limiter {
    let quota = governor::Quota::per_second(
        NonZeroU32::new(args.rate_limit_per_sec).expect("rate_limit_per_sec must be positive"),
    )
    .allow_burst(
        NonZeroU32::new(args.rate_limit_burst).expect("rate_limit_burst must be positive"),
    );

    governor::RateLimiter::direct(quota)
}

#[cfg(feature = "caching")]
fn create_redis_pool(url: Option<&str>) -> Option<deadpool_redis::Pool> {
    url.map(|url| {
        deadpool_redis::Config::from_url(url)
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .expect("Failed to create Redis pool")
    })
}

fn create_http_client(args: &Args) -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.upstream_timeout_secs))
        .redirect(reqwest::redirect::Policy::none());

    if let Some(proxy_url) = &args.proxy_url {
        tracing::info!("Configuring proxy: {proxy_url}");
        let mut proxy = reqwest::Proxy::all(proxy_url)?;

        if let (Some(user), Some(pass)) = (&args.proxy_username, &args.proxy_password) {
            tracing::info!("Using proxy authentication");
            proxy = proxy.basic_auth(user, pass);
        }

        builder = builder.proxy(proxy);
    }

    Ok(builder.build()?)
}

pub async fn start(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        bind_addr = %args.bind_addr,
        external_api_base = %args.external_api_base,
        rate_limit_per_sec = args.rate_limit_per_sec,
        rate_limit_burst = args.rate_limit_burst,
        cache_ttl_secs = args.cache_ttl_secs,
        skip_idempotency_check = args.skip_idempotency_check,
        hard_timeout_secs = args.hard_timeout_secs,
        upstream_timeout_secs = args.upstream_timeout_secs,
        max_elapsed_time_secs = args.max_elapsed_time_secs,
        initial_backoff_ms = args.initial_backoff_ms,
        max_body_size = args.max_body_size,
        proxy_configured = args.proxy_url.is_some(),
        "Starting proxy with configuration"
    );

    let default_headers = parse_default_headers(args.default_headers.as_ref())?;
    if !default_headers.is_empty() {
        tracing::info!(
            count = default_headers.len(),
            header_names = ?default_headers.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            "Loaded default headers"
        );
    }

    #[cfg(feature = "caching")]
    let redis_pool = create_redis_pool(args.redis_url.as_deref());

    #[cfg(feature = "caching")]
    if let Some(ref pool) = redis_pool {
        tracing::info!(
            redis_url = %args.redis_url.as_deref().unwrap_or(""),
            pool_max_size = ?pool.status().max_size,
            "Redis caching enabled"
        );
    } else {
        tracing::warn!("Redis caching disabled (no REDIS_URL provided)");
    }

    #[cfg(not(feature = "caching"))]
    tracing::warn!("Redis caching disabled (compiled without caching feature)");

    let state = Arc::new(AppState {
        limiter: create_rate_limiter(args),
        #[cfg(feature = "caching")]
        redis_pool,
        http_client: create_http_client(args)?,
        default_headers,
        cfg: args.clone(),
    });

    let app = axum::Router::new()
        .fallback(axum::routing::any(proxy_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind_addr).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!(
        bind_addr = %local_addr,
        "Server listening"
    );

    axum::serve(listener, app).await?;

    tracing::info!("Server shutdown complete");

    Ok(())
}
