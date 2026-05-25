# frank-stack 服务端部署

服务端栈用 Docker Compose 编排. 项目作者本人跑在腾讯云 VM `tx:8318` (=外网公共 demo
server `frank.hutiefang.com`). 任何用户也可在自己机器一键自建.

## 一键自建 (v0.10.10 新, Linux only)

**最快路径** — 一行命令搞定 (Mac/Windows 后续支持):

```bash
curl -sSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/install-server.sh | bash
```

脚本做的事:

1. 检查 docker / docker compose
2. 创建 `/opt/frank` (可用 `FRANK_HOME=/data/frank` 覆盖)
3. 下载 `docker-compose.yml` + `Caddyfile` + `scripts/init-models.sh`
4. **下 BGE-small ONNX 模型 (~250MB) 到 `/opt/frank/models`** — 一次性
5. 生成随机 API token, 写到 `/opt/frank/.secrets/api_token`
6. `docker compose up -d` 起 qdrant + sync-agent + caddy
7. 输出客户端配置命令

完成后客户端配:

```bash
frank config set sync.agent_url http://<your-host>:8318
frank login --token <api-token>      # token 在 /opt/frank/.secrets/api_token
```

## 为什么模型要单独下

v0.10.10 把 ONNX 模型从 docker image 移到 host volume:

| | v0.10.9 及之前 | v0.10.10 起 |
|---|---|---|
| 镜像大小 | 572MB | ~80MB |
| 首次部署 | docker pull 5-30min (国内常 timeout) | docker pull 30s + 下模型 2-10min |
| 升级 | 每次 pull 572MB | 每次 pull 80MB |
| CI build | 8min (含模型预下) | 3min |

模型从 `hf-mirror.com` (HuggingFace 国内镜像) 下, 解决国内连 HF 不稳的问题.

## 手动自建 (不用脚本)

```bash
# 1. 准备目录 (Linux/Mac)
sudo mkdir -p /opt/frank && sudo chown -R $(id -u):$(id -g) /opt/frank
cd /opt/frank

# 2. 下配置文件
curl -O https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/docker-compose.yml
curl -O https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/Caddyfile
mkdir -p scripts && curl -o scripts/init-models.sh \
    https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/scripts/init-models.sh
chmod +x scripts/init-models.sh

# 3. 下模型 (一次性, ~250MB)
bash scripts/init-models.sh

# 4. 生成 API token + .env
mkdir -p .secrets
head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=' > .secrets/api_token
chmod 600 .secrets/api_token
echo "FRANK_API_TOKEN=$(cat .secrets/api_token)" > .env

# 5. 启动
docker compose up -d
```

---

## 项目作者的 tx 部署 (历史)

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
