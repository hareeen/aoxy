use std::collections::HashSet;
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
use crate::utils::{error_response, filter_headers, generate_cache_key, headers_to_hashmap};

async fn make_upstream_request(
    client: &reqwest::Client,
    limiter: &Limiter,
    method: reqwest::Method,
    url: &str,
    default_headers: &axum::http::HeaderMap,
    incoming_headers: &axum::http::HeaderMap,
    body: Vec<u8>,
) -> Result<reqwest::Response, reqwest::Error> {
    // Wait for rate limit permit
    limiter.until_ready().await;

    // Start with filtered incoming headers
    let mut header_map = filter_headers(incoming_headers);

    // Add default headers if not already present
    // This ensures that incoming headers take precedence, and properly handling multi-value headers
    let mut header_name_allowlist = HashSet::new();
    for name in default_headers.keys() {
        if !header_map.contains_key(name) {
            header_name_allowlist.insert(name.clone());
        }
    }
    for (name, value) in default_headers.iter() {
        if header_name_allowlist.contains(name) {
            header_map.append(name.clone(), value.clone());
        }
    }

    client
        .request(method, url)
        .headers(header_map)
        .body(body)
        .send()
        .await
}

fn should_retry_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
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
    let upstream_url = format!(
        "{}{}",
        state.cfg.external_api_base.trim_end_matches('/'),
        uri
    );
    let cache_key = generate_cache_key(&method, &upstream_url, &body);

    tracing::debug!(
        method = %method,
        uri = %uri,
        upstream_url = %upstream_url,
        cache_key = %cache_key,
        body_size = body.len(),
        "Received request"
    );

    // Try cache for idempotent methods (or all methods if skip_idempotency_check is enabled)
    let is_cacheable = state.cfg.skip_idempotency_check || method.is_idempotent();

    #[cfg(feature = "redis")]
    if is_cacheable
        && let Some(cached) = try_get_cached_response(state.redis_pool.as_ref(), &cache_key).await
    {
        tracing::info!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            cached_status = cached.status,
            "Cache hit"
        );
        return build_cached_response(cached);
    }

    #[cfg(not(feature = "redis"))]
    if is_cacheable && let Some(cached) = try_get_cached_response(&cache_key).await {
        tracing::info!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            cached_status = cached.status,
            "Cache hit"
        );
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
    let retry_start = std::time::Instant::now();
    let upstream_res = tokio::time::timeout(
        Duration::from_secs(state.cfg.hard_timeout_secs),
        retry(backoff, || async {
            tracing::debug!(
                method = %method,
                uri = %uri,
                upstream_url = %upstream_url,
                "Forwarding request to upstream"
            );

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
                    tracing::warn!(
                        method = %method,
                        uri = %uri,
                        status = %status,
                        "Upstream returned retryable status, will retry"
                    );
                    Err(backoff::Error::transient(UpstreamError::BadStatus(
                        format!(
                            "{status}: {}",
                            status.canonical_reason().unwrap_or("Unknown")
                        ),
                    )))
                }
                Ok(res) => Ok(res),
                Err(e) => {
                    tracing::warn!(
                        method = %method,
                        uri = %uri,
                        error = %e,
                        "Upstream request failed, will retry"
                    );
                    Err(backoff::Error::transient(UpstreamError::RequestFailed(e)))
                }
            }
        }),
    )
    .await
    .map_err(|e| {
        tracing::error!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            hard_timeout_secs = state.cfg.hard_timeout_secs,
            error = %e,
            "Hard timeout reached"
        );
        error_response(StatusCode::GATEWAY_TIMEOUT, format!("Timeout: {e}"))
    })?
    .map_err(|e| {
        tracing::error!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            elapsed_ms = retry_start.elapsed().as_millis(),
            error = %e,
            "Upstream failed after retries"
        );
        error_response(StatusCode::BAD_GATEWAY, format!("Upstream error: {e}"))
    })?;

    let retry_elapsed = retry_start.elapsed();

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
        tracing::info!(
            method = %method,
            uri = %uri,
            status = %status,
            content_length = ?content_length,
            elapsed_ms = retry_elapsed.as_millis(),
            "Streaming large response"
        );
        return build_streaming_response(status, &response_headers, upstream_res.bytes_stream());
    }

    // Buffer the response
    let response_body = upstream_res.bytes().await.map_err(|e| {
        tracing::error!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            error = %e,
            "Error reading response body"
        );
        error_response(
            StatusCode::BAD_GATEWAY,
            format!("Error reading upstream: {e}"),
        )
    })?;

    tracing::info!(
        method = %method,
        uri = %uri,
        status = %status,
        response_size = response_body.len(),
        elapsed_ms = retry_elapsed.as_millis(),
        "Received upstream response"
    );

    // Cache if appropriate
    let should_cache = is_cacheable
        && (status.is_success() || status.is_redirection())
        && (state.cfg.max_body_size == 0
            || response_body.len() <= state.cfg.max_body_size as usize);

    if should_cache {
        let cached = CachedResponse {
            status: status.as_u16(),
            headers: headers_to_hashmap(&filter_headers(&response_headers)),
            body: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &response_body,
            ),
        };

        #[cfg(feature = "redis")]
        cache_response(
            state.redis_pool.as_ref(),
            &cache_key,
            &cached,
            state.cfg.cache_ttl_secs,
        )
        .await;

        #[cfg(not(feature = "redis"))]
        cache_response(&cache_key, &cached, state.cfg.cache_ttl_secs).await;
    }

    if should_cache {
        tracing::debug!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            status = %status,
            response_size = response_body.len(),
            ttl_secs = state.cfg.cache_ttl_secs,
            "Caching response"
        );
    } else {
        tracing::debug!(
            method = %method,
            uri = %uri,
            cache_key = %cache_key,
            status = %status,
            is_cacheable = is_cacheable,
            response_size = response_body.len(),
            max_body_size = state.cfg.max_body_size,
            "Response not cached"
        );
    }

    build_buffered_response(status, &response_headers, response_body.to_vec())
}
