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
      protobuf-compiler pkg-config libssl-dev git ca-certificates \
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
ENV SENTINEL_WEB_DIR=/app/web/dist \
    SENTINEL_DB=/data/sentinel.db \
    SENTINEL_PORT=8080 \
    RUST_LOG=info,h2=warn,hyper=warn,tower=warn
VOLUME /data
EXPOSE 8080
CMD ["sentinel"]
