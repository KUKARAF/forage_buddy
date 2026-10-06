# syntax=docker/dockerfile:1

# ---- frontend build -------------------------------------------------------
FROM node:22-bookworm-slim AS frontend
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
# Short commit SHA baked into the build (shown in the UI footer) so "are you
# actually running the latest build" is never a guessing game when debugging.
ARG GIT_SHA=dev
ENV PUBLIC_BUILD_SHA=$GIT_SHA
RUN npm run build

# ---- backend build ---------------------------------------------------------
FROM rust:bookworm AS backend
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
# docs/ is copied in case the server embeds any docs via include_str! at build
# time (harmless if it doesn't).
COPY docs/ ./docs/
RUN cargo build --release -p server

# ---- runtime -----------------------------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin foragebuddy

WORKDIR /app
COPY --from=backend /app/target/release/server /app/server
COPY --from=frontend /app/web/build /app/static

# Default data locations inside the container. `FORAGEBUDDY_SQLITE_PATH`'s and
# `FORAGEBUDDY_PHOTO_DIR`'s parent dirs should be persistent volumes so the DB
# and uploaded sighting photos survive restarts (no GeoIP database here -
# unlike ai_buddy, this app gets real GPS coordinates from the client, so it
# has no use for IP->country lookup).
ENV FORAGEBUDDY_SQLITE_PATH=/data/db/forage_buddy.db \
    FORAGEBUDDY_PHOTO_DIR=/data/photos \
    FORAGEBUDDY_STATIC_DIR=/app/static \
    FORAGEBUDDY_BIND_ADDR=0.0.0.0:8080

# RUST_LOG is intentionally not defaulted here — set it via the deploy
# infra's docker-compose `environment:` instead. main.rs's EnvFilter setup
# honors it when set (e.g. RUST_LOG=info, or RUST_LOG=info,server=debug for
# just this app's own target) and otherwise falls back to "info" itself.

RUN mkdir -p /data/photos /data/db && chown -R foragebuddy:foragebuddy /data /app
USER foragebuddy

EXPOSE 8080
ENTRYPOINT ["/app/server"]
