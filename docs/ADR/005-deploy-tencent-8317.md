# ADR-005: 腾讯云 :8317 部署拓扑

| Field | Value |
|---|---|
| **Status** | Accepted (qdrant 部分进行中) |
| **Date** | 2026-05-21 |
| **Decider** | hutiefang |
| **Host** | tx (101.35.227.232) — Ubuntu 22.04, 4 vCPU, 3.3 GB RAM, 59 GB disk |

## 现状摸底 (2026-05-21)

```
RAM:    3.3G total, 2.0G used, 1.0G available, swap 2.0G 已用满
DISK:   /  59G total, 49G used (86%!), 8.2G free  ⚠️ 紧
DOCKER: 29.2.1 OK
端口 8317: 未占用 ✅
```

**已跑容器** (9 个):
| 容器 | 镜像 | 关键端口 | 评估 |
|---|---|---|---|
| 1Panel-openresty | openresty | 80/443 | 保留 (web 入口) |
| 1Panel-mysql | mysql:8.4 | 3306 | 保留 (1Panel 自用) |
| 1Panel-redis | redis:8.4 | 6379 | 保留 |
| 1Panel-rustfs | rustfs | 9000 | 评估: 是否真用? S3 兼容存储 |
| napcat | napcat | 3000-3001, 6099 | QQ Bot, 用户在用? |
| antigravity-manager | antigravity | 6080, 8045, 19527 | 用户工具 |
| myai-signaling | custom | 9001 | 旧 myai 项目? |
| gradient-demo | nginx:alpine | 12011 | 演示页, 可停 |
| vigilant_greider | (匿名) | 12013 | 不明, 36h up |

**资源决策**: 不强停任何容器, 给 frank 自己留 ~600 MB 预算 (qdrant ~200 + sync-agent ~50 + 后续 orchestrator ~200 + Postgres ~150)。若实际超, **先停 `gradient-demo` (演示页) + `vigilant_greider` (不明)**, 收回 ~50 MB。

## 部署拓扑

```
┌─────────────────────────────────────────────────────────┐
│  腾讯云 VM  (tx, 101.35.227.232)                         │
│                                                         │
│  既有: 1Panel (openresty :80/:443)                       │
│                                                         │
│  ─── 新增 frank stack (port 8317 暴露) ───              │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Caddy (容器内 :8317)                             │    │
│  │  - TLS 终止 (自签 / 后续 LE)                       │    │
│  │  - 路由:                                          │    │
│  │    /memory/*       → sync-agent:3000             │    │
│  │    /orchestrator/* → sync-agent:3000             │    │
│  │    /qdrant/*       → qdrant:6333 (管理界面)        │    │
│  │    /ui             → 静态文件 (orchestrator web)  │    │
│  └─────────────────────────────────────────────────┘    │
│        │                                                │
│        ├──────────┬──────────────────┬─────────────┐    │
│        ↓          ↓                  ↓             ↓    │
│   ┌────────┐ ┌──────────────┐  ┌───────────┐  ┌────┐   │
│   │ qdrant │ │ sync-agent   │  │ orchestrator│ │ pg │   │
│   │ :6333  │ │ (Rust axum)  │  │ web 静态    │ │ alp│   │
│   │ :6334  │ │ 内部 :3000   │  │ 单页 SPA   │ │ pn │   │
│   │ vec DB │ │ /memory api │  │             │ │ e  │   │
│   │        │ │ /orch  api │  │             │ │    │   │
│   └────────┘ └──────────────┘  └───────────┘  └────┘   │
│                                                         │
└─────────────────────────────────────────────────────────┘
            ↑ HTTPS :8317
            │ (本机 frank-cli + 浏览器 都走这里)
```

## 容器配置 (docker-compose.yml 草案)

```yaml
# deploy/docker-compose.yml
version: "3.9"

networks:
  frank-net:
    driver: bridge

volumes:
  qdrant-data:
  postgres-data:
  caddy-data:
  caddy-config:

services:
  caddy:
    image: caddy:2.10-alpine
    restart: unless-stopped
    ports:
      - "8317:8317"   # 唯一对外
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
      - caddy-config:/config
      - ./web:/srv/web:ro    # orchestrator 静态前端
    networks: [frank-net]
    depends_on: [sync-agent, qdrant]

  qdrant:
    image: qdrant/qdrant:v1.13.0
    restart: unless-stopped
    # 不暴露公网, 只走 caddy 反代
    expose: ["6333", "6334"]
    volumes:
      - qdrant-data:/qdrant/storage
    environment:
      QDRANT__SERVICE__GRPC_PORT: 6334
    networks: [frank-net]
    mem_limit: 256m

  postgres:
    image: postgres:17-alpine
    restart: unless-stopped
    expose: ["5432"]
    environment:
      POSTGRES_USER: frank
      POSTGRES_PASSWORD_FILE: /run/secrets/pg_password
      POSTGRES_DB: frank_orchestrator
    volumes:
      - postgres-data:/var/lib/postgresql/data
    secrets: [pg_password]
    networks: [frank-net]
    mem_limit: 200m

  sync-agent:
    image: frank-sync-agent:latest
    restart: unless-stopped
    expose: ["3000"]
    environment:
      FRANK_QDRANT_URL: http://qdrant:6334
      FRANK_POSTGRES_URL: postgresql://frank:@postgres/frank_orchestrator
      FRANK_OPENAI_API_KEY_FILE: /run/secrets/openai_key
      FRANK_ANTHROPIC_API_KEY_FILE: /run/secrets/anthropic_key
      RUST_LOG: info
    secrets: [pg_password, openai_key, anthropic_key]
    networks: [frank-net]
    depends_on: [qdrant, postgres]
    mem_limit: 128m

secrets:
  pg_password:
    file: ./.secrets/pg_password
  openai_key:
    file: ./.secrets/openai_key
  anthropic_key:
    file: ./.secrets/anthropic_key
```

## Caddyfile (草案)

```caddy
:8317 {
  tls internal  # 先自签; 有域名后改 LE

  encode gzip zstd

  # frank-sync-agent REST + WS
  reverse_proxy /memory/* sync-agent:3000
  reverse_proxy /orchestrator/* sync-agent:3000
  reverse_proxy /api/* sync-agent:3000

  # 静态前端
  handle /ui/* {
    root * /srv/web
    try_files {path} /index.html
    file_server
  }
  handle / {
    redir /ui/ 302
  }

  # Qdrant 管理界面 (可选, 调试用; 上线后用 IP 白名单关掉)
  reverse_proxy /qdrant/* qdrant:6333 {
    rewrite /qdrant{uri}
  }

  log {
    output stdout
    format console
  }
}
```

## 部署步骤 (脚本化)

```bash
# 1. ssh tx
ssh tx

# 2. 在 home 下建工作目录
sudo mkdir -p /opt/frank /opt/frank/.secrets
sudo chown ubuntu:ubuntu /opt/frank
cd /opt/frank

# 3. 拷贝 compose / Caddyfile / 前端静态
# (从本机 rsync)

# 4. 写 secrets (一次性, 含 OpenAI key / Anthropic key / pg 密码)
echo "$OPENAI_KEY" > .secrets/openai_key
echo "$ANTHROPIC_KEY" > .secrets/anthropic_key
openssl rand -hex 32 > .secrets/pg_password
chmod 600 .secrets/*

# 5. 拉镜像 + 启
docker compose up -d qdrant      # 先起 qdrant 单点测
# 验证 qdrant 健康
curl -s http://localhost:6333/healthz
docker compose up -d caddy        # 起 caddy
# 验证外网可达
curl -sk https://101.35.227.232:8317/qdrant/healthz
```

## 防火墙

腾讯云控制台需放行 TCP **8317**:
- Source: 0.0.0.0/0 (或更严: 仅个人公网 IP)
- 端口: 8317
- 协议: TCP

## 资源监控

`docker stats` 每天看; 若 sync-agent / qdrant 内存上升明显:
- qdrant 调 `search.config.hnsw_config.m=8` (默认 16, 减半省内存)
- Postgres `shared_buffers = 64MB` (默认 128, 减半)

## 风险

| ID | 风险 | 对策 |
|---|---|---|
| R-D1 | 磁盘 49/59 GB 见底 | 部署前 `docker system prune -af` 清旧镜像; 长远迁数据卷到 /data (若挂了大盘) |
| R-D2 | 公网暴露 :8317 被扫 | Caddy 加 `@blocked not header X-Frank-Token "xxx" respond 401`, 给本机加 token; 或彻底 IP 白名单 |
| R-D3 | TLS 自签浏览器警告 | 给 tx 配域名 → LE 自动签 |
| R-D4 | docker compose 重启 ↻ 数据丢 | volume 已配置, 实测确认 |
| R-D5 | 1Panel 抢 80/443 与 caddy 冲突 | caddy 只听 :8317, 不抢 :80/:443; OK |

## 不在范围

- ❌ Kubernetes / Nomad (3.3G VM 跑 k8s 自杀)
- ❌ mTLS 设备证书 (P2 完整设计里有, 当前简化为 token + IP 白名单)
- ❌ 自动备份 / 异地容灾 (P3 一起做)

## 后续动作

- [ ] deploy/docker-compose.yml + Caddyfile 写完
- [ ] deploy/.secrets.example 模板 (不入 git)
- [ ] tx 上拉 qdrant 跑通 (本 PR 完成)
- [ ] frank-sync-agent v0.1 (axum hello-world)
- [ ] 域名 + LE 替换自签 (待用户决定是否要)
