# --- Build Stage ---
FROM rust:alpine AS builder

RUN apk add --no-cache build-base musl-dev

WORKDIR /app
COPY . .

# Build the Rust application
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp /app/target/release/aoxy /app/aoxy

# --- Runtime Stage ---
FROM alpine:latest

WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /app/aoxy /app/aoxy

ENTRYPOINT ["/app/aoxy"]
