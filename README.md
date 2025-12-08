# aoxy: Axum Proxy with Optional Redis Caching & Rate Limiting

`aoxy` is a high-performance HTTP proxy server built with Rust, leveraging [Axum](https://github.com/tokio-rs/axum) for async web handling, [Governor](https://docs.rs/governor/) for rate limiting, and optionally [Redis](https://redis.io/) for response caching. It is designed to forward all incoming HTTP requests to a configurable upstream API, cache idempotent responses (when Redis is enabled), and enforce global rate limits with robust retry and backoff strategies.

## Features

Forwards all HTTP requests to a specified external API.

- **Optional Redis Caching:** Caches idempotent (GET, HEAD, etc.) responses for configurable TTL when compiled with the `redis` feature and a Redis URL is provided.
- **Global Rate Limiting:** Restricts outbound requests per second using a token bucket algorithm.
- **Retry with Exponential Backoff:** Retries failed upstream requests with configurable backoff and timeout.
- **Proxy Support:** Supports HTTP, HTTPS, and SOCKS5 proxies with optional authentication.
- **Default Headers:** Add custom headers to all upstream requests via JSON configuration.
- **Configurable via Environment Variables or CLI Args**
- **Production-Ready:** Graceful error handling, logging, and timeout controls.

## Usage

### 1. Build

```sh
# Build without Redis caching (default)
cargo build --release

# Build with Redis caching support
cargo build --release --features redis
```

### 2. Run

You can configure `aoxy` via environment variables or CLI arguments:

```sh
# Example with environment variables (without Redis)
export BIND_ADDR=0.0.0.0:8080
export EXTERNAL_API_BASE=https://api.example.com
export RATE_LIMIT_PER_SEC=10
export UPSTREAM_TIMEOUT_SECS=30
export MAX_ELAPSED_TIME_SECS=30
export INITIAL_BACKOFF_MS=200
# Optional: Configure redis
export REDIS_URL=redis://127.0.0.1/
export CACHE_TTL_SECS=600
# Optional: Configure proxy
export PROXY_URL=socks5://127.0.0.1:1080
export PROXY_USERNAME=myuser
export PROXY_PASSWORD=mypass
# Optional: Add default headers to all upstream requests
export DEFAULT_HEADERS='{"Authorization":"Bearer token123","X-API-Key":"key456"}'

./target/release/aoxy
```

With Redis caching enabled (requires building with `--features redis`):

```sh
export BIND_ADDR=0.0.0.0:8080
export EXTERNAL_API_BASE=https://api.example.com
export REDIS_URL=redis://127.0.0.1/
export RATE_LIMIT_PER_SEC=10
export CACHE_TTL_SECS=600

./target/release/aoxy
```

Or with CLI arguments:

```sh
./target/release/aoxy \
  --bind-addr 0.0.0.0:8080 \
  --external-api-base https://api.example.com \
  --rate-limit-per-sec 10 \
  --rate-limit-burst 1 \
  --upstream-timeout-secs 30 \
  --max-elapsed-time-secs 30 \
  --initial-backoff-ms 200 \
  --proxy-url socks5://127.0.0.1:1080 \
  --proxy-username myuser \
  --proxy-password mypass \
  --default-headers '{"Authorization":"Bearer token123","X-API-Key":"key456"}'
```

With Redis (requires `--features redis`):

```sh
./target/release/aoxy \
  --bind-addr 0.0.0.0:8080 \
  --external-api-base https://api.example.com \
  --redis-url redis://127.0.0.1/ \
  --cache-ttl-secs 600 \
  --rate-limit-per-sec 10
```

## Environment Variables / CLI Arguments

| Name                     | CLI Arg                    | Default           | Description                                                                       |
| ------------------------ | -------------------------- | ----------------- | --------------------------------------------------------------------------------- |
| `BIND_ADDR`              | `--bind-addr`              | `0.0.0.0:8080`    | Address and port to listen on                                                     |
| `EXTERNAL_API_BASE`      | `--external-api-base`      | _(required)_      | Base URL of the upstream API to proxy requests to                                 |
| `REDIS_URL`              | `--redis-url`              | _(optional)_      | Redis connection string for caching (requires `redis` feature)                    |
| `RATE_LIMIT_PER_SEC`     | `--rate-limit-per-sec`     | `10`              | Max outbound requests per second (global)                                         |
| `RATE_LIMIT_BURST`       | `--rate-limit-burst`       | `1`               | Burst capacity for rate limiting                                                  |
| `CACHE_TTL_SECS`         | `--cache-ttl-secs`         | `600`             | Cache time-to-live in seconds for idempotent responses                            |
| `SKIP_IDEMPOTENCY_CHECK` | `--skip-idempotency-check` | `false`           | Skip idempotency check; cache all methods (useful for known idempotent upstreams) |
| `UPSTREAM_TIMEOUT_SECS`  | `--upstream-timeout-secs`  | `30`              | Timeout for each upstream request (seconds)                                       |
| `MAX_ELAPSED_TIME_SECS`  | `--max-elapsed-time-secs`  | `30`              | Max total retry time for upstream requests (seconds)                              |
| `INITIAL_BACKOFF_MS`     | `--initial-backoff-ms`     | `200`             | Initial backoff interval for retries (milliseconds)                               |
| `MAX_BODY_SIZE`          | `--max-body-size`          | `10485760` (10MB) | Max response size to buffer/cache. Larger = streaming                             |
| `PROXY_URL`              | `--proxy-url`              | _(optional)_      | Proxy URL (supports http, https, socks5)                                          |
| `PROXY_USERNAME`         | `--proxy-username`         | _(optional)_      | Proxy username for authentication                                                 |
| `PROXY_PASSWORD`         | `--proxy-password`         | _(optional)_      | Proxy password for authentication                                                 |
| `DEFAULT_HEADERS`        | `--default-headers`        | _(optional)_      | Default headers as JSON string (e.g., `'{"key":"val"}'`)                          |

## How It Works

1. **Request Handling:** All incoming HTTP requests are forwarded to the configured upstream API, preserving method, path, query, and headers (except hop-by-hop headers like `Host`, `Connection`, `Transfer-Encoding`).
2. **Default Headers:** Custom headers from `DEFAULT_HEADERS` are added to all upstream requests. Incoming request headers override default headers if they have the same name.
3. **Status & Headers Preservation:** The proxy preserves the exact HTTP status code and all headers from upstream responses (except hop-by-hop headers). This ensures proper handling of redirects, authentication, CORS, and caching directives.
4. **Redirect Handling:** The proxy does NOT follow redirects automatically. Instead, it returns 3xx responses with `Location` headers to clients, allowing them to handle redirects appropriately.
5. **Streaming for Large Responses:** When `Content-Length` exceeds `MAX_BODY_SIZE` (default 10MB), responses are streamed directly to clients without buffering or caching. This prevents memory issues with large files. Streamed responses include an `X-Cache: STREAM` header. Set `MAX_BODY_SIZE=0` to disable streaming (always buffer).
6. **Caching (Optional):** When compiled with the `redis` feature and `REDIS_URL` is provided, idempotent methods (GET, HEAD, etc.) with successful responses (2xx/3xx) under `MAX_BODY_SIZE` are cached in Redis as JSON (including status, headers, and base64-encoded body). Cache keys use SHA256 hashing for collision prevention. Cached responses include an `X-Cache: HIT` header; cache misses include `X-Cache: MISS`. Without Redis, caching is disabled. When `SKIP_IDEMPOTENCY_CHECK` is enabled, all HTTP methods are eligible for caching regardless of idempotency—useful for upstreams known to be fully idempotent.
7. **Rate Limiting:** Outbound requests are globally rate-limited using a token bucket. Requests exceeding the rate are queued until a permit is available.
8. **Retry & Backoff:** Upstream failures (network errors, 5xx, or 429) are retried with exponential backoff up to a configurable maximum elapsed time. Client errors (4xx, except 429) are returned immediately without retry.
9. **Timeouts:** Each upstream request has a configurable timeout.
10. **Logging:** All major events (requests, cache hits/misses/streams, retries, errors) are logged.

## Docker

A production-ready Dockerfile is provided. To build and run:

```sh
# Without Redis support
docker build -t aoxy .
docker run --rm -p 8080:8080 \
  -e EXTERNAL_API_BASE=https://api.example.com \
  aoxy

# With Redis support
docker build -t aoxy --build-arg FEATURES=redis .
docker run --rm -p 8080:8080 \
  -e EXTERNAL_API_BASE=https://api.example.com \
  -e REDIS_URL=redis://host.docker.internal/ \
  aoxy
```

## Examples

### Basic proxy without caching

```sh
export EXTERNAL_API_BASE=https://api.example.com
export RATE_LIMIT_PER_SEC=10

./target/release/aoxy
```

### Proxy with Redis caching

Build with Redis support first: `cargo build --release --features redis`

```sh
export EXTERNAL_API_BASE=https://api.example.com
export REDIS_URL=redis://127.0.0.1/
export RATE_LIMIT_PER_SEC=10
export CACHE_TTL_SECS=600

./target/release/aoxy
```

### Using a SOCKS5 proxy with authentication

```sh
export EXTERNAL_API_BASE=https://api.example.com
export PROXY_URL=socks5://127.0.0.1:1080
export PROXY_USERNAME=myuser
export PROXY_PASSWORD=mypass

./target/release/aoxy
```

### Using an HTTP proxy without authentication

```sh
export EXTERNAL_API_BASE=https://api.example.com
export PROXY_URL=http://proxy.example.com:8080

./target/release/aoxy
```

### Using default headers for API authentication

```sh
export EXTERNAL_API_BASE=https://api.example.com
export DEFAULT_HEADERS='{"Authorization":"Bearer your-token-here","X-Custom-Header":"value"}'

./target/release/aoxy
```

**Note:** All response headers (including `Content-Type`, `Set-Cookie`, `Location`, etc.) are preserved from upstream responses and returned to clients. When Redis caching is enabled, the proxy adds an `X-Cache` header (`HIT`, `MISS`, or `STREAM`) to indicate cache status.

## Requirements

- Rust (1.70+ recommended)
- Redis server (optional, only needed when using the `redis` feature)
- Upstream API endpoint

## License

MIT
