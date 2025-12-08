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

fn create_redis_pool(url: &str) -> deadpool_redis::Pool {
    deadpool_redis::Config::from_url(url)
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .expect("Failed to create Redis pool")
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
    let default_headers = parse_default_headers(args.default_headers.as_ref())?;
    if !default_headers.is_empty() {
        tracing::info!("Loaded {} default headers", default_headers.len());
    }

    let state = Arc::new(AppState {
        limiter: create_rate_limiter(args),
        redis_pool: create_redis_pool(&args.redis_url),
        http_client: create_http_client(args)?,
        default_headers,
        cfg: args.clone(),
    });

    tracing::info!(
        "Starting proxy: {} -> {}",
        args.bind_addr,
        args.external_api_base
    );

    let app = axum::Router::new()
        .fallback(axum::routing::any(proxy_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&args.bind_addr).await?;

    axum::serve(listener, app).await?;

    Ok(())
}
