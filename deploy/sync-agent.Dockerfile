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
FROM rust:1-slim-trixie AS builder
# v0.8.1: bookworm(glibc 2.36) → trixie(glibc 2.41)
# onnxruntime 5.x prebuilt (fastembed dep) 引用 __isoc23_strtoull (ISO C 2023),
# 需 glibc 2.38+. 不升 base 直接 rust-lld 报 undefined symbol 链接失败.

# v0.8: deb.debian.org 走 fastly CDN, 跨平台 emulation 下比阿里云稳定
# (实测阿里云在 buildx --platform 模拟下偶尔 connection failed)
# 重试 3 次 + 短 timeout, 防偶发网络抖动
RUN for i in 1 2 3; do \
        apt-get update \
        && apt-get install -y --no-install-recommends \
            pkg-config \
            libssl-dev \
            protobuf-compiler \
            g++ \
            libstdc++-12-dev \
            ca-certificates \
        && rm -rf /var/lib/apt/lists/* \
        && break || { echo "apt attempt $i failed, retry..."; sleep 5; }; \
    done

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
FROM debian:trixie-slim AS runtime
# v0.8.1: 同 builder, runtime 用 trixie 才能 dynamic link 到 glibc 2.41+
# (binary 在 builder 上链接 isoc23 符号, runtime 必须有对应 glibc 版本)

# 切换到阿里云镜像 (同 builder stage 原因)
RUN sed -i 's|http://deb.debian.org|http://mirrors.aliyun.com|g' \
        /etc/apt/sources.list.d/debian.sources

# ca-certificates: rustls + webpki-roots 是装在 binary 里的, 但 OS-level CA bundle 装上
# 也不亏 (后续真接 OpenAI / Anthropic 走 HTTPS, 双保险)
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

# 二进制放到 PATH; debian:trixie-slim 自带 nobody:65534
COPY --from=builder /workspace/target/release/frank-sync-agent /usr/local/bin/frank-sync-agent

# v0.10.10: 模型不再 COPY 进镜像, 改为 volume 挂载 (镜像 572MB → ~80MB).
# 部署侧 docker-compose 把 host /opt/frank/models 挂到 /home/nobody/.cache/huggingface,
# 首次部署用 deploy/scripts/init-models.sh 在 host 下好模型, 之后所有 docker pull 只拉 binary.
# 历史背景 (v0.8.1): 原方案是 GH Actions runner 预下 + COPY, 解决 tx 连 HF 不稳;
# v0.10.10 改 volume 后 init-models.sh 也走 hf-mirror.com 国内镜像, 同样绕开 GFW.
RUN mkdir -p /home/nobody/.cache/huggingface && chown -R nobody:nogroup /home/nobody
ENV HOME=/home/nobody
ENV FASTEMBED_CACHE_DIR=/home/nobody/.cache/huggingface/hub
# 注意: hf-cli (python) 写到 $HF_HOME/hub/models--...; 但 hf-hub crate 通过 ApiBuilder.with_cache_dir
# 直接用 cache_dir/models--... (不加 hub 子目录). 所以这里 FASTEMBED_CACHE_DIR 要带 /hub 后缀对齐 layout.
WORKDIR /home/nobody

# 服务在 0.0.0.0:3000 监听 (FRANK_BIND_ADDR 默认值);
# 由 docker-compose expose, 不绑 host port
EXPOSE 3000

USER nobody

ENTRYPOINT ["/usr/local/bin/frank-sync-agent"]
