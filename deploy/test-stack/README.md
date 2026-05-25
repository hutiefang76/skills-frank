# frank test-stack — 本机起 frank-official skill 的依赖中间件

给开发者 / 想真试一下 `frank-official` skill 的用户用 — `docker compose up -d` 起本机 demo 服务, 然后 `frank install <skill>` + 跑 SKILL.md 教的命令, 端到端验证一遍。

跟 `deploy/docker-compose.yml`(部署到 tx 服务器的生产 stack)无关。

## 前提

- Docker Desktop / OrbStack / colima (Mac) 或 docker engine (Linux/Windows)
- 端口空闲: 8848 (nacos), 9848 (nacos gRPC), 10000 (streampark), 3306 (mysql)
- 内存预算: 默认 ~512MB (只 nacos); 加 streampark profile ~2GB

## 快速验证 nacos-ops

```bash
# 1. 起 nacos
docker compose -f deploy/test-stack/docker-compose.yml up -d
# 等 healthcheck 通过 (大约 1-2 分钟)
docker compose -f deploy/test-stack/docker-compose.yml ps

# 2. 浏览器开 http://localhost:8848/nacos , 账号 nacos/nacos
#    在 "配置管理 → 配置列表" 手动建一个测试配置:
#    Data ID: realtime-job.yaml
#    Group:   DEFAULT_GROUP
#    Content: foo: bar

# 3. 装 nacos-ops skill
frank install nacos-ops

# 4. 配置 + 跑命令
cd ~/.claude/skills/nacos-ops
bash setup.sh
# config.ini 默认就是 localhost:8848 nacos/nacos, 直接能跑
.venv/bin/python nacos_config.py list --env local
# 期望输出:
# Configs in namespace 'local' (1 items):
#   [DEFAULT_GROUP] realtime-job.yaml  (8 bytes)

.venv/bin/python nacos_config.py fetch --env local --data-id realtime-job.yaml
# 期望输出: foo: bar
```

## 验证 streampark-ops (额外 ~1.5GB)

```bash
# 1. 起 streampark (额外起 mysql + streampark 容器)
docker compose -f deploy/test-stack/docker-compose.yml --profile streampark up -d
# 等 streampark healthcheck (~90s)
docker compose -f deploy/test-stack/docker-compose.yml ps

# 2. 浏览器开 http://localhost:10000 , 账号 admin/streampark
#    (默认状态下没有 Flink 应用; 测试时可手动建一个 demo flink-test 应用占位)

# 3. 装 streampark-ops skill
frank install streampark-ops

# 4. 配置 + 跑命令
cd ~/.claude/skills/streampark-ops
bash setup.sh
.venv/bin/python scripts/sp_apps_list.py --env local
# 期望输出: env=local team_id=100000 count=0   (或你手动建的应用)
```

## 让 claude 真测一次 (端到端)

```bash
# 装好 nacos-ops 后, 直接在 claude code 里:
claude "用 nacos-ops skill 列一下 local 环境的所有配置"
# claude 会读 SKILL.md, 拼出 bash 命令, 真打到 docker 起的 nacos
```

## 清理

```bash
# 停服务保留数据
docker compose -f deploy/test-stack/docker-compose.yml down

# 停服务 + 删卷 (清空 demo 数据)
docker compose -f deploy/test-stack/docker-compose.yml --profile streampark down -v
```

## 资源占用

| 服务 | 镜像 | 内存上限 | 启动时间 |
|------|------|----------|---------|
| nacos | `nacos/nacos-server:v2.3.2-slim` | 512MB | ~60s |
| mysql | `mysql:8.0` | 512MB | ~20s |
| streampark | `apache/streampark:2.1.4` | 1GB | ~90s |

只用 nacos: ~512MB。+streampark profile: ~2GB。

## 已知问题

- StreamPark 2.1.4 镜像首次启动会跑 SQL 初始化, 慢 (~90s)。`healthcheck` 阶段不要中断
- `sp_deploy_batch.py` 的默认值面向 KDWL 内部 (Flink 镜像走内部 Harbor registry), demo 跑会失败 — 这命令不适合 demo, 只 list/show 能 demo
- 如果你 docker desktop 没装/没起, `frank doctor` 会标 docker 缺失 — v0.8+ 实现
