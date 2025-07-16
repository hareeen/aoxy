use std::num::NonZeroU32;
use std::time::Duration;

use actix_web::{App, Error, HttpRequest, HttpResponse, HttpServer, http::header, web};
use backoff::{ExponentialBackoff, future::retry};
use clap::Parser;
use governor::clock::QuantaInstant;
use governor::middleware::NoOpMiddleware;
use governor::state::NotKeyed;
use governor::{RateLimiter, clock::DefaultClock, state::InMemoryState};
use log::{debug, error, info};
use tokio::time::timeout;

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware<QuantaInstant>>;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    /// Where the proxy listens (addr:port). ENV: BIND_ADDR
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    bind_addr: String,

    /// Base URL of the external API we proxy to (e.g. https://api.example.com). ENV: EXTERNAL_API_BASE
    #[arg(long, env = "EXTERNAL_API_BASE")]
    external_api_base: String,

    /// Max outbound requests per second. ENV: RATE_LIMIT_PER_SEC
    #[arg(long, env = "RATE_LIMIT_PER_SEC", default_value_t = 10)]
    rate_limit_per_sec: u32,

    /// Max burst size for rate limiting. ENV: RATE_LIMIT_BURST
    #[arg(long, env = "RATE_LIMIT_BURST", default_value_t = 1)]
    rate_limit_burst: u32,

    /// Redis connection string (e.g. redis://127.0.0.1/0). ENV: REDIS_URL
    #[arg(long, env = "REDIS_URL", default_value = "redis://127.0.0.1/")]
    redis_url: String,

    /// Cache TTL in seconds. ENV: CACHE_TTL_SECS
    #[arg(long, env = "CACHE_TTL_SECS", default_value_t = 600)]
    cache_ttl_secs: usize,

    /// Hard Timeout for retries in seconds. ENV: HARD_TIMEOUT_SECS
    #[arg(long, env = "HARD_TIMEOUT_SECS", default_value_t = 60)]
    hard_timeout_secs: u64,

    /// Timeout for upstream requests in seconds. ENV: UPSTREAM_TIMEOUT_SECS
    #[arg(long, env = "UPSTREAM_TIMEOUT_SECS", default_value_t = 30)]
    upstream_timeout_secs: u64,

    /// Maximum elapsed time for retries in seconds. ENV: MAX_ELAPSED_TIME_SECS
    #[arg(long, env = "MAX_ELAPSED_TIME_SECS", default_value_t = 30)]
    max_elapsed_time_secs: u64,

    /// Initial back‑off in milliseconds. ENV: INITIAL_BACKOFF_MS
    #[arg(long, env = "INITIAL_BACKOFF_MS", default_value_t = 200)]
    initial_backoff_ms: u64,
}

/// Shared application state
struct AppState {
    limiter: Limiter,
    redis_pool: deadpool_redis::Pool,
    cfg: Args,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    let args = Args::parse();

    // Governor rate‑limiter (one global bucket)
    let quota = governor::Quota::per_second(
        NonZeroU32::new(args.rate_limit_per_sec)
            .expect("RATE_LIMIT_PER_SEC must be a positive integer"),
    )
    .allow_burst(
        NonZeroU32::new(args.rate_limit_burst)
            .expect("RATE_LIMIT_BURST must be a positive integer"),
    );
    let limiter: Limiter = RateLimiter::direct(quota);

    // Redis pool (async‑std compatible)
    let redis_cfg = deadpool_redis::Config::from_url(&args.redis_url);
    let redis_pool = redis_cfg
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap();

    // Create a shared application state
    let state = web::Data::new(AppState {
        limiter,
        redis_pool,
        cfg: args.clone(),
    });

    info!(
        "Starting proxy on {} -> {}",
        args.bind_addr, args.external_api_base
    );

    let bind_addr = args.bind_addr.clone();
    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .default_service(web::route().to(proxy_handler))
    })
    .bind(&bind_addr)?
    .run()
    .await?;

    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum UpstreamError {
    #[error("Upstream request failed: {0}")]
    RequestFailed(#[from] awc::error::SendRequestError),

    #[error("Failed to read response body: {0}")]
    BodyReadError(#[from] awc::error::PayloadError),

    #[error("Upstream returned an error status: {0}")]
    BadStatus(String),
}

/// Core proxy handler – captures all routes and forwards to the external API.
async fn proxy_handler(
    req: HttpRequest,
    body: web::Bytes,
    data: web::Data<AppState>,
) -> Result<HttpResponse, Error> {
    debug!("Received request: {} {}", req.method(), req.uri());

    let upstream_url = format!(
        "{}{}",
        data.cfg.external_api_base.trim_end_matches('/'),
        req.uri()
    );

    // Compose cache key (method + full path & query)
    let cache_key = format!("{}:{}", req.method(), upstream_url);

    // Try Redis cache first
    if let Ok(mut conn) = data.redis_pool.get().await {
        let cached: Option<Vec<u8>> = redis::cmd("GET")
            .arg(&cache_key)
            .query_async(&mut conn)
            .await
            .unwrap_or(None);
        if let Some(cached) = cached {
            debug!("Cache hit for key: {}", cache_key);
            if !cached.is_empty() {
                return Ok(HttpResponse::Ok()
                    .insert_header((header::CONTENT_TYPE, "application/json"))
                    .body(cached));
            }
        }
    }

    // Retry with back‑off
    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(data.cfg.max_elapsed_time_secs)),
        initial_interval: Duration::from_millis(data.cfg.initial_backoff_ms),
        multiplier: 2.0,
        ..ExponentialBackoff::default()
    };

    let response_bytes: Vec<u8> = timeout(
        Duration::from_secs(data.cfg.hard_timeout_secs),
        retry(backoff, || {
            // Create the upstream request
            debug!(
                "Forwarding request to upstream: {} {}",
                req.method(),
                req.uri()
            );
            let client = awc::Client::builder()
                .timeout(Duration::from_secs(data.cfg.upstream_timeout_secs)) // Set a timeout for the request
                .finish();

            let mut req_to_send = client.request(req.method().clone(), upstream_url.clone());
            for (h, v) in req.headers().iter() {
                if h != header::HOST {
                    req_to_send = req_to_send.insert_header((h.clone(), v.clone()));
                }
            }

            let body_clone = body.clone();
            let data = data.clone();
            async move {
                // Enforce rate‑limit (permits per second)
                data.limiter.until_ready().await;

                // Send the request with the body
                match req_to_send.send_body(body_clone).await {
                    Ok(mut res) if res.status().is_success() => {
                        match res.body().limit(16 << 20).await {
                            Ok(body) => Ok(body.to_vec()),
                            Err(e) => {
                                error!("Error reading body: {e}");
                                Err(backoff::Error::transient(UpstreamError::BodyReadError(e)))
                            }
                        }
                    }
                    Ok(res) => {
                        let err = UpstreamError::BadStatus(format!(
                            "Upstream returned {}: {}",
                            res.status(),
                            res.status().canonical_reason().unwrap_or("Unknown reason")
                        ));

                        return Err(
                            if res.status().is_client_error() && res.status().as_u16() != 429 {
                                // Handle client errors (4xx) except rate limit (429)
                                backoff::Error::permanent(err)
                            } else {
                                // For server errors (5xx) or rate limit (429), retry
                                backoff::Error::transient(err)
                            },
                        );
                    }
                    Err(e) => Err(backoff::Error::transient(UpstreamError::RequestFailed(e))),
                }
            }
        }),
    )
    .await
    .map_err(|e| {
        error!("Request to upstream reached hard timeout: {e}");
        actix_web::error::ErrorGatewayTimeout(e)
    })?
    .map_err(|e| {
        error!("Upstream call failed after retries: {e}");
        actix_web::error::ErrorBadGateway(e)
    })?;

    debug!(
        "Received response from upstream, request: {} {}, size: {} bytes",
        req.method(),
        req.uri(),
        response_bytes.len()
    );

    // Cache the successful response, if idempotent
    if req.method().is_idempotent() {
        if let Ok(mut conn) = data.redis_pool.get().await {
            let _: () = redis::pipe()
                .cmd("SET")
                .arg(&cache_key)
                .arg(&response_bytes)
                .cmd("EXPIRE")
                .arg(&cache_key)
                .arg(data.cfg.cache_ttl_secs)
                .query_async(&mut conn)
                .await
                .unwrap_or(());
        }
    }

    // Return to client (assume JSON; adjust if needed)
    debug!(
        "Returning response to client, request: {} {}",
        req.method(),
        req.uri()
    );
    Ok(HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .body(response_bytes))
}
