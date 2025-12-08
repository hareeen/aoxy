use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use backoff::ExponentialBackoff;
use backoff::future::retry;
use reqwest::StatusCode;

use crate::build_response::{
    build_buffered_response, build_cached_response, build_streaming_response,
};
use crate::cache::{cache_response, try_get_cached_response};
use crate::models::{AppState, CachedResponse, HandlerResult, Limiter, UpstreamError};
use crate::utils::{error_response, filter_headers, generate_cache_key, is_hop_by_hop_header};

async fn make_upstream_request(
    client: &reqwest::Client,
    limiter: &Limiter,
    method: reqwest::Method,
    url: &str,
    default_headers: &HashMap<String, String>,
    incoming_headers: &axum::http::HeaderMap,
    body: Vec<u8>,
) -> Result<reqwest::Response, reqwest::Error> {
    // Wait for rate limit permit
    limiter.until_ready().await;

    // Build request with headers
    let mut req = client.request(method, url);

    // Add default headers first
    for (name, value) in default_headers {
        req = req.header(name, value);
    }

    // Add incoming headers (except hop-by-hop), overriding defaults
    for (name, value) in incoming_headers.iter() {
        if !is_hop_by_hop_header(name.as_str()) {
            req = req.header(name, value);
        }
    }

    req.body(body).send().await
}

fn should_retry_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
}

// =============================================================================
// Main Handler
// =============================================================================

pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    method: reqwest::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: bytes::Bytes,
) -> HandlerResult {
    tracing::debug!("Received request: {method} {uri}");

    let upstream_url = format!(
        "{}{}",
        state.cfg.external_api_base.trim_end_matches('/'),
        uri
    );
    let cache_key = generate_cache_key(&method, &upstream_url);

    // Try cache for idempotent methods
    if method.is_idempotent()
        && let Some(cached) = try_get_cached_response(&state.redis_pool, &cache_key).await
    {
        tracing::debug!("Cache hit for {cache_key}");
        return build_cached_response(cached);
    }

    // Configure retry backoff
    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(state.cfg.max_elapsed_time_secs)),
        initial_interval: Duration::from_millis(state.cfg.initial_backoff_ms),
        multiplier: 2.0,
        ..Default::default()
    };

    // Make request with retry logic
    let body_vec = body.to_vec();
    let upstream_res = tokio::time::timeout(
        Duration::from_secs(state.cfg.hard_timeout_secs),
        retry(backoff, || async {
            tracing::debug!("Forwarding to upstream: {method} {uri}");

            match make_upstream_request(
                &state.http_client,
                &state.limiter,
                method.clone(),
                &upstream_url,
                &state.default_headers,
                &headers,
                body_vec.clone(),
            )
            .await
            {
                Ok(res) if should_retry_status(res.status()) => {
                    let status = res.status();
                    Err(backoff::Error::transient(UpstreamError::BadStatus(
                        format!(
                            "{status}: {}",
                            status.canonical_reason().unwrap_or("Unknown")
                        ),
                    )))
                }
                Ok(res) => Ok(res),
                Err(e) => Err(backoff::Error::transient(UpstreamError::RequestFailed(e))),
            }
        }),
    )
    .await
    .map_err(|e| {
        tracing::error!("Hard timeout reached: {e}");
        error_response(StatusCode::GATEWAY_TIMEOUT, format!("Timeout: {e}"))
    })?
    .map_err(|e| {
        tracing::error!("Upstream failed after retries: {e}");
        error_response(StatusCode::BAD_GATEWAY, format!("Upstream error: {e}"))
    })?;

    let status = upstream_res.status();
    let response_headers = upstream_res.headers().clone();

    // Check if we should stream (large response)
    let content_length: Option<u64> = upstream_res
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let should_stream = state.cfg.max_body_size > 0
        && content_length.is_some_and(|len| len > state.cfg.max_body_size);

    if should_stream {
        tracing::debug!("Streaming large response ({content_length:?} bytes) for {method} {uri}");
        return build_streaming_response(status, &response_headers, upstream_res.bytes_stream());
    }

    // Buffer the response
    let response_body = upstream_res.bytes().await.map_err(|e| {
        tracing::error!("Error reading response body: {e}");
        error_response(
            StatusCode::BAD_GATEWAY,
            format!("Error reading upstream: {e}"),
        )
    })?;

    tracing::debug!(
        "Received response: {method} {uri} -> {status}, {} bytes",
        response_body.len()
    );

    // Cache if appropriate
    let should_cache = method.is_idempotent()
        && (status.is_success() || status.is_redirection())
        && (state.cfg.max_body_size == 0
            || response_body.len() <= state.cfg.max_body_size as usize);

    if should_cache {
        let cached = CachedResponse {
            status: status.as_u16(),
            headers: filter_headers(&response_headers),
            body: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &response_body,
            ),
        };
        cache_response(
            &state.redis_pool,
            &cache_key,
            &cached,
            state.cfg.cache_ttl_secs,
        )
        .await;
    }

    build_buffered_response(status, &response_headers, response_body.to_vec())
}
