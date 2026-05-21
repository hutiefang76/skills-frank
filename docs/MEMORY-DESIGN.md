# 分布式记忆方案设计（DESIGN.md §7.4 补充）

## TL;DR（v2 — 用户拍板版）

frank 的分布式记忆 = **自建 PostgreSQL + pgvector（你机器上 Docker）** + **mem0 Python 服务（同机）** + **frank-sync-agent (Rust) 做 MCP 协议适配层**。

- **不付 TencentDB**（节省 ~1000 元/月）
- **直接用 mem0**（不自己实现记忆抽取逻辑）
- **跨设备访问走 Tailscale**（免费、内网安全）
- **客户端跨平台不受影响**（Python 只在服务端跑）

---

## v2 部署架构

```
┌────────────────────────────────────────────┐
│ 你这一台机器 (Linux/WSL2/Mac, 任选一台常开)  │
│                                            │
│ Tailscale 网络: 100.x.x.x                  │
│  │                                         │
│  ▼                                         │
│ docker-compose:                            │
│  ├ frank-sync-agent  (Rust)   :443         │
│  │   ↓ localhost HTTP                       │
│  ├ mem0-service      (Python) :8888        │
│  │   ↓                                      │
│  └ postgres+pgvector          :5432        │
└────────────────────────────────────────────┘
              ▲       ▲       ▲
              │       │       │ HTTPS over Tailscale
              │       │       │
   ┌──────────┘  ┌────┘  └────┐
   │             │            │
┌──────┐    ┌──────┐     ┌──────┐
│ 设备A │    │ 设备B │     │ 设备C │
│ Win  │    │ Mac  │     │ Linux│
│ frank│    │ frank│     │ frank│
└──────┘    └──────┘     └──────┘
```

### 跨平台兼容澄清

- ✅ **frank-cli (Rust)**：客户端，跨 Win/Mac/Linux/ARM 全平台分发
- ❌ **mem0 (Python) + postgres**：服务端单机，固定 Linux container，不参与跨平台分发
- 用户在 Mac 上跑 `frank install ...` → 调 Tailscale 100.x.x.x → 击中 sync-agent → 内部走 localhost 到 mem0
- 整个链路 Python 对客户端**完全透明**

### 成本对比

| 方案 | 月费 | 备注 |
|---|---|---|
| ~~TencentDB PG 1核2G~~ | ~~~1000 元~~ | v1 推荐，过度设计 |
| **自建 + Tailscale** | **0 元** | v2 推荐 ⭐ |
| 自建 + Cloudflare Tunnel | 0 元 | 需绑域名 |
| 自建 + 腾讯云 CVM 2核4G | 50-80 元 | 单独 sync-agent 服务器 |

## 选型对比

### 候选 A：mem0（开源 memory layer）

- **Repo**: https://github.com/mem0ai/mem0
- **优势**：
  - 完整记忆生命周期：extract → store → retrieve → update → forget
  - 内置多种 LLM 后端（OpenAI / Anthropic / Gemini）
  - 内置多种向量库（Qdrant / Pinecone / pgvector）
  - 文档完整、有社区
- **劣势**（对 frank 而言）：
  - **Python 主导**，frank sync-agent 用 Rust 写，集成要么跨进程要么用 PyO3，复杂
  - 库重，引入大量传递依赖
  - mem0 是产品级方案，我们只要一个轻量的"跨设备记忆同步"，杀鸡用牛刀
- **借鉴价值**：⭐⭐⭐⭐（fact-extraction prompt 设计 + 记忆冲突合并策略）

### 候选 B：LangChain / LlamaIndex（RAG 库）

- **优势**：通用、生态大、链式工作流强
- **劣势**：
  - Python/JS 主导，与 Rust 后端不匹配
  - 设计目标是 RAG（检索增强生成），不是"记忆"——我们要的是带语义检索的 KV 存储，不是文档问答
  - 太重，启动慢
- **结论**：不适合 frank 场景

### 候选 C：现成 MCP memory server

- 你已装 `mcp__memory__*`，本地 SQLite（实体 + 关系图）
- **优势**：协议成熟、各 AI CLI 已对接、SQLite 简单
- **劣势**：本地 SQLite 不跨设备
- **借鉴价值**：⭐⭐⭐⭐⭐（直接用其**协议**，让 sync-agent 实现这个协议作为云端版本）

### 候选 D：自建 pgvector + 兼容 MCP 协议（推荐）

- **方案**：
  - 存储：TencentDB PostgreSQL + pgvector 扩展
  - 协议：实现 MCP memory server 协议（兼容 `mcp__memory__*` 工具）
  - 嵌入：调 OpenAI/Tencent Hunyuan embedding API（可配置）
  - 抽取逻辑：参考 mem0 的 prompt 设计，但实现在 Rust 里
- **优势**：
  - 全 Rust，与 sync-agent 同栈
  - 协议兼容 → AI 客户端零成本切换（改 MCP 配置一行）
  - pgvector 是 PostgreSQL 官方扩展，TencentDB PG 支持
  - 控制力强，能加自定义业务字段（device_id / profile）
- **劣势**：要写部分代码（但量不大，pgvector 操作只是 SQL）

## 推荐方案详细设计

### 存储 schema（pgvector）

```sql
-- 启用扩展
CREATE EXTENSION IF NOT EXISTS vector;

-- 实体表（带向量）
CREATE TABLE memories (
    id BIGSERIAL PRIMARY KEY,
    user_id     VARCHAR(64) NOT NULL,
    name        VARCHAR(256) NOT NULL,       -- 实体名 / 记忆主题
    entity_type VARCHAR(64) NOT NULL,        -- 类型 (人/项目/规则等)
    content     TEXT NOT NULL,                -- 原文记忆内容
    observations JSONB,                       -- 结构化观察
    embedding   vector(1536),                 -- OpenAI text-embedding-3-small 维度
    metadata    JSONB,                        -- profile / device / source 等
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    updated_at  TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, name)
);

-- 向量索引 (HNSW, 适合中等规模)
CREATE INDEX idx_memories_embedding
    ON memories USING hnsw (embedding vector_cosine_ops);

-- 关系表
CREATE TABLE memory_relations (
    id BIGSERIAL PRIMARY KEY,
    user_id     VARCHAR(64) NOT NULL,
    from_name   VARCHAR(256) NOT NULL,
    to_name     VARCHAR(256) NOT NULL,
    relation    VARCHAR(64) NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (user_id, from_name) REFERENCES memories(user_id, name) ON DELETE CASCADE,
    FOREIGN KEY (user_id, to_name)   REFERENCES memories(user_id, name) ON DELETE CASCADE
);
```

### MCP 协议兼容层

sync-agent 暴露 HTTP 端点实现 MCP memory server 协议：

```
POST /mcp/memory/create_entities
POST /mcp/memory/add_observations
POST /mcp/memory/create_relations
GET  /mcp/memory/read_graph
GET  /mcp/memory/search_nodes?q=<query>
GET  /mcp/memory/open_nodes?names=<list>
DELETE /mcp/memory/delete_entities
DELETE /mcp/memory/delete_relations
```

各 AI CLI 的 MCP 配置（例如 `~/.claude/mcp.json`）改成：

```json
{
  "memory": {
    "url": "https://frank-sync.your-domain.com/mcp/memory",
    "auth": "Bearer <device-cert-token>"
  }
}
```

这样 `mcp__memory__*` 工具调用透明走云端，跨设备共享。

### 记忆写入流程

```
AI 调 mcp__memory__create_entities("user prefers Rust")
    ↓
sync-agent 收到 HTTP POST
    ↓
1. 调 embedding API 生成向量
2. 检查近似已有记忆 (similarity > 0.85?)
   ├─ 是 → 合并/更新 (mem0 conflict resolution 风格)
   └─ 否 → INSERT 新记忆
3. KMS 加密 content 字段（敏感内容）
4. 返回 entity id
```

### 检索流程

```
AI 调 mcp__memory__search_nodes("Rust language preference")
    ↓
1. embedding 查询文本
2. pgvector 相似度搜索 top-K
3. KMS 解密 content
4. 返回结果
```

## 借鉴 mem0 的 prompt 工程

mem0 的核心 prompt 设计（值得抄过来）：

1. **Fact Extraction**：从对话/上下文里识别"长期有价值"的事实
2. **Memory Conflict Resolution**：新旧记忆冲突时的合并策略（保留更新 vs 覆盖 vs 标记历史）
3. **Forget Detection**：判断哪些记忆已过期/无效

这些 prompt 用 Rust 调 LLM API 实现，不依赖 mem0 库。

## 演进路径（写入 DESIGN.md §10 的 P2）

| 子任务 | 验收 |
|---|---|
| TencentDB PG 实例 + pgvector 扩展 | `\dx vector` 看到扩展 |
| memories / memory_relations 表 + 索引 | DDL 执行成功 |
| sync-agent 实现 MCP 协议 8 个端点 | curl 测试通过 |
| 客户端 `frank memory` 命令 | `frank memory add/query/list` 可用 |
| 各 AI MCP 配置切换文档 | 一台机器写、另一台读到 |
| KMS 加密 content 字段 | 数据库直接 SELECT 看到密文 |
| embedding provider 可配置（OpenAI / 腾讯 Hunyuan / Anthropic） | YAML 配置切换不改代码 |

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| embedding API 调用费用 | 缓存 query embedding；提供本地 sentence-transformers fallback（P4） |
| pgvector 单表性能 | 分用户分片；HNSW 索引参数调优；超大规模可换 Qdrant |
| 记忆冲突误合并 | 保留 history 表（INSERT-only），可回放 |
| MCP 协议变更 | sync-agent 加版本号路由，旧版本短期支持 |

## 总结一句话（v2 用户拍板版）

**自建 PG+pgvector（你机器上 Docker），mem0 Python 服务直接用（不自己写抽取逻辑），frank-sync-agent (Rust) 做 MCP 协议适配 + 加密，跨设备访问走 Tailscale，全链成本 0 元。**

---

## docker-compose.yml 雏形（待 P2 day1 落地）

```yaml
# ~/frank-self-host/docker-compose.yml
version: '3.8'

networks:
  internal:

services:
  postgres:
    image: pgvector/pgvector:pg16
    restart: unless-stopped
    networks: [internal]
    environment:
      POSTGRES_PASSWORD: ${PG_PASSWORD:?需在 .env 设置}
      POSTGRES_DB: frank
      POSTGRES_USER: frank
    volumes:
      - ./data/pg:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U frank"]
      interval: 10s

  mem0:
    image: mem0ai/mem0:latest  # 或自己 dockerfile 包一层 FastAPI
    restart: unless-stopped
    networks: [internal]
    depends_on:
      postgres: { condition: service_healthy }
    environment:
      MEM0_VECTOR_STORE: pgvector
      MEM0_PG_HOST: postgres
      MEM0_PG_USER: frank
      MEM0_PG_PASSWORD: ${PG_PASSWORD}
      OPENAI_API_KEY: ${OPENAI_API_KEY}  # 或 ANTHROPIC_API_KEY
    expose: ["8888"]   # 仅 internal 网络可访问, 不暴露宿主机

  sync-agent:
    image: hutiefang76/frank-sync-agent:latest  # 你自己构建的 Rust 镜像
    restart: unless-stopped
    networks: [internal]
    depends_on: [mem0, postgres]
    environment:
      MEM0_URL: http://mem0:8888
      PG_URL: postgres://frank:${PG_PASSWORD}@postgres/frank
      KMS_PROVIDER: local-file  # 自建用本地密钥, 不强制腾讯云 KMS
    ports:
      - "127.0.0.1:443:443"  # 绑 Tailscale, 不暴公网
    volumes:
      - ./secrets:/secrets:ro
```

`.env` 文件（gitignored）：
```bash
PG_PASSWORD=<强密码>
OPENAI_API_KEY=<for mem0>
```

启动：
```bash
cd ~/frank-self-host
docker compose up -d
docker compose logs -f sync-agent
```

## Tailscale 接入步骤

```bash
# 1. 安装 Tailscale 三台设备 (各自 1 行)
# Linux:  curl -fsSL https://tailscale.com/install.sh | sh && sudo tailscale up
# macOS:  brew install tailscale && sudo tailscale up
# Win:    下载 .exe → 安装 → 登录

# 2. 找到 sync-agent 主机的 Tailscale IP
tailscale ip -4  # 例如 100.96.12.34

# 3. 其他设备 frank 配置指向
echo "sync_url: https://100.96.12.34" > ~/.frank/sync.yaml
```

## 降级与离线 (R3 风险缓解)

如果你这台机器关了，其他设备的 frank 会出现：
- ❌ `frank memory query`：超时报错，降级到本地 SQLite 缓存
- ❌ `frank sync push`：失败，本地变更存 outbox，下次连上批量上报
- ✅ 其他命令（install/list/enable/disable）：正常，本地有 manifest 缓存

详见 DESIGN.md §11 风险 R3。
