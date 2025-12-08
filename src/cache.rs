use crate::models::CachedResponse;

pub async fn try_get_cached_response(
    pool: &deadpool_redis::Pool,
    cache_key: &str,
) -> Option<CachedResponse> {
    let mut conn = pool.get().await.ok()?;
    let cached_json: Option<String> = redis::cmd("GET")
        .arg(cache_key)
        .query_async(&mut conn)
        .await
        .ok()?;

    cached_json.and_then(|json| serde_json::from_str(&json).ok())
}

pub async fn cache_response(
    pool: &deadpool_redis::Pool,
    cache_key: &str,
    response: &CachedResponse,
    ttl_secs: usize,
) {
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
