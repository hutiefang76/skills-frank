#!/usr/bin/env bash
# v0.8: 部署新 sync-agent 镜像到 tx (从 mock → real LocalEmbedder 模式).
#
# 流程:
#   1. 本机 docker buildx 必须**已经跑完**, 产物 /tmp/frank-sync-agent.tar 在
#   2. scp tar 到 tx:/opt/frank/
#   3. tx docker load 进来
#   4. scp 新版 docker-compose.yml (qdrant v1.18 + sync-agent v0.8.0 + mock=0 + HF mirror)
#   5. docker compose up -d (重启 qdrant + sync-agent)
#   6. 等 90s 让 fastembed 下载 BGE 模型 + 建 collection
#   7. curl healthz + 简单 add/search 验证
#
# 用法:
#   bash deploy/scripts/deploy-tx-sync-agent.sh [TAR_PATH]
#
# 默认 TAR_PATH=/tmp/frank-sync-agent.tar
# 默认 TX_HOST=tx (要求 ~/.ssh/config 配好 Host tx)

set -euo pipefail

TX_HOST="${TX_HOST:-tx}"
TAR="${1:-/tmp/frank-sync-agent.tar}"

if [[ ! -f "$TAR" ]]; then
    echo "[ERROR] tar not found: $TAR" >&2
    echo "  先跑: docker buildx build --platform linux/amd64 \\" >&2
    echo "          -f deploy/sync-agent.Dockerfile -t frank-sync-agent:0.8.0 \\" >&2
    echo "          --output type=docker,dest=$TAR ." >&2
    exit 1
fi

TAR_SIZE_MB=$(du -m "$TAR" | awk '{print $1}')
echo "============================================================"
echo "deploy-tx-sync-agent.sh"
echo "  TX_HOST = $TX_HOST"
echo "  TAR     = $TAR ($TAR_SIZE_MB MB)"
echo "============================================================"

echo ""
echo "[1/6] scp $TAR → $TX_HOST:/opt/frank/frank-sync-agent.tar ..."
scp "$TAR" "$TX_HOST:/opt/frank/frank-sync-agent.tar"

echo ""
echo "[2/6] docker load on $TX_HOST ..."
ssh "$TX_HOST" "cd /opt/frank && docker load -i frank-sync-agent.tar && rm frank-sync-agent.tar"

echo ""
echo "[3/6] scp deploy/docker-compose.yml + deploy/Caddyfile → $TX_HOST:/opt/frank/ ..."
# 备份 tx 上现有 yml (回滚用)
ssh "$TX_HOST" "test -f /opt/frank/docker-compose.yml && cp /opt/frank/docker-compose.yml /opt/frank/docker-compose.yml.bak.\$(date +%Y%m%d-%H%M%S) || true"
scp deploy/docker-compose.yml "$TX_HOST:/opt/frank/docker-compose.yml"
scp deploy/Caddyfile "$TX_HOST:/opt/frank/Caddyfile" 2>/dev/null || true

echo ""
echo "[4/6] docker compose pull qdrant (v1.18.0) + recreate sync-agent ..."
ssh "$TX_HOST" "cd /opt/frank && docker compose pull qdrant && docker compose up -d --force-recreate qdrant sync-agent"

echo ""
echo "[5/6] Wait 90s for fastembed BGE 模型下载 + collection init (首次可能 1-2 分钟)..."
for i in $(seq 1 9); do
    sleep 10
    status=$(ssh "$TX_HOST" "docker inspect frank-sync-agent --format '{{.State.Status}} {{.State.Health.Status}}' 2>/dev/null || echo 'unknown'")
    echo "  ${i}0s: $status"
done

echo ""
echo "[6/6] Verify:"
echo "  --- container logs (tail 20) ---"
ssh "$TX_HOST" "docker logs frank-sync-agent --tail 20 2>&1"
echo ""
echo "  --- healthz ---"
if curl -sf https://frank.hutiefang.com/healthz; then
    echo " ✓ frank.hutiefang.com/healthz reachable"
else
    echo " ✗ healthz FAIL — 看上面的日志找错"
    exit 2
fi
echo ""
echo "============================================================"
echo "deploy 完成. 端到端测:"
echo "  frank memory add-raw \"test fact $(date +%s)\" --user test"
echo "  frank memory search test --user test --limit 5"
echo "============================================================"
