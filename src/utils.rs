use std::collections::HashMap;

use sha2::{Digest, Sha256};

/// Generate a cache key using SHA256 hash of method and URL.
pub fn generate_cache_key(method: &reqwest::Method, url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str());
    hasher.update(":");
    hasher.update(url);
    format!("cache:{:x}", hasher.finalize())
}

/// Check if a header is a hop-by-hop header that should not be forwarded.
pub fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "upgrade"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "te"
            | "trailer"
    )
}

/// Parse default headers from JSON string.
pub fn parse_default_headers(json: Option<&String>) -> Result<HashMap<String, String>, String> {
    match json {
        Some(s) => serde_json::from_str(s).map_err(|e| format!("Invalid DEFAULT_HEADERS: {e}")),
        None => Ok(HashMap::new()),
    }
}

/// Build an error response with the given status and message.
pub fn error_response(
    status: reqwest::StatusCode,
    message: String,
) -> (reqwest::StatusCode, String) {
    (status, message)
}

/// Filter and collect headers, excluding hop-by-hop headers.
pub fn filter_headers(headers: &axum::http::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop_header(name.as_str()))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), v.to_string()))
        })
        .collect()
}

/// Add headers to a response builder, filtering hop-by-hop headers.
pub fn add_headers_to_response(
    mut builder: axum::http::response::Builder,
    headers: &axum::http::HeaderMap,
) -> axum::http::response::Builder {
    for (name, value) in headers.iter() {
        if !is_hop_by_hop_header(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    builder
}

/// Add headers from a HashMap to a response builder.
pub fn add_cached_headers_to_response(
    mut builder: axum::http::response::Builder,
    headers: &HashMap<String, String>,
) -> axum::http::response::Builder {
    for (name, value) in headers {
        if let (Ok(header_name), Ok(header_value)) = (
            axum::http::HeaderName::try_from(name.as_str()),
            axum::http::HeaderValue::try_from(value.as_str()),
        ) {
            builder = builder.header(header_name, header_value);
        }
    }
    builder
}
