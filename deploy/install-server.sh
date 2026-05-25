#!/usr/bin/env bash
#
# install-server.sh — frank 服务端 (sync-agent + qdrant + caddy) 一键自建.
#
# 用法 (Linux VPS):
#   curl -sSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/install-server.sh | bash
#
# 或本机下好 repo:
#   bash deploy/install-server.sh
#
# 做的事:
#   1. 检查 docker / docker compose 装好
#   2. 创建 /opt/frank (可改 FRANK_HOME)
#   3. 下 docker-compose.yml + Caddyfile + init-models.sh
#   4. 跑 init-models.sh 下 BGE-small 模型 (~250MB, 一次性)
#   5. 生成 .env (API_TOKEN 随机)
#   6. docker compose up -d
#   7. 健康检查 + 提示客户端配置命令
#
# 完成后:
#   - sync-agent 跑在 0.0.0.0:8318 (caddy 反代)
#   - 客户端配: frank config set sync.agent_url http://<this-host>:8318
#   - API token 在 /opt/frank/.secrets/api_token
#
# v0.10.10 新增. Linux only (Mac/Windows 后续版本).

set -euo pipefail

# ---- 配置 ----
FRANK_HOME="${FRANK_HOME:-/opt/frank}"
REPO_RAW="${REPO_RAW:-https://raw.githubusercontent.com/hutiefang76/skills-frank/main}"

# ---- 颜色输出 (TTY only) ----
if [ -t 1 ]; then
    G='\033[0;32m'; Y='\033[0;33m'; R='\033[0;31m'; N='\033[0m'
else
    G=''; Y=''; R=''; N=''
fi
ok()   { echo -e "${G}✓${N} $*"; }
warn() { echo -e "${Y}⚠${N} $*"; }
err()  { echo -e "${R}✗${N} $*" >&2; }
step() { echo -e "\n${Y}==>${N} $*"; }

# ---- 1. 检查依赖 ----
step "1/7 检查 docker"
if ! command -v docker >/dev/null 2>&1; then
    err "找不到 docker. 请先装: https://docs.docker.com/engine/install/"
    exit 1
fi
ok "docker $(docker --version | awk '{print $3}' | tr -d ',')"

if ! docker compose version >/dev/null 2>&1; then
    err "找不到 docker compose v2. 请装 docker-compose-plugin."
    exit 1
fi
ok "docker compose $(docker compose version --short)"

# 操作系统
if [ "$(uname -s)" != "Linux" ]; then
    warn "当前是 $(uname -s), 脚本只测过 Linux. 继续? [y/N]"
    read -r reply
    [ "$reply" = "y" ] || [ "$reply" = "Y" ] || exit 1
fi

# ---- 2. 创建目录 ----
step "2/7 创建 $FRANK_HOME"
if [ ! -d "$FRANK_HOME" ]; then
    sudo mkdir -p "$FRANK_HOME"
    sudo chown -R "$(id -u):$(id -g)" "$FRANK_HOME"
fi
mkdir -p "$FRANK_HOME/scripts" "$FRANK_HOME/models" "$FRANK_HOME/.secrets"
ok "$FRANK_HOME 就绪"

# ---- 3. 下配置文件 ----
step "3/7 下载 docker-compose.yml + Caddyfile + init-models.sh"
cd "$FRANK_HOME"
for f in docker-compose.yml Caddyfile; do
    if [ -f "$f" ]; then
        warn "$f 已存在, 跳过. (强制覆盖: rm $FRANK_HOME/$f)"
    else
        curl -sSLf "$REPO_RAW/deploy/$f" -o "$f"
        ok "下好 $f"
    fi
done
if [ ! -f scripts/init-models.sh ]; then
    curl -sSLf "$REPO_RAW/deploy/scripts/init-models.sh" -o scripts/init-models.sh
    chmod +x scripts/init-models.sh
    ok "下好 scripts/init-models.sh"
fi

# ---- 4. 下模型 ----
step "4/7 下 BGE-small 模型 (~250MB, 视网速 2-10 分钟)"
MODEL_DIR="$FRANK_HOME/models" bash scripts/init-models.sh

# ---- 5. 生成 API token ----
step "5/7 生成 API token"
if [ ! -f .secrets/api_token ]; then
    # 32 字节 base64url, ~43 chars
    head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=' > .secrets/api_token
    chmod 600 .secrets/api_token
    ok "新 token 写到 $FRANK_HOME/.secrets/api_token"
else
    ok "token 已存在, 复用"
fi
API_TOKEN=$(cat .secrets/api_token)

# 写 .env (compose 自动读)
cat > .env <<EOF
# v0.10.10 自动生成. 改 token 后重启: docker compose restart caddy
FRANK_API_TOKEN=$API_TOKEN
EOF
chmod 600 .env
ok ".env 写好"

# ---- 6. 启动 ----
step "6/7 docker compose up -d"
docker compose pull
docker compose up -d
ok "服务已启动"

# 等 qdrant healthy
echo -n "    等 qdrant healthy "
for i in $(seq 1 30); do
    if docker compose ps qdrant --format json 2>/dev/null | grep -q '"Health":"healthy"'; then
        echo " 好了"
        break
    fi
    echo -n "."
    sleep 2
done

# ---- 7. 验证 ----
step "7/7 健康检查"
sleep 2  # caddy 起来一下
if curl -sf "http://localhost:8318/healthz" >/dev/null; then
    ok "caddy:8318 → sync-agent:3000 通了"
else
    warn "caddy:8318 还没通, 等 30s 再 curl: curl -sf http://localhost:8318/healthz"
    warn "或查日志: cd $FRANK_HOME && docker compose logs --tail=50"
fi

# ---- 完成 ----
HOST_IP=$(hostname -I 2>/dev/null | awk '{print $1}' || hostname)
cat <<EOF

${G}========================================${N}
${G}  frank server 装好了!${N}
${G}========================================${N}

服务地址:
  http://$HOST_IP:8318     (内网)
  https://你的域名:8318     (要先配 DNS + caddy TLS)

API token (保管好, 不要 git):
  $API_TOKEN

客户端配置 (在装了 frank cli 的机器上跑):
  ${Y}frank config set sync.agent_url http://$HOST_IP:8318${N}
  ${Y}frank login --token $API_TOKEN${N}

或临时 env (单次):
  FRANK_SYNC_AGENT_URL=http://$HOST_IP:8318 frank memory list

服务管理:
  cd $FRANK_HOME
  docker compose ps           # 看状态
  docker compose logs -f      # 看日志
  docker compose restart      # 重启
  docker compose down         # 停 (数据保留)

EOF
