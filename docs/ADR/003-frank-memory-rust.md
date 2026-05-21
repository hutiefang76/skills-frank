# ADR-003: frank-memory — mem0 同思路的 Rust 重写

| Field | Value |
|---|---|
| **Status** | Accepted (设计, 实现进行中) |
| **Date** | 2026-05-21 |
| **Decider** | hutiefang |
| **Replaces** | docs/MEMORY-DESIGN.md v2 (那里用 mem0 Python 服务 + Rust MCP 适配; 现改为纯 Rust) |

## 背景

frank 目标用户跨多设备 + 多 AI provider 工作 (Claude Code / codex / opencode), 强烈需要 "AI 跨会话 / 跨设备 / 跨 provider 记住事" 的能力。

最成熟的方案是 [mem0](https://github.com/mem0ai/mem0) (Python), 但:
- 是 Python 服务, 引入新运行时
- frank 主栈是 Rust (ADR-001), Rust 客户端调 Python 服务 = 多一跳 + Python 容器要 ~500MB 内存
- 腾讯云 VM 只有 3.3G RAM, 80% 已被占用 (见部署调研)

## 候选

| 方案 | 工作量 | 内存占用 | 维护成本 | 一致性 |
|---|---|---|---|---|
| A. 调 mem0 Python 服务 (HTTP) | 0.5 周 | +500 MB (Python) | 高 (Python deps 漂移) | 与 upstream 1:1 |
| **B. Rust 手写 mem0-同思路 算法重表述** | 1.5 周 | +50 MB (Rust 二进制) | 低 (纯 Rust) | 算法等价, 接口可定制 |
| C. 完整 mem0 Rust port | 2-4 周 | +50 MB | 中 (跟 upstream 同步) | 1:1 等价 |

## 决策

**采用方案 B**: Rust 手写, 抓 mem0 的核心算法骨架, 不强行 1:1 复刻。

### 抓住的算法骨架 (mem0 的核心抽象)

```
用户对话 / 事件
    ↓
[Fact Extractor]    用 LLM 抽取 "事实声明" (Subject-Verb-Object 类似 triple)
    ↓
[Embedder]          每条 fact 算 dense vector
    ↓
[Vector Store]      存 (id, embedding, fact, metadata) → Qdrant
    ↓ ............ 检索时 .............
查询 query
    ↓
[Embedder]          query → vector
    ↓
[Retriever]         top-k 相似 + 阈值过滤 + 元数据筛 (user / agent / session)
    ↓
返回 Vec<MemoryMatch>
```

去掉的部分 (mem0 有, frank-memory v1 不要):
- ❌ Graph store (Neo4j) — 关系图; 等真有需求再加
- ❌ Memory versioning history — 改记录前/后; 太重, mem0 v2 也是后加的
- ❌ Active multi-LLM provider routing — 写 Anthropic 单一 LLM 抽取即可
- ❌ Streaming embedding — 批 embed 已够用

保留的部分:
- ✅ User / Agent / Session 三级 scope
- ✅ Metadata 任意 JSON
- ✅ Top-k 相似检索 + score 阈值
- ✅ CRUD (add / search / get / update / delete)

## 模块切分

```
crates/frank-memory/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 库导出
│   ├── memory.rs           # Memory / MemoryMatch struct + Scope (user/agent/session)
│   ├── store/              # 存储抽象
│   │   ├── mod.rs          # MemoryStore trait
│   │   └── qdrant.rs       # Qdrant 实现 (qdrant-client 官方 Rust SDK)
│   ├── embed/              # 向量化
│   │   ├── mod.rs          # Embedder trait
│   │   └── openai.rs       # OpenAI text-embedding-3-small (1536 dim, 便宜)
│   ├── extract/            # 事实提取
│   │   ├── mod.rs          # FactExtractor trait
│   │   └── claude.rs       # Anthropic Claude 抽取实现 (用 claude-haiku 省钱)
│   └── client.rs           # Memory 高层 API: add() / search() / delete() / list()
└── tests/
    └── integration.rs       # tempdir + qdrant container 集成测
```

## 关键接口 (Rust API)

```rust
// 高层 API
pub struct Memory { /* ... */ }

impl Memory {
    pub fn new(config: MemoryConfig) -> Result<Self>;
    
    /// 从一段对话 / 文本中抽取 + 存储记忆。
    pub async fn add(&self, content: &str, scope: Scope, metadata: Option<Value>) -> Result<Vec<MemoryId>>;
    
    /// 按 query 语义检索。
    pub async fn search(&self, query: &str, scope: Scope, opts: SearchOpts) -> Result<Vec<MemoryMatch>>;
    
    pub async fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>>;
    pub async fn update(&self, id: &MemoryId, content: &str) -> Result<()>;
    pub async fn delete(&self, id: &MemoryId) -> Result<()>;
    pub async fn list(&self, scope: Scope, limit: u64) -> Result<Vec<MemoryRecord>>;
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub user_id: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub limit: u64,            // top-k, 默认 10
    pub score_threshold: f32,  // 余弦相似 >= 此值才返回, 默认 0.5
    pub filter: Option<Value>, // 任意 metadata JSON 过滤
}
```

## 数据模型 (Qdrant collection schema)

Collection: `frank_memories` (一个 collection 含所有 user/agent/session 的记忆, 用 metadata 过滤)

```json
{
  "id": "uuid v4",
  "vector": [1536 float],
  "payload": {
    "content": "user prefers vim over emacs",
    "user_id": "alice",
    "agent_id": "claude-code",     // 可选
    "session_id": "2026-05-21-T-1",// 可选
    "metadata": { /* 任意 JSON */ },
    "created_at": "2026-05-21T13:00:00Z",
    "updated_at": "2026-05-21T13:00:00Z"
  }
}
```

## 选型理由

### 为什么 Qdrant 而非 Chroma / Pinecone / Weaviate?

| | Qdrant | Chroma | Pinecone | Weaviate |
|---|---|---|---|---|
| 官方 Rust SDK | ✅ | ❌ | ❌ | ❌ |
| 自托管 docker | ✅ 一行 | ✅ | ❌ 付费云 | ✅ 但重 |
| 内存占用 | ~200MB | ~300MB | N/A | ~600MB |
| 性能 | 高 (Rust 写的) | 中 | 高 | 中 |

→ Qdrant 三项第一, 选它。

### 为什么 OpenAI embedding 而非 Claude / Cohere?

- Claude 没公开 embedding API (2026-05 时点)
- Cohere v3 质量好但贵 (US$ 1 / 1M tokens vs OpenAI text-embedding-3-small US$ 0.02 / 1M tokens)
- OpenAI text-embedding-3-small 1536 dim, 中英文都行, 0.02 美元 / 1M tokens 极便宜

→ 主选 OpenAI; 预留 Embedder trait 方便切换。

### 为什么 Claude 而非 GPT 做 fact extraction?

- Claude Haiku 4.5 抽 fact 质量好且便宜 (~0.25 USD / 1M tokens input)
- 用户主打 Claude Code, 已有 Anthropic API key
- GPT-4o-mini 价格类似, 抽取效果接近; 留 trait 待用户切换

## 风险

| ID | 风险 | 对策 |
|---|---|---|
| R-M1 | OpenAI / Anthropic API 限速 | client 加 retry + 指数退避; 失败的 add 进重试队列 |
| R-M2 | Qdrant 重启数据丢 | docker volume 持久化 + 定时 snapshot |
| R-M3 | embedding 模型升级导致 collection 全失效 | collection 名带版本: `frank_memories_v1`, 升级时双写 → 灰度 |
| R-M4 | LLM 抽取的 fact 错误 / 幻觉 | add 时不强信任, retrieve 时 score 阈值过滤; 后续给 UI 让用户 review |
| R-M5 | 跨 user / agent 数据串 | Qdrant payload 过滤强制 user_id 必填 (除非显式 global scope) |

## 不在 v1 范围

- Graph 关系 (P6 联动 orchestrator 时再加)
- 记忆衰减 / 主动遗忘 (用 TTL 简单实现; mem0 v3 思路待 review)
- 跨语言 SDK (Python / JS 等) — 暂时 Rust + REST 即可

## 后续动作

- [ ] crates/frank-memory 骨架: 6 个文件 (上面切分图)
- [ ] qdrant-client + anthropic-sdk + openai 依赖加 workspace
- [ ] tx:8317 deploy qdrant container (ADR-005)
- [ ] frank-memory 集成测: 真 qdrant + 真 embedding 跑一遍 add/search
- [ ] frank-sync-agent 暴露 REST API: `POST /memory/add` / `GET /memory/search` 等
- [ ] frank-cli 增 `frank memory add|search|list` 子命令调 sync-agent
