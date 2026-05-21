# syntax=docker/dockerfile:1.7
#
# frank-sync-agent multi-stage Dockerfile.
#
# 构建产物: ~15-30 MB 静态二进制 + ca-certificates (CA bundle 给 rustls/webpki 用)。
# 目标 size < 100 MB (实际约 80-90 MB,大头是 debian:12-slim 基础镜像 ~75 MB)。
#
# 构建命令 (本机 macOS arm64 → linux/amd64 跨平台):
#
#   docker buildx build \
#     --platform linux/amd64 \
#     -f deploy/sync-agent.Dockerfile \
#     -t frank-sync-agent:0.1.0 \
#     --output type=docker,dest=/tmp/frank-sync-agent.tar \
#     .
#
# 在 tx (x86_64) 上 build 用 plain docker build 即可。
#
# 注意 build context = workspace root (有 Cargo.toml + crates/), 不是 deploy/。

# ---- stage 1: builder ----
# rust:1-slim (latest stable) — 注: Cargo.lock 里某些 transitive deps (idna_adapter 1.2+) 需
# edition 2024, 这在 cargo 1.85+ (rust 1.85+) 才稳定. workspace 声明的 rust-version = 1.75
# 只是 MSRV 提示, 构建用 newer cargo 完全兼容.
FROM rust:1-slim-bookworm AS builder

# 切换到阿里云镜像: deb.debian.org 在国内访问偶尔 timeout (60+秒/包), 阿里云镜像稳定
# Debian 12 bookworm 用 deb822 格式 (debian.sources)
RUN sed -i 's|http://deb.debian.org|http://mirrors.aliyun.com|g' \
        /etc/apt/sources.list.d/debian.sources

# protobuf-compiler: qdrant-client 走 gRPC, build.rs 需要 protoc 生成 PB stub
# pkg-config + libssl-dev: 保险起见装上 (reqwest 走 rustls 理论不需要, 但 transitive 偶尔会 link;
#                          不装的话失败成本高、装上的话只在 builder stage 增加体积, runtime 不受影响)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        protobuf-compiler \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

# cargo 用国内镜像 (rsproxy.cn = 中科大维护的 rust 全镜像); --locked 时 cargo 仍会查 registry
# 索引, 没镜像在 tx 上拉 crates.io 偶尔 60s+ timeout
RUN mkdir -p /usr/local/cargo \
    && printf '%s\n' \
       '[source.crates-io]' \
       'replace-with = "rsproxy-sparse"' \
       '[source.rsproxy]' \
       'registry = "https://rsproxy.cn/crates.io-index"' \
       '[source.rsproxy-sparse]' \
       'registry = "sparse+https://rsproxy.cn/index/"' \
       '[registries.rsproxy]' \
       'index = "https://rsproxy.cn/crates.io-index"' \
       '[net]' \
       'git-fetch-with-cli = true' \
       > /usr/local/cargo/config.toml

# 先 COPY manifest 触发 deps 解析 / pre-cache (但因为我们用 path 依赖 + 没用 cargo-chef, 这里
# 不能真正按 layer-cache 玩, 直接整库 COPY 也行)
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# 编 release; --locked 强制走 Cargo.lock (CI 友好, 防 deps 飘);
# RUSTFLAGS 不开 link-arg 静态化 — 我们走 glibc(debian:12-slim 同 family) 已可移植
RUN cargo build --release -p frank-sync-agent --locked

# ---- stage 2: runtime ----
FROM debian:12-slim AS runtime

# 切换到阿里云镜像 (同 builder stage 原因)
RUN sed -i 's|http://deb.debian.org|http://mirrors.aliyun.com|g' \
        /etc/apt/sources.list.d/debian.sources

# ca-certificates: rustls + webpki-roots 是装在 binary 里的, 但 OS-level CA bundle 装上
# 也不亏 (后续真接 OpenAI / Anthropic 走 HTTPS, 双保险)
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

# 二进制放到 PATH; debian:12-slim 自带 nobody:65534
COPY --from=builder /workspace/target/release/frank-sync-agent /usr/local/bin/frank-sync-agent

# 服务在 0.0.0.0:3000 监听 (FRANK_BIND_ADDR 默认值);
# 由 docker-compose expose, 不绑 host port
EXPOSE 3000

USER nobody

ENTRYPOINT ["/usr/local/bin/frank-sync-agent"]
