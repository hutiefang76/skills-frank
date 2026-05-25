#!/usr/bin/env bash
#
# init-models.sh — 一次性把 fastembed BGE-small-en-v1.5 模型下到 host 路径.
# 之后 docker-compose 把这个目录挂进 frank-sync-agent 容器.
#
# 用法 (在服务器上跑一次, 之后不用):
#   bash deploy/scripts/init-models.sh                    # 默认下到 /opt/frank/models
#   MODEL_DIR=/data/frank/models bash ...init-models.sh   # 自定义路径
#
# 为啥不让 sync-agent 自己下:
# - tx (国内) 连 huggingface.co 经常 timeout
# - 这里走 HF_ENDPOINT=https://hf-mirror.com (社区维护的 HF 国内镜像)
# - 用临时 docker 容器跑 python hf-cli, 避免在 host 装 python 污染环境
#
# v0.10.10: 从原方案 (CI 预下 + COPY 进镜像) 改为 host 挂载, 镜像 572MB → 80MB.

set -euo pipefail

MODEL_DIR="${MODEL_DIR:-/opt/frank/models}"
MODEL_REPO="${MODEL_REPO:-Xenova/bge-small-en-v1.5}"
HF_ENDPOINT="${HF_ENDPOINT:-https://hf-mirror.com}"

echo "==> init-models.sh"
echo "    模型仓库: $MODEL_REPO"
echo "    下载到:   $MODEL_DIR"
echo "    HF 镜像:  $HF_ENDPOINT"
echo ""

# 检查 docker 可用
if ! command -v docker >/dev/null 2>&1; then
    echo "ERROR: 找不到 docker 命令. 请先装 docker." >&2
    exit 1
fi

# 检查权限 — /opt 通常需要 sudo, 但用户可能用自定义路径
if [ ! -d "$MODEL_DIR" ]; then
    echo "==> 创建目录 $MODEL_DIR"
    if ! mkdir -p "$MODEL_DIR" 2>/dev/null; then
        echo "    需要 sudo:"
        sudo mkdir -p "$MODEL_DIR"
        # 给当前用户 + nobody (容器内 uid 65534) 可写
        sudo chown -R "$(id -u):$(id -g)" "$MODEL_DIR"
    fi
fi

# 已下载过? (snapshots 目录非空就跳过)
SNAPSHOT_GLOB="$MODEL_DIR/hub/models--${MODEL_REPO//\//--}/snapshots"
if [ -d "$SNAPSHOT_GLOB" ] && [ "$(ls -A "$SNAPSHOT_GLOB" 2>/dev/null)" ]; then
    echo "==> 检测到已下载, 跳过. (强制重下: rm -rf $MODEL_DIR/hub && 重跑)"
    du -sh "$MODEL_DIR"
    exit 0
fi

# 跑临时容器下模型
# - 用 python:3.12-slim (~50MB), 跑完 --rm 自动删
# - HF_HOME 控制 cache 位置, 直接挂到 host 的 MODEL_DIR
# - retry 3 次防国内网络抖动
echo "==> 启动临时 python 容器下模型 (~250MB, 视网速 2-10 分钟)..."
docker run --rm \
    -e HF_ENDPOINT="$HF_ENDPOINT" \
    -e HF_HOME=/cache \
    -v "$MODEL_DIR:/cache" \
    --user "$(id -u):$(id -g)" \
    python:3.12-slim \
    bash -c "
        set -e
        pip install --quiet --no-cache-dir 'huggingface_hub[cli]>=0.24'
        for i in 1 2 3; do
            echo '--- attempt' \$i '---'
            hf download '$MODEL_REPO' && break || sleep 5
        done
    "

echo ""
echo "==> 完成. 模型大小:"
du -sh "$MODEL_DIR"
echo ""
echo "==> 验证 layout:"
ls -la "$MODEL_DIR/hub/models--${MODEL_REPO//\//--}/snapshots/"*/
echo ""
echo "==> 下一步: docker compose up -d (compose 已配 sync-agent-models volume)"
