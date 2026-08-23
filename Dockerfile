# Multi-stage build for the act-api-server Rust binary.
# Dependencies are cached in a separate layer so source-only changes rebuild fast.
FROM rust:1-bookworm AS builder
WORKDIR /usr/src/app

# Pre-build dependencies against a stub main to leverage layer caching.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --locked --release \
    && rm -rf src

# Build the real sources.
COPY src ./src
RUN touch src/main.rs && cargo build --locked --release

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

# --- sops: decrypt at `docker run`, never at `docker build` ------------------
# The image carries only CIPHERTEXT (env/enc/<SOPS_ENV>.env.enc) and the sops
# binary. The age key arrives at run time (SOPS_AGE_KEY / SOPS_AGE_KEY_FILE);
# scripts/sops-entrypoint.sh decrypts into the process environment and execs
# the real command, so no plaintext ever lands in a layer or on disk.
# See env/README.md.
ARG SOPS_ENV=prod
COPY --chmod=0755 --from=ghcr.io/getsops/sops:v3.10.2-alpine /usr/local/bin/sops /usr/local/bin/sops
COPY --chmod=0755 scripts/sops-entrypoint.sh /usr/local/bin/sops-entrypoint.sh
COPY --chmod=0644 env/enc/${SOPS_ENV}.env.enc /app/secrets/app.env
ENV SOPS_SECRETS_FILE=/app/secrets/app.env

ENTRYPOINT ["/usr/local/bin/sops-entrypoint.sh", "act-api-server"]
