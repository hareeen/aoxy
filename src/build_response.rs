use futures::TryStreamExt;
use reqwest::StatusCode;

#[cfg(feature = "caching")]
use crate::models::CachedResponse;
use crate::models::HandlerResult;
#[cfg(feature = "caching")]
use crate::utils::hashmap_to_headers;
use crate::utils::{add_headers_to_response_builder, error_response};

#[cfg(feature = "caching")]
pub fn build_cached_response(cached: CachedResponse) -> HandlerResult {
    let mut builder = axum::http::Response::builder().status(cached.status);
    let cached_header_map = hashmap_to_headers(&cached.headers);
    builder = add_headers_to_response_builder(builder, &cached_header_map);
    builder = builder.header("X-Cache", "HIT");

    let body_size = cached.body.len();
    builder
        .body(axum::body::Body::from(cached.body))
        .map_err(|e| {
            tracing::error!(
                cached_status = cached.status,
                body_size = body_size,
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
