# syntax=docker/dockerfile:1

# ── 前端 ────────────────────────────────────────────────────────────────
FROM node:22-alpine AS web
WORKDIR /web
# 小内存机器限制 Node 堆，避免 Vite 构建把 1.8GB 机器打满。
ENV NODE_OPTIONS=--max-old-space-size=768
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-fund --no-audit
COPY web/ ./
RUN npm run build

# ── Rust 服务 ───────────────────────────────────────────────────────────
FROM rust:1.90-bookworm AS build
WORKDIR /src
# 2C1.8G 机器建议 1；默认 2 适配 2C4G 以上。
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# 三个编译期常量：与 constants.rs 的 option_env! 对应。
# 用 --build-arg 或 compose 的 build.args 注入；为空时镜像仍可启动，但登录/推送会失败。
ARG GLACC_CLIENT_ID=""
ARG GLACC_CLIENT_SECRET=""
ARG GLACC_SIGN_KEY=""
ENV GLACC_CLIENT_ID=${GLACC_CLIENT_ID} \
    GLACC_CLIENT_SECRET=${GLACC_CLIENT_SECRET} \
    GLACC_SIGN_KEY=${GLACC_SIGN_KEY}

RUN cargo build --release

# ── 运行时 ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin glacc \
    && mkdir -p /app/web /app/lib /data \
    && chown -R 10001:10001 /app /data

COPY --from=build /src/target/release/glacc-auto /app/glacc-auto
COPY --from=web /web/dist /app/web/dist
COPY LICENSE NOTICE /app/
# Linux 版 libtls-client.so（仓库已内置 v1.18.1 linux/amd64）一并进镜像。
COPY lib/ /app/lib/
RUN chown -R 10001:10001 /app

ENV GLACC_LISTEN=0.0.0.0:8080 \
    GLACC_DATA_DIR=/data \
    GLACC_WEB_DIR=/app/web/dist \
    GLACC_TLS_LIB=/app/lib/libtls-client.so \
    RUST_LOG=info

USER 10001
EXPOSE 8080
VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/app/glacc-auto"]
