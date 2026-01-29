use crate::models::CachedResponse;

#[cfg(feature = "redis")]
pub async fn try_get_cached_response(
    pool: Option<&deadpool_redis::Pool>,
    cache_key: &str,
) -> Option<CachedResponse> {
    use deadpool_redis::redis::AsyncCommands;

    let pool = pool?;

    let mut con = match pool.get().await {
        Ok(con) => con,
        Err(e) => {
            tracing::warn!(cache_key = %cache_key, error = %e, "Failed to get Redis connection from pool");
            return None;
        }
    };

    let cached_json: Option<String> = match con.get(cache_key).await {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(cache_key = %cache_key, error = %e, "Redis GET command failed");
            return None;
        }
    };

    match cached_json {
        Some(json) => match serde_json::from_str(&json) {
            Ok(cached) => Some(cached),
            Err(e) => {
                tracing::error!(cache_key = %cache_key, error = %e, "Failed to deserialize cached response");
                None
            }
        },
        None => None,
    }
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
    ttl_secs: u64,
) {
    use deadpool_redis::redis::AsyncCommands;

    let Some(pool) = pool else {
        return;
    };

    let mut con = match pool.get().await {
        Ok(con) => con,
        Err(e) => {
            tracing::warn!(cache_key = %cache_key, error = %e, "Failed to get Redis connection from pool for caching");
            return;
        }
    };

    let json = match serde_json::to_string(response) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!(cache_key = %cache_key, error = %e, "Failed to serialize response for caching");
            return;
        }
    };

    if let Err(e) = con.set_ex::<_, _, ()>(cache_key, &json, ttl_secs).await {
        tracing::warn!(cache_key = %cache_key, ttl_secs = %ttl_secs, error = %e, "Redis SETEX command failed");
    } else {
        tracing::debug!(cache_key = %cache_key, ttl_secs = %ttl_secs, "Cached response successfully");
    }
}

#[cfg(not(feature = "redis"))]
pub async fn cache_response(_cache_key: &str, _response: &CachedResponse, _ttl_secs: usize) {
    // No-op when redis feature is disabled
}
