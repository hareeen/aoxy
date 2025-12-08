use crate::models::CachedResponse;

#[cfg(feature = "redis")]
pub async fn try_get_cached_response(
    pool: Option<&deadpool_redis::Pool>,
    cache_key: &str,
) -> Option<CachedResponse> {
    let pool = pool?;
    let mut conn = pool.get().await.ok()?;
    let cached_json: Option<String> = redis::cmd("GET")
        .arg(cache_key)
        .query_async(&mut conn)
        .await
        .ok()?;

    cached_json.and_then(|json| serde_json::from_str(&json).ok())
}

#[cfg(not(feature = "redis"))]
pub async fn try_get_cached_response(_cache_key: &str) -> Option<CachedResponse> {
    None
}

#[cfg(feature = "redis")]
pub async fn cache_response(
    pool: Option<&deadpool_redis::Pool>,
    cache_key: &str,
    response: &CachedResponse,
    ttl_secs: usize,
) {
    let Some(pool) = pool else {
        return;
    };
    let Ok(mut conn) = pool.get().await else {
        return;
    };
    let Ok(json) = serde_json::to_string(response) else {
        return;
    };
    let _: Result<(), _> = redis::cmd("SETEX")
        .arg(cache_key)
        .arg(ttl_secs)
        .arg(&json)
        .query_async(&mut conn)
        .await;
}

#[cfg(not(feature = "redis"))]
pub async fn cache_response(_cache_key: &str, _response: &CachedResponse, _ttl_secs: usize) {
    // No-op when redis feature is disabled
}
