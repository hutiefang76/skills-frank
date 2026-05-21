# frank-stack 服务端部署 (tx:8317)

服务端栈用 Docker Compose 编排, 跑在腾讯云 VM `tx` 上, 唯一对外端口 `8317`。

## 当前架构

```
┌─────────────────────────────────────────┐
│ tx (101.35.227.232)                     │
│                                         │
│  ┌──────┐    ┌──────────┐               │
│  │ caddy│←───│  qdrant  │               │
│  │:8317 │    │  :6333   │               │
│  └──────┘    │  :6334   │               │
│              └──────────┘               │
│                                         │
│  (sync-agent + postgres 待上)            │
└─────────────────────────────────────────┘
            ↑ HTTPS:8317
            │
       本机 frank-cli + 浏览器
```

详见 `docs/ADR/005-deploy-tencent-8317.md`。

## 首次部署

```bash
# 0. 本机: rsync 整个 deploy/ 到 tx:/opt/frank
rsync -avh --delete deploy/ tx:/opt/frank/

# 1. tx 侧: 进 /opt/frank, 起 qdrant
ssh tx
cd /opt/frank
docker compose pull qdrant caddy
docker compose up -d qdrant
sleep 5 && curl -sf http://localhost:6333/healthz && echo "qdrant OK"

# 2. 起 caddy
docker compose up -d caddy
sleep 2 && curl -sfk https://localhost:8317/healthz && echo "caddy OK"

# 3. 外网验证 (本机)
curl -sfk https://101.35.227.232:8317/healthz
curl -sfk https://101.35.227.232:8317/qdrant/healthz
```

## 防火墙

腾讯云控制台 → 安全组 → 入站规则:

```
协议: TCP
端口: 8317
源:   你的本机公网 IP/32   (推荐) 或 0.0.0.0/0 (粗放)
策略: 允许
```

## 日常运维

```bash
# 看日志
ssh tx 'cd /opt/frank && docker compose logs -f --tail=100'

# 重启
ssh tx 'cd /opt/frank && docker compose restart qdrant'

# 看资源
ssh tx 'docker stats --no-stream frank-caddy frank-qdrant'

# 升 qdrant 版本: 改 docker-compose.yml 的 tag, 然后
ssh tx 'cd /opt/frank && docker compose pull qdrant && docker compose up -d qdrant'

# 数据备份: qdrant snapshot API (后期接入)
# 暂时手动: docker exec frank-qdrant qdrant snapshot create frank_memories
```

## 拆除

```bash
ssh tx 'cd /opt/frank && docker compose down -v'   # 注意 -v 会删 volume!
```

## 后续 (待 sync-agent 上线)

1. 取消 Caddyfile 里 `/memory/*` `/orchestrator/*` 两行注释
2. 在 docker-compose.yml 加 `sync-agent` 服务 (frank-sync-agent docker 镜像)
3. 加 `postgres` 服务 (orchestrator job 表)
4. `docker compose up -d sync-agent postgres`
