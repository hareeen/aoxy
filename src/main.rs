use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::any;
use backoff::ExponentialBackoff;
use backoff::future::retry;
use bytes::Bytes;
use clap::Parser;
use governor::RateLimiter;
use governor::clock::{DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use reqwest::header::CONTENT_TYPE;
use tokio::time::timeout;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

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

    /// Proxy URL (supports http, https, socks5). ENV: PROXY_URL
    /// Examples: http://proxy.example.com:8080, socks5://127.0.0.1:1080
    #[arg(long, env = "PROXY_URL")]
    proxy_url: Option<String>,

    /// Proxy username for authentication. ENV: PROXY_USERNAME
    #[arg(long, env = "PROXY_USERNAME")]
    proxy_username: Option<String>,

    /// Proxy password for authentication. ENV: PROXY_PASSWORD
    #[arg(long, env = "PROXY_PASSWORD")]
    proxy_password: Option<String>,
}

/// Shared application state
struct AppState {
    limiter: Limiter,
    redis_pool: deadpool_redis::Pool,
    cfg: Args,
    http_client: reqwest::Client,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

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

    // Redis pool
    let redis_cfg = deadpool_redis::Config::from_url(&args.redis_url);
    let redis_pool = redis_cfg
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap();

    // HTTP client with timeout and optional proxy
    let mut client_builder =
        reqwest::Client::builder().timeout(Duration::from_secs(args.upstream_timeout_secs));

    // Configure proxy if provided
    if let Some(proxy_url) = &args.proxy_url {
        info!("Configuring proxy: {}", proxy_url);
        let mut proxy =
            reqwest::Proxy::all(proxy_url).map_err(|e| format!("Invalid proxy URL: {}", e))?;

        // Add proxy authentication if credentials are provided
        if let (Some(username), Some(password)) = (&args.proxy_username, &args.proxy_password) {
            info!("Using proxy authentication with username: {}", username);
            proxy = proxy.basic_auth(username, password);
        }

        client_builder = client_builder.proxy(proxy);
    }

    let http_client = client_builder.build()?;

    // Create a shared application state
    let state = Arc::new(AppState {
        limiter,
        redis_pool,
        cfg: args.clone(),
        http_client,
    });

    info!(
        "Starting proxy on {} -> {}",
        args.bind_addr, args.external_api_base
    );

    let app = Router::new().fallback(any(proxy_handler)).with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum UpstreamError {
    #[error("Upstream request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("Upstream returned an error status: {0}")]
    BadStatus(String),
}

/// Core proxy handler – captures all routes and forwards to the external API.
async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    debug!("Received request: {} {}", method, uri);

    let upstream_url = format!(
        "{}{}",
        state.cfg.external_api_base.trim_end_matches('/'),
        uri
    );

    // Compose cache key (method + full path & query)
    let cache_key = format!("{}:{}", method, upstream_url);

    // Try Redis cache first
    if let Ok(mut conn) = state.redis_pool.get().await {
        let cached: Option<Vec<u8>> = redis::cmd("GET")
            .arg(&cache_key)
            .query_async(&mut conn)
            .await
            .unwrap_or(None);
        if let Some(cached) = cached {
            debug!("Cache hit for key: {}", cache_key);
            if !cached.is_empty() {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(cached))
                    .unwrap());
            }
        }
    }

    // Retry with back‑off
    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(state.cfg.max_elapsed_time_secs)),
        initial_interval: Duration::from_millis(state.cfg.initial_backoff_ms),
        multiplier: 2.0,
        ..ExponentialBackoff::default()
    };

    let response_bytes: Vec<u8> = timeout(
        Duration::from_secs(state.cfg.hard_timeout_secs),
        retry(backoff, || {
            debug!("Forwarding request to upstream: {} {}", method, uri);

            let body_clone = body.clone();
            let state = state.clone();
            let method = method.clone();
            let upstream_url = upstream_url.clone();
            let headers = headers.clone();

            async move {
                // Enforce rate‑limit (permits per second)
                state.limiter.until_ready().await;

                // Build the request
                let mut req = state.http_client.request(method.clone(), &upstream_url);

                // Copy headers (except HOST)
                for (name, value) in headers.iter() {
                    if name != "host" {
                        req = req.header(name, value);
                    }
                }

                // Send the request with the body
                match req.body(body_clone.to_vec()).send().await {
                    Ok(res) if res.status().is_success() => match res.bytes().await {
                        Ok(body) => Ok(body.to_vec()),
                        Err(e) => {
                            error!("Error reading body: {e}");
                            Err(backoff::Error::transient(UpstreamError::RequestFailed(e)))
                        }
                    },
                    Ok(res) => {
                        let status = res.status();
                        let err = UpstreamError::BadStatus(format!(
                            "Upstream returned {}: {}",
                            status,
                            status.canonical_reason().unwrap_or("Unknown reason")
                        ));

                        if status.is_client_error() && status.as_u16() != 429 {
                            // Handle client errors (4xx) except rate limit (429)
                            Err(backoff::Error::permanent(err))
                        } else {
                            // For server errors (5xx) or rate limit (429), retry
                            Err(backoff::Error::transient(err))
                        }
                    }
                    Err(e) => Err(backoff::Error::transient(UpstreamError::RequestFailed(e))),
                }
            }
        }),
    )
    .await
    .map_err(|e| {
        error!("Request to upstream reached hard timeout: {e}");
        (StatusCode::GATEWAY_TIMEOUT, format!("Request timeout: {e}"))
    })?
    .map_err(|e| {
        error!("Upstream call failed after retries: {e}");
        (StatusCode::BAD_GATEWAY, format!("Upstream error: {e}"))
    })?;

    debug!(
        "Received response from upstream, request: {} {}, size: {} bytes",
        method,
        uri,
        response_bytes.len()
    );

    // Cache the successful response, if idempotent
    if method.is_idempotent()
        && let Ok(mut conn) = state.redis_pool.get().await
    {
        let _: () = redis::pipe()
            .cmd("SET")
            .arg(&cache_key)
            .arg(&response_bytes)
            .cmd("EXPIRE")
            .arg(&cache_key)
            .arg(state.cfg.cache_ttl_secs)
            .query_async(&mut conn)
            .await
            .unwrap_or(());
    }

    // Return to client (assume JSON; adjust if needed)
    debug!("Returning response to client, request: {} {}", method, uri);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(response_bytes))
        .unwrap())
}
