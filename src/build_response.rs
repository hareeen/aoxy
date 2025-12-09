use futures::TryStreamExt;
use reqwest::StatusCode;

use crate::models::{CachedResponse, HandlerResult};
use crate::utils::{add_headers_to_response_builder, error_response, hashmap_to_headers};

pub fn build_cached_response(cached: CachedResponse) -> HandlerResult {
    let body_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cached.body).map_err(
            |e| {
                tracing::error!(
                    cached_status = cached.status,
                    body_len = cached.body.len(),
                    error = %e,
                    "Failed to decode cached body from base64"
                );
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Cache decode error".to_string(),
                )
            },
        )?;

    let mut builder = axum::http::Response::builder().status(cached.status);
    let cached_header_map = hashmap_to_headers(&cached.headers);
    builder = add_headers_to_response_builder(builder, &cached_header_map);
    builder = builder.header("X-Cache", "HIT");

    builder
        .body(axum::body::Body::from(body_bytes.clone()))
        .map_err(|e| {
            tracing::error!(
                cached_status = cached.status,
                body_size = body_bytes.len(),
                error = %e,
                "Failed to build cached response"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Response build error: {e}"),
            )
        })
}

pub fn build_streaming_response<S>(
    status: StatusCode,
    headers: &axum::http::HeaderMap,
    stream: S,
) -> HandlerResult
where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let builder = axum::http::Response::builder().status(status);
    let builder = add_headers_to_response_builder(builder, headers);
    let builder = builder.header("X-Cache", "STREAM");

    let status_for_logging = status;
    let mapped_stream = stream.map_err(move |e| {
        tracing::error!(
            status = %status_for_logging,
            error = %e,
            "Error during response streaming"
        );
        std::io::Error::other(e)
    });

    builder
        .body(axum::body::Body::from_stream(mapped_stream))
        .map_err(|e| {
            tracing::error!(
                status = %status,
                error = %e,
                "Failed to build streaming response"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Response build error: {e}"),
            )
        })
}

pub fn build_buffered_response(
    status: StatusCode,
    headers: &axum::http::HeaderMap,
    body: Vec<u8>,
) -> HandlerResult {
    let body_size = body.len();
    let builder = axum::http::Response::builder().status(status);
    let builder = add_headers_to_response_builder(builder, headers);
    let builder = builder.header("X-Cache", "MISS");

    builder.body(axum::body::Body::from(body)).map_err(|e| {
        tracing::error!(
            status = %status,
            body_size = body_size,
            error = %e,
            "Failed to build buffered response"
        );
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Response build error: {e}"),
        )
    })
}
