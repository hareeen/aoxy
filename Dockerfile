# --- Build Stage ---
FROM rust:alpine AS builder

ARG FEATURES=""

RUN apk add --no-cache build-base musl-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build the Rust application with optional features
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    if [ -n "$FEATURES" ]; then \
        cargo build --release --features "$FEATURES"; \
    else \
        cargo build --release; \
    fi && \
    cp /app/target/release/aoxy /app/aoxy

# --- Runtime Stage ---
FROM alpine:latest

WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /app/aoxy /app/aoxy

ENTRYPOINT ["/app/aoxy"]
