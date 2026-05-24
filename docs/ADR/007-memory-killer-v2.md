# ADR-007: frank-memory v2 "Memory Killer" — 三王炸 + 本地缓存 + bi-temporal

| Field | Value |
|---|---|
| **Status** | Deferred to v0.11+ (v0.10.4 优先 ADR-009 凭据桥; 记忆 v2 待用户后续决策) |
| **Date** | 2026-05-24 |
| **Decider** | hutiefang |
| **Extends** | ADR-003 (frank-memory v1 — Rust 重写 mem0 思路) |
| **Target release** | v0.11.0 |
| **Estimated effort** | ~17 工作日 (3-3.5 周) |

## 背景

frank-memory v1 (ADR-003) 已落地:Qdrant + OpenAI embedding + Claude Haiku 抽取, 三层 Scope, CRUD 全有。但暴露 4 个关键问题:

1. **本地无缓存** — 每次 `frank memory search` 都打云端 (~200-500ms),且烧 embedding API
2. **无淘汰策略** — 持续写入会爆 Qdrant (服务器 3.3G RAM 已紧张)
3. **被 mcp__memory 绕过** — claude 倾向调 MCP native tool, frank-memory 拿不到流量 (实测验证, 见对话 2026-05-24)
4. **可观测性零** — 用户看不到调用走云端还是本地, 烧了多少 token, 哪条记忆被命中

同时, 调研 2024-2026 6 个主流开源记忆方案 (mem0 / Letta / Graphiti / cognee / LangMem / mcp-memory) 发现:

- **行业 0/6 家做真淘汰** — Graphiti 是 bi-temporal "非删除式", 其他全 persist forever
- **行业 0/6 家做中转站 / 共享账号场景** — 全部假设 "账号即身份"
- **行业 0/6 家是纯 Rust 嵌入式** — mem0/Letta/Graphiti 全 Python+server, cognee 虽 embedded 但 Python

这是 frank 差异化的真空地带。v0.11 一刀切到位。

## 决策

**v0.11 = 必备可观测 + LanceDB 本地缓存 + 三王炸 (淘汰 / procedural / MCP兼容) + bi-temporal (零换栈)**。

不切技术栈, 全 Rust。新增依赖只 `lancedb` (Rust 嵌入式向量库, 与 Qdrant 同栈思路)。

### 关键选型表

| 维度 | v1 现状 | v2 决策 | 抄谁 |
|---|---|---|---|
| 存储 | 远程 Qdrant 单层 | **本地 LanceDB 缓存 + 远程 Qdrant source-of-truth** | cognee 验证嵌入式 LanceDB |
| 抽取 | LLM (Claude Haiku) | 保留, 加 single-pass ADD-only 算法 | mem0 2024 末新算法 |
| 检索 | 单纯向量 top-k | top-k + 本地缓存优先 | mem0 多信号思路简化 |
| Scope | user/agent/session | + procedural 第 4 类 | LangMem 三分类 |
| 时间 | 仅 created_at/updated_at | **+ valid_from / valid_to (bi-temporal)** | Graphiti (零换栈实现) |
| 淘汰 | 无 | **LFU + LRU + Agentic LLM 终审三层** | **frank 独创** (行业空白) |
| 协议 | 自家 REST | + frank-mem MCP server (9 端点兼容 mcp__memory) | 反 mcp-memory 的设计 |
| 可观测 | 仅 endpoint 一行 | token + cost + latency + source 全打 | frank 独创 |

### 不抄的 (含理由)

- ❌ **Letta Core/Recall/Archival 三层 session** — 复杂度高, frank `session_id` filter 已能 80% 覆盖。延后 v0.13。
- ❌ **Graphiti Neo4j 图层** — 换栈成本太高, 用户明确拒绝。bi-temporal 用 Qdrant payload 字段实现。
- ❌ **mem0 conflict resolution UPDATE/DELETE 重写** — mem0 自己 2024 末放弃了, 用 single-pass ADD-only 替代。
- ❌ **LangMem episodic 显式建模** — 已经能用 `Scope { agent_id, session_id }` + metadata.kind="ai_call" 覆盖。

## 模块切分变化 (相对 ADR-003)

```
crates/frank-memory/
├── src/
│   ├── lib.rs
│   ├── memory.rs          (扩: + valid_from/valid_to)
│   ├── store/
│   │   ├── mod.rs         (扩: + LocalCacheStore trait)
│   │   ├── qdrant.rs      (扩: bi-temporal payload 字段)
│   │   └── lance.rs       NEW   LanceDB 本地缓存实现
│   ├── embed/             (不变)
│   ├── extract/           (扩: ProceduralExtractor — 从对话识别 "用户偏好规则")
│   ├── evict/             NEW   三层淘汰
│   │   ├── mod.rs         Evictor trait + 价值分计算
│   │   ├── lru.rs         LRU
│   │   ├── lfu.rs         LFU
│   │   └── agentic.rs     LLM 终审
│   ├── procedural/        NEW   procedural memory
│   │   ├── mod.rs         ProceduralStore (用户级偏好规则)
│   │   └── injector.rs    frank-ask 调用前注入 system prompt
│   └── client.rs          (扩: + 全 API 返回 MemoryCallReport)

crates/frank-mcp/          NEW   frank-mem MCP server
├── src/
│   ├── main.rs            stdio-mode MCP server 入口
│   └── handlers/          9 端点: create_entities / search_nodes / ...
```

新文件 ~10 个 (按 ADR-001 每文件 < 300 行控制); 改文件 ~5 个。

## 关键数据模型变化

### MemoryRecord 扩展 (bi-temporal + procedural kind)

```rust
pub struct MemoryRecord {
    pub id: MemoryId,
    pub content: String,
    pub scope: Scope,
    pub metadata: serde_json::Value,

    // 旧字段
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    // 新字段 (bi-temporal)
    pub valid_from: DateTime<Utc>,         // 事实生效起点
    pub valid_to: Option<DateTime<Utc>>,   // None = 当前生效; Some = 已过期

    // 新字段 (淘汰评分)
    pub last_accessed: DateTime<Utc>,
    pub access_count: u64,
    pub pinned: bool,                      // 用户标记 important 的不淘汰

    // 新字段 (procedural 标记)
    pub kind: MemoryKind,                  // Semantic / Episodic / Procedural
}

pub enum MemoryKind { Semantic, Episodic, Procedural }
```

### MemoryCallReport (可观测性)

```rust
pub struct MemoryCallReport {
    pub source: CallSource,                // LocalCache / RemoteQdrant
    pub latency_ms: u64,
    pub embedding_tokens: u64,             // 0 if cache hit
    pub llm_tokens_input: u64,             // 0 if no extraction
    pub llm_tokens_output: u64,
    pub cost_usd: f64,
    pub hit_count: usize,
}

pub enum CallSource { LocalCache, RemoteQdrant, Hybrid }
```

所有 public API (`add` / `search` / `list` ...) 返回 `(result, MemoryCallReport)`, frank-cli 把 report 打 stderr。

## 三层淘汰算法详细

触发条件: `frank_memories_<user>` collection 达到 `max_records` (默认 100k, 可 config 调)。

```rust
// 价值分 (越高越保留)
score = pinned ? +∞                       // 用户标记不动
       : (
           α * recency_score(last_accessed, half_life=30d)
         + β * freq_score(access_count, log scaling)
         - γ * age_score(created_at)
         )

// 默认 α=0.5, β=0.3, γ=0.2
```

淘汰流程 (每天后台或手动 `frank memory cleanup` 触发):

1. **筛候选池**: 全量按 score 升序, 取最低 10%
2. **LFU 过滤**: 去掉近 7 天被检索过的 (避免误杀)
3. **LRU 过滤**: 去掉最近 30 天访问过的 (除非 score 极低)
4. **Agentic 终审** (可选, 默认开): 候选池给 Claude Haiku, 一次性 prompt:
   ```
   以下 N 条记忆即将淘汰, 请标记哪些 "明显仍有价值不该删":
   [...]
   仅返回该删的 id 列表 JSON。
   ```
5. **Tombstone**: 标记 `deleted_at`, 30 天后 hard delete (给用户后悔时间)

Agentic 终审成本: 一次淘汰 ~100 条 × 50 token = 5k input + 100 output ≈ 0.002 USD。可忽略。

## bi-temporal 实现 (零换栈)

不需要 Neo4j 或时间序数据库, 全在 Qdrant payload 实现:

```python
# update 时不覆盖, 走 "标过期 + 新建":
update(id, new_content):
    old = qdrant.retrieve(id)
    old.valid_to = now()
    qdrant.upsert(old)

    new_record = MemoryRecord {
        content: new_content,
        valid_from: now(),
        valid_to: None,
        ...
    }
    qdrant.insert(new_record)

# search 时默认只看当前生效:
search(query, scope):
    filter = scope_filter ++ "valid_to IS NULL"
    qdrant.search(query, filter)

# 时间旅行查询 (可选):
search_at(query, scope, t):
    filter = scope_filter ++ "valid_from <= t AND (valid_to IS NULL OR valid_to > t)"
```

复杂度 O(1), 存储成本翻倍但可接受 (淘汰策略会回收)。

## frank-mem MCP server 协议兼容

实现 9 个 endpoint 完全兼容 `mcp__memory__*`:

```
create_entities      → 转 frank memory add (kind=Semantic)
create_relations     → 转 add 含 metadata.relation_to
add_observations     → 转 add 追加到 existing entity
read_graph           → 转 list 全 scope 转 KG 视图
search_nodes         → 转 search top-k
open_nodes           → 转 get by name (用 content 匹配)
delete_entities      → 转 delete
delete_observations  → 转 update 减少 observation
delete_relations     → 转 update 移除 metadata.relation_to
```

用户切换方式:
```jsonc
// ~/.claude.json  改一行:
"memory": {
  "command": "frank-mcp",           // 之前是 npx @modelcontextprotocol/server-memory
  "args": ["memory"]
}
```

切换后, claude 调 `mcp__memory__*` 透明走 frank-memory (本地缓存 + 云端 Qdrant), 跨设备 + 跨 provider 通吃。

## Procedural Memory 实现

不是存"事实"是存"规则"。例:

```
用户多次纠正: "别废话, 直接给答案"
    ↓
ProceduralExtractor (LLM) 识别为规则: { rule: "回答简短, 不冗余" }
    ↓
存为 MemoryKind::Procedural, scope = { user_id }
    ↓
frank-ask 调用任何 LLM 前, ProceduralInjector 拉取 user 的全部 Procedural,
拼接为 system prompt 注入:
    "[用户偏好]\n- 回答简短, 不冗余\n- 优先 Rust 例子\n..."
```

效果: frank-ask 越用越懂用户, **无需每次检索注入** (规则常驻 system prompt, token 一次性付)。

## 风险

| ID | 风险 | 对策 |
|---|---|---|
| R-K1 | LanceDB 本地缓存与云端不一致 | 写永远先云后本地; 缓存有 TTL (默认 1h); 用户可 `frank memory sync --force` |
| R-K2 | 三层淘汰误删用户重要记忆 | Tombstone 30 天后 hard delete; `frank memory restore <id>` 命令; pinned 不淘 |
| R-K3 | Agentic 终审 LLM 幻觉错杀 | LLM 仅决定 "保哪些", 不决定 "删哪些" (反向逻辑, 错杀难发生) |
| R-K4 | bi-temporal 翻倍存储 | 自动淘汰会回收; 老 `valid_to != NULL` 记录走单独 archival collection |
| R-K5 | frank-mem MCP server 协议变更 | MCP 协议有版本号, server 端 fallback 旧版 |
| R-K6 | Procedural 注入污染 system prompt | 上限 1000 chars; 用户可 `frank memory disable-procedural` |
| R-K7 | 用户从 mcp__memory 迁移数据丢 | 提供 `frank memory import-from-mcp <path>` 命令一次性迁移 |

## 不在 v0.11 范围 (留 v0.13+)

- Letta Core/Recall/Archival 三层 session 升级
- 多设备 CRDT 双向同步 (现在 LWW 已够, sync-agent 是单点)
- Graph 关系层 (KG 在 frank-mem MCP server 内部模拟, 不真建 Neo4j)
- 时间旅行 Web UI 可视化 (CLI 先支持, Web UI 后跟)
- 团队/组织级共享记忆

## 后续动作 (task 拆解, 已进 TaskList)

见 TaskList #66-#75 (10 个 v0.11 任务)。

## 验收 (v0.11.0 release 前必须)

- [ ] `frank memory` 全部子命令 stderr 输出 MemoryCallReport
- [ ] `frank ai ask` 输出 model + tokens + cost + latency
- [ ] `frank doctor` 输出"记忆全景"节, 明确列出 mcp__memory 和 frank-memory 状态
- [ ] 装 frank 自动创建 `~/.frank/claude-template.md` 供用户参考插入 CLAUDE.md
- [ ] LanceDB 本地缓存命中 `< 50ms` (benchmarks)
- [ ] bi-temporal 集成测试: update 后 search 默认只见新, `--at <ts>` 能见旧
- [ ] 三层淘汰跑通: 写 200 条 → 触发 cleanup → 标 tombstone → 30 天后 hard delete (用 time mock 测)
- [ ] Procedural extraction + injection 跑通: 模拟"用户说别废话" → 下次 ask 自动注入
- [ ] frank-mcp binary 启动, 9 端点对 MCP inspector 全绿
- [ ] Web UI 加 token 仪表 + procedural 规则 list
- [ ] Plan Review by codex >= 7.0, 无单维度 <= 3
- [ ] Code Review by codex >= 7.0, 无单维度 <= 3
- [ ] CI 全绿 (workspace clippy/test/fmt/docs/audit/secret-scan)
- [ ] 6 平台 release archive 全 success
- [ ] Homebrew Formula bump 0.10.3 → 0.11.0

## 相关

- ADR-003: frank-memory Rust 重写 (本 ADR 扩展)
- ADR-005: tx 部署 (qdrant 在那)
- ADR-006: skill 自含原则 (frank-mcp 作为新 binary 不破坏)
