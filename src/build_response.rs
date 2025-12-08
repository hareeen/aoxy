use futures::TryStreamExt;
use reqwest::StatusCode;

use crate::models::{CachedResponse, HandlerResult};
use crate::utils::{add_headers_to_response_builder, error_response, hashmap_to_headers};

pub fn build_cached_response(cached: CachedResponse) -> HandlerResult {
    let body_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cached.body).map_err(
            |e| {
                tracing::warn!("Failed to decode cached body: {e}");
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
        .body(axum::body::Body::from(body_bytes))
        .map_err(|e| {
            tracing::error!("Failed to build cached response: {e}");
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

    let mapped_stream = stream.map_err(std::io::Error::other);

    builder
        .body(axum::body::Body::from_stream(mapped_stream))
        .map_err(|e| {
            tracing::error!("Failed to build streaming response: {e}");
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
    let builder = axum::http::Response::builder().status(status);
    let builder = add_headers_to_response_builder(builder, headers);
    let builder = builder.header("X-Cache", "MISS");

    builder.body(axum::body::Body::from(body)).map_err(|e| {
        tracing::error!("Failed to build response: {e}");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Response build error: {e}"),
        )
    })
}
