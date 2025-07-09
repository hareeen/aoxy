# aoxy: Actix-Web Proxy with Redis Caching & Rate Limiting

`aoxy` is a high-performance HTTP proxy server built with Rust, leveraging [Actix-web](https://actix.rs/) for async web handling, [Governor](https://docs.rs/governor/) for rate limiting, and [Redis](https://redis.io/) for response caching. It is designed to forward all incoming HTTP requests to a configurable upstream API, cache idempotent responses, and enforce global rate limits with robust retry and backoff strategies.

## Features

- **Universal Proxy:** Forwards all HTTP requests to a specified external API.
- **Redis Caching:** Caches idempotent (GET, HEAD, etc.) responses for configurable TTL.
- **Global Rate Limiting:** Restricts outbound requests per second using a token bucket algorithm.
- **Retry with Exponential Backoff:** Retries failed upstream requests with configurable backoff and timeout.
- **Configurable via Environment Variables or CLI Args**
- **Production-Ready:** Graceful error handling, logging, and timeout controls.

## Usage

### 1. Build

```sh
cargo build --release
```

### 2. Run

You can configure `aoxy` via environment variables or CLI arguments:

```sh
# Example with environment variables
export BIND_ADDR=0.0.0.0:8080
export EXTERNAL_API_BASE=https://api.example.com
export REDIS_URL=redis://127.0.0.1/
export RATE_LIMIT_PER_SEC=10
export CACHE_TTL_SECS=600
export UPSTREAM_TIMEOUT_SECS=30
export MAX_ELAPSED_TIME_SECS=30
export INITIAL_BACKOFF_MS=200

./target/release/aoxy
```

Or with CLI arguments:

```sh
./target/release/aoxy \
  --bind-addr 0.0.0.0:8080 \
  --external-api-base https://api.example.com \
  --redis-url redis://127.0.0.1/ \
  --rate-limit-per-sec 10 \
  --cache-ttl-secs 600 \
  --upstream-timeout-secs 30 \
  --max-elapsed-time-secs 30 \
  --initial-backoff-ms 200
```

## Environment Variables / CLI Arguments

| Name                    | CLI Arg                  | Default                | Description                                                      |
|-------------------------|--------------------------|------------------------|------------------------------------------------------------------|
| `BIND_ADDR`             | `--bind-addr`            | `0.0.0.0:8080`         | Address and port to listen on                                    |
| `EXTERNAL_API_BASE`     | `--external-api-base`    | *(required)*           | Base URL of the upstream API to proxy requests to                |
| `REDIS_URL`             | `--redis-url`            | `redis://127.0.0.1/`   | Redis connection string for caching                              |
| `RATE_LIMIT_PER_SEC`    | `--rate-limit-per-sec`   | `10`                   | Max outbound requests per second (global)                        |
| `CACHE_TTL_SECS`        | `--cache-ttl-secs`       | `600`                  | Cache time-to-live in seconds for idempotent responses           |
| `UPSTREAM_TIMEOUT_SECS` | `--upstream-timeout-secs`| `30`                   | Timeout for each upstream request (seconds)                      |
| `MAX_ELAPSED_TIME_SECS` | `--max-elapsed-time-secs`| `30`                   | Max total retry time for upstream requests (seconds)             |
| `INITIAL_BACKOFF_MS`    | `--initial-backoff-ms`   | `200`                  | Initial backoff interval for retries (milliseconds)              |

## How It Works

1. **Request Handling:** All incoming HTTP requests are forwarded to the configured upstream API, preserving method, path, query, and headers (except `Host`).
2. **Caching:** For idempotent methods (GET, HEAD, etc.), responses are cached in Redis using a key based on method and full upstream URL. Cache TTL is configurable.
3. **Rate Limiting:** Outbound requests are globally rate-limited using a token bucket. Requests exceeding the rate are queued until a permit is available.
4. **Retry & Backoff:** Upstream failures (network errors, 5xx, or 429) are retried with exponential backoff up to a configurable maximum elapsed time.
5. **Timeouts:** Each upstream request has a configurable timeout.
6. **Logging:** All major events (requests, cache hits, retries, errors) are logged.

## Docker

A production-ready Dockerfile is provided. To build and run:

```sh
docker build -t aoxy .
docker run --rm -p 8080:8080 \
  -e EXTERNAL_API_BASE=https://api.example.com \
  -e REDIS_URL=redis://host.docker.internal/ \
  aoxy
```

## Example

Proxy all requests to `https://api.example.com`, caching GET responses for 10 minutes, and limiting to 10 requests per second:

```sh
export EXTERNAL_API_BASE=https://api.example.com
export REDIS_URL=redis://127.0.0.1/
export RATE_LIMIT_PER_SEC=10
export CACHE_TTL_SECS=600

./target/release/aoxy
```

---

## Requirements

- Rust (1.70+ recommended)
- Redis server (for caching)
- Upstream API endpoint

## License

MIT
