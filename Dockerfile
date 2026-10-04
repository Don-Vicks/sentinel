# --- dashboard -------------------------------------------------------------
FROM node:22-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# --- server ----------------------------------------------------------------
FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential protobuf-compiler libprotobuf-dev pkg-config libssl-dev git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY examples ./examples
RUN cargo build --release --bin sentinel

# --- runtime ---------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/sentinel /usr/local/bin/sentinel
COPY --from=web /web/dist ./web/dist
# The database lives in /data. Mount a volume there to keep it across restarts: docker-compose
# does (see docker-compose.yml) and on Railway you add a Volume at /data. A Dockerfile VOLUME
# instruction is deliberately not used: Railway rejects it.
RUN mkdir -p /data
ENV SENTINEL_WEB_DIR=/app/web/dist \
    SENTINEL_DB=/data/sentinel.db \
    MALLOC_ARENA_MAX=2 RUST_LOG=info,h2=warn,hyper=warn,tower=warn
# The port comes from SENTINEL_PORT, else PORT (Railway sets it), else 8080.
EXPOSE 8080
CMD ["sentinel"]
