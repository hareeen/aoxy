use std::collections::HashMap;

use governor::RateLimiter;
use governor::clock::{DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use serde::{Deserialize, Serialize};

// Utility Types

pub type Limiter =
    RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware<QuantaInstant>>;

pub type HandlerResult = Result<axum::response::Response, (axum::http::StatusCode, String)>;

// Cli Arguments

#[derive(clap::Parser, Debug, Clone)]
#[command(author, version, about)]
pub struct Args {
    /// Address and port to listen on. ENV: BIND_ADDR
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    pub bind_addr: String,

    /// Base URL of the external API to proxy to. ENV: EXTERNAL_API_BASE
    #[arg(long, env = "EXTERNAL_API_BASE")]
    pub external_api_base: String,

    /// Max outbound requests per second. ENV: RATE_LIMIT_PER_SEC
    #[arg(long, env = "RATE_LIMIT_PER_SEC", default_value_t = 10)]
    pub rate_limit_per_sec: u32,

    /// Burst capacity for rate limiting. ENV: RATE_LIMIT_BURST
    #[arg(long, env = "RATE_LIMIT_BURST", default_value_t = 1)]
    pub rate_limit_burst: u32,

    /// Redis connection string (optional, enables caching when provided). ENV: REDIS_URL
    #[arg(long, env = "REDIS_URL")]
    pub redis_url: Option<String>,

    /// Cache TTL in seconds. ENV: CACHE_TTL_SECS
    #[arg(long, env = "CACHE_TTL_SECS", default_value_t = 600)]
    pub cache_ttl_secs: usize,

    /// Hard timeout for entire retry cycle in seconds. ENV: HARD_TIMEOUT_SECS
    #[arg(long, env = "HARD_TIMEOUT_SECS", default_value_t = 60)]
    pub hard_timeout_secs: u64,

    /// Timeout for each upstream request in seconds. ENV: UPSTREAM_TIMEOUT_SECS
    #[arg(long, env = "UPSTREAM_TIMEOUT_SECS", default_value_t = 30)]
    pub upstream_timeout_secs: u64,

    /// Maximum elapsed time for retries in seconds. ENV: MAX_ELAPSED_TIME_SECS
    #[arg(long, env = "MAX_ELAPSED_TIME_SECS", default_value_t = 30)]
    pub max_elapsed_time_secs: u64,

    /// Initial backoff interval in milliseconds. ENV: INITIAL_BACKOFF_MS
    #[arg(long, env = "INITIAL_BACKOFF_MS", default_value_t = 200)]
    pub initial_backoff_ms: u64,

    /// Max response size to buffer/cache (bytes). Larger responses are streamed.
    /// Set to 0 to disable streaming. ENV: MAX_BODY_SIZE
    #[arg(long, env = "MAX_BODY_SIZE", default_value_t = 10 * 1024 * 1024)]
    pub max_body_size: u64,

    /// Proxy URL (http, https, socks5). ENV: PROXY_URL
    #[arg(long, env = "PROXY_URL")]
    pub proxy_url: Option<String>,

    /// Proxy username. ENV: PROXY_USERNAME
    #[arg(long, env = "PROXY_USERNAME")]
    pub proxy_username: Option<String>,

    /// Proxy password. ENV: PROXY_PASSWORD
    #[arg(long, env = "PROXY_PASSWORD")]
    pub proxy_password: Option<String>,

    /// Default headers as JSON. ENV: DEFAULT_HEADERS
    /// Example: '{"Authorization":"Bearer token"}'
    #[arg(long, env = "DEFAULT_HEADERS")]
    pub default_headers: Option<String>,
}

// Application State

pub struct AppState {
    pub limiter: Limiter,
    #[cfg(feature = "redis")]
    pub redis_pool: Option<deadpool_redis::Pool>,
    pub cfg: Args,
    pub http_client: reqwest::Client,
    pub default_headers: axum::http::HeaderMap,
}

// Cache Types

#[derive(Serialize, Deserialize, Debug)]
pub struct CachedResponse {
    pub status: u16,
    pub headers: HashMap<String, Vec<String>>,
    pub body: String, // base64 encoded
}

// Errors

#[derive(thiserror::Error, Debug)]
pub enum UpstreamError {
    #[error("Upstream request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("Upstream returned error status: {0}")]
    BadStatus(String),
}
