# Multi-stage build for the act-api-server Rust binary.
# Dependencies are cached in a separate layer so source-only changes rebuild fast.
FROM rust:1-bookworm AS builder
WORKDIR /usr/src/app

# Pre-build dependencies against a stub main to leverage layer caching.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

# Build the real sources.
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Run as an unprivileged user.
RUN useradd --system --uid 10001 --no-create-home appuser
USER 10001

WORKDIR /app
COPY --from=builder /usr/src/app/target/release/act_api_server /usr/local/bin/act-api-server

ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["act-api-server"]
