use std::collections::HashMap;

use sha2::{Digest, Sha256};

/// Generate a cache key using SHA256 hash of method and URL.
pub fn generate_cache_key(method: &reqwest::Method, url: &str, body: &bytes::Bytes) -> String {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str());
    hasher.update(":");
    hasher.update(url);
    hasher.update(":");
    hasher.update(body);
    format!("cache:{:x}", hasher.finalize())
}

/// Check if a header is a hop-by-hop header that should not be forwarded.
pub fn is_hop_by_hop_header(name: &axum::http::header::HeaderName) -> bool {
    matches!(
        name.as_str().to_lowercase().as_str(),
        "host"
            | "connection"
            | "proxy-connection"
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
pub fn parse_default_headers(
    json: Option<&String>,
) -> Result<axum::http::HeaderMap, Box<dyn std::error::Error>> {
    match json {
        Some(s) => {
            let map: HashMap<String, String> = serde_json::from_str(s)
                .map_err(|e| format!("Failed to parse default headers JSON: {}", e))?;
            let mut headers = axum::http::HeaderMap::new();
            for (name, value) in map {
                let header_name = axum::http::HeaderName::try_from(name.as_str())
                    .map_err(|e| format!("Invalid header name '{}': {}", name, e))?;
                let header_value = axum::http::HeaderValue::try_from(value.as_str())
                    .map_err(|e| format!("Invalid header value for '{}': {}", name, e))?;
                headers.insert(header_name, header_value);
            }
            Ok(headers)
        }
        None => Ok(axum::http::HeaderMap::new()),
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
pub fn filter_headers(headers: &axum::http::HeaderMap) -> axum::http::HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop_header(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<axum::http::HeaderMap>()
}

/// Convert headers to a HashMap<String, Vec<String>>.
pub fn headers_to_hashmap(headers: &axum::http::HeaderMap) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            map.entry(name.as_str().to_string())
                .or_insert_with(Vec::new)
                .push(value_str.to_string());
        }
    }
    map
}

/// Convert a HashMap<String, Vec<String>> to headers.
pub fn hashmap_to_headers(map: &HashMap<String, Vec<String>>) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for (name, values) in map {
        if let Ok(header_name) = axum::http::HeaderName::try_from(name.as_str()) {
            for value in values {
                if let Ok(header_value) = axum::http::HeaderValue::try_from(value.as_str()) {
                    headers.append(header_name.clone(), header_value);
                }
            }
        }
    }
    headers
}

/// Add headers to axum::http::Response::Builder.
pub fn add_headers_to_response_builder(
    mut builder: axum::http::response::Builder,
    headers: &axum::http::HeaderMap,
) -> axum::http::response::Builder {
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }

    builder
}
