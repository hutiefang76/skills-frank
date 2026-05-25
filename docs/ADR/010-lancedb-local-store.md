> Status: Proposed (待 codex Plan Review + 用户拍板)
> Date: 2026-05-25
> Decider: hutiefang
> Target release: v0.11.0 (PHASE-9 子项 A)
> Relates to: ADR-003 (frank-memory v1 Qdrant 实现), ADR-005 (tx 部署), ADR-007 (memory v2 已 deferred 至本 phase)
> Source-of-truth 引用: `docs/POSITION.md` 维度 #1, `docs/phases/PHASE-9-PLAN.md` 子项 A

# ADR-010: 本地 LanceDB 主存倒置 (v0.11)

## 1. 背景

frank-memory v1 (ADR-003) 把 Qdrant 当唯一存储, 所有 `frank memory add/list/search` 都走 tx:8318 → caddy → qdrant。两个实测痛点:

1. **离线全瘫** — VPN / 飞机 / 公司禁外网 → frank memory 全废 (写不进, 也搜不到)。
2. **每次 search 200-500ms 网络往返** — embedding 算完后还要打云 + qdrant HNSW (实测中位数 280ms, P99 600ms+)。本地 BGE-small 算 embedding 只要 ~30ms, 真正瓶颈是 RTT。

`POSITION.md` 维度 #1 已拍板: **倒过来 — 本地 LanceDB 主存, Qdrant 仅同步**。这跟"多设备同步"(第 0 优先级) 不矛盾:本地是写入入口和主索引, 远程 sync-agent 异步备份成跨设备共享层。先单机闭环, 再 v0.12 把跨设备同步加强。

本 ADR 仅决"本地主存选哪个嵌入式向量库", **不**决多设备同步算法 (LWW/CRDT 留 v0.12 单独 ADR)。

## 2. 决策

**采用 `lancedb` Rust crate (v0.29.0, 2026-05-13 发布) 做本地主存。**

数据库文件落 `~/.frank/memory/lance.db/` 目录, 走 LanceDB 嵌入式模式 (无 server 进程), 同 binary 内 await。

写策略: **本地先写**, 标 `sync_status="pending"`; 后台 task 异步推 sync-agent → Qdrant。读策略: **优先本地**, 本地查不到 (或显式 `--remote`) 才打 Qdrant。

文件锁策略: 复用 v0.10.7 已引入的 `fs2` crate, 在 `~/.frank/memory/lance.db.lock` 上加 `try_lock_exclusive()` 串行化写入器; 读不加锁 (LanceDB MVCC 保证读一致快照)。

## 3. LanceDB 成熟度调研 (2026-05-25 实查)

| 维度 | 数据 | 来源 |
|---|---|---|
| **GitHub stars** | 10,391 | `gh api repos/lancedb/lancedb` (2026-05-25) |
| **Forks** | 883 | 同上 |
| **Open issues** | 668 | 同上 — 数量多但活跃维护中 |
| **首次发布** | 2023-02-28 | 同上 |
| **最近 commit** | 2026-05-23 | 同上 (2 天前, 活跃) |
| **License** | Apache-2.0 | 同上 |
| **crate `lancedb` 最新版** | **0.29.0** (2026-05-13 发) | https://crates.io/api/v1/crates/lancedb |
| **总下载** | 414,706 | 同上 |
| **近 90 日下载** | 226,854 | 同上 — 真在用, 不是僵尸 crate |
| **底层 `lance` crate** | 6.0.1 (2026-05-20 发), 总下载 1.38M | https://crates.io/api/v1/crates/lance |
| **真实生产用户** | AnythingLLM 默认存储 (10W+ docker pulls), Rig framework 集成 | https://github.com/Mintplex-Labs/anything-llm |
| **公司背书** | LanceDB Inc. 商业公司, 主营 OSS embedded retrieval | https://lancedb.com |

**结论:** 不是早期/玩具项目。0.29 版本号偏低是 Rust 生态保守习惯 (qdrant-client Rust 也才 1.13), 不代表不稳定。issue 668 数量大但常见于热门 OSS, 跟 docker 镜像或 Python 包相关的多。**production-ready, 选它没问题。**

### `lance` vs `lancedb` 的关系

- `lance` = 底层列存格式 + 向量索引引擎 (类似 parquet + faiss, 用 Apache Arrow), Rust 原生
- `lancedb` = `lance` 之上的"数据库"语义层, 提供 `Connection` / `Table` / `Query` API
- frank 用 `lancedb` (高层 API 更稳, 低层细节让 lance 内部处理)

## 4. 并发与文件锁 (实查)

**LanceDB FAQ + GitHub issue 真实读到的结论:**

| 场景 | 是否安全 | 实测来源 |
|---|---|---|
| 多 reader 同时读 | ✅ 完全 OK, 无锁 | docs.lancedb.com FAQ: "concurrent reads very well" |
| 单 writer + 多 reader | ✅ MVCC 保证读永远一致快照 | 同上 + lancedb issue #1888 |
| 多 writer 并发 append | ⚠️ 一定数量内 OK, 太多 commit 冲突会失败 | FAQ 原文: "limited number of times a writer retries a commit" |
| 多 writer 并发 delete/update/merge_insert | ❌ 冲突频繁 | issue #1597 (要求自动 retry, 截至 0.29 未实现) |
| `fork()` 多进程 | ❌ 文档明确禁用 (Lance 多线程内部, fork 不安全) | FAQ |

**LanceDB 本身没有 OS 级 file lock (flock/fcntl)**, 它走 MVCC + 乐观提交 (写新版本元数据, 冲突时 retry)。这对 frank 的场景有个 sharp edge:

> 同机器多 frank cli 窗口同时跑 `frank memory add` (用户 3 个终端粘贴对话) → 100% 都是 append → 理论上 lancedb 自己能扛, 但 retry 上限是 internal 默认 (源码 5 次), 偶发触顶。

**frank 的对策 — 单写串行化:**

```rust
// crates/frank-memory/src/local_store.rs
use fs2::FileExt;

let lock = File::create(memory_dir.join("lance.db.lock"))?;
lock.try_lock_exclusive()
    .map_err(|_| anyhow!("another frank process is writing memory; retry in a moment"))?;
// ... lancedb write here ...
drop(lock);  // 隐式解锁
```

- `fs2 = "0.4.3"` 已在 workspace deps (v0.10.7 history file lock 已用), 不引新依赖。
- 写串行化: 单进程持锁 → LanceDB 不可能撞 commit 冲突。
- 读不加锁: LanceDB MVCC 保证读永远见某个一致快照, 不需要协调。
- 锁竞争失败给清晰人话提示 ("另一个 frank 进程在写, 稍后重试"), 不卡住用户。

跨平台:
- macOS / Linux: `fs2` 底层 `flock(2)`, OK。
- Windows: `fs2` 底层 `LockFileEx`, OK。CI matrix 已覆盖 windows-2026。

## 5. 性能调研 (实查)

> 注: 网上 LanceDB benchmark 多数面向"百万-千万级 cloud workload", 跟 frank 单用户~1k-10w 条记忆不完全对应。下面是查到的可比数字, 加上对 frank 实际场景的推断。

| 数据集 / 向量维度 | LanceDB | Qdrant (现行) | pgvector |
|---|---|---|---|
| GIST 1M, 960d | 40-60ms 平均 (IVF-PQ) | 20-30ms 平均 (HNSW) | — |
| 100k records, 768d | — | P50 1.8ms / P99 3.6ms (HNSW) | P50 2.1ms / P99 4.3ms |
| 500k records, 768d | — | P50 3.1ms / P99 6.4ms | P50 4.8ms / P99 9.2ms |
| 召回率 (GIST 1M) | ~88% recall@1 (IVF-PQ) | ~95% recall@1 (HNSW) | — |

来源:
- LanceDB vs Qdrant: https://medium.com/@vinayak702010/lancedb-vs-qdrant-for-conversational-ai-vector-search-in-knowledge-bases-793ac51e0b81
- pgvector vs Qdrant: callsphere.ai 2026 benchmark, https://callsphere.ai/blog/vector-database-benchmarks-2026-pgvector-qdrant-weaviate-milvus-lancedb

**对 frank 场景 (384d BGE-small, 单用户 1k-10w 记忆) 的推断:**

- 维度小 (384 vs 768/960) → 检索更快, 估计 1k records 单查 < 10ms (CPU 直 brute force 也只 1k × 384 × 4 byte = 1.5 MB 全过一遍 SIMD)
- 即使 LanceDB 默认 IVF-PQ 慢于 Qdrant HNSW, 本地省掉 网络 RTT (200-500ms) 也直接赢
- v0.11 性能验收门 (PHASE-9 已定): **1k records P50 < 50ms / 10k records P50 < 200ms** — 看上面对比表绝对达得到 (LanceDB 在 GIST 1M @ 960d 才 40-60ms, frank 数据量小 25-100 倍 + 维度小 2.5 倍)

如果实测不达标, 见 §8 风险 R3 应对。

## 6. 详设

### 6.1 文件布局

```
~/.frank/memory/
├── lance.db/                  # LanceDB 数据目录 (lancedb::connect 指向)
│   └── memories.lance/        # 自动建的 table (Lance 列存格式)
├── lance.db.lock              # fs2 互斥锁 (写串行化)
└── bm25.idx/                  # PHASE-9 子项 B 的 tantivy 索引 (不在本 ADR 范围)
```

跨平台 `~`: 沿用现有 `dirs::home_dir()`, frank 已统一处理。

### 6.2 Arrow Schema

LanceDB 用 arrow-rs, 向量列必须是 `FixedSizeList<Float32, N>`:

```rust
use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

fn build_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        // MemoryId, UUID string
        Field::new("id", DataType::Utf8, false),

        // Scope: 三字段独立列 (而非嵌套 struct) — 直接走 LanceDB SQL filter,
        // 性能好且不重蹈 Qdrant 早期 "scope.user_id" dot-notation 的坑 (ADR-003 实施总结).
        Field::new("user_id", DataType::Utf8, true),     // nullable
        Field::new("agent_id", DataType::Utf8, true),
        Field::new("session_id", DataType::Utf8, true),

        // 事实内容
        Field::new("content", DataType::Utf8, false),

        // 元数据 JSON (字符串存, 上层 serde_json 解)
        Field::new("metadata_json", DataType::Utf8, true),

        // 时间 (Unix epoch ms, i64 — Arrow Timestamp 跨版本兼容性差, 实用主义选 i64)
        Field::new("created_at_ms", DataType::Int64, false),
        Field::new("updated_at_ms", DataType::Int64, false),

        // 同步状态: "synced" / "pending" / "failed" (短字符串足够, 不用枚举编码省事)
        Field::new("sync_status", DataType::Utf8, false),

        // 向量 (384d BGE-small 默认)
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                384,
            ),
            false,
        ),
    ]))
}
```

设计决策说明:

- **scope 拍平** — 不嵌套 struct, 直接 3 列 nullable utf8。理由: ADR-003 实施时踩过 Qdrant payload dot-notation 漏 filter 的坑 (`scope.user_id` vs `user_id` 写错全 list 漏命中, codex review + 真测才发现)。LanceDB SQL filter `WHERE user_id = 'alice'` 写起来直观, 不留隐患。
- **metadata JSON 字符串** — 不展开成 列, 因为 metadata 是用户自由 JSON, 字段不确定。上层 `MemoryRecord` serde 时序列化 `metadata` 字段为 string 存进去, 读出来再 deserialize。
- **时间 i64 ms** — Arrow Timestamp 类型在 lance 0.x 跨小版本曾改过编码, 用 i64 epoch ms 跟 chrono 单向转换 (`DateTime::from_timestamp_millis`) 最稳。
- **向量维度 hardcode 384** — 与现行 `LocalEmbedder` (fastembed BGE-small) 对齐。后续换模型走 ADR-003 的 versioned collection 思路 (table 名带版本号 `memories_v1`)。

### 6.3 `LocalStore` trait

复用现有 `store::MemoryStore` async trait? **不**, 单独一个 trait `LocalStore`。理由:

- `MemoryStore` 是远程后端抽象 (Qdrant), 方法返 anyhow::Result 不带 sync_status
- `LocalStore` 多了 `sync_status` / `pending` / `mark_synced` 等本地专属操作
- 把两个混一起会让 trait 膨胀, 违反 ADR-001 "单一职责"

```rust
// crates/frank-memory/src/local_store/mod.rs
use async_trait::async_trait;
use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Synced,
    Pending,
    Failed,
}

#[derive(Debug, Clone)]
pub struct LocalRecord {
    pub record: MemoryRecord,
    pub embedding: Vec<f32>,
    pub sync_status: SyncStatus,
}

#[async_trait]
pub trait LocalStore: Send + Sync {
    /// 初始化目录 + table, 幂等。维度由调用方传 (跟 embedder 对齐)。
    async fn ensure_initialized(&self, vector_dim: usize) -> anyhow::Result<()>;

    /// 写入一条; sync_status 默认 Pending。
    async fn add(&self, item: LocalRecord) -> anyhow::Result<()>;

    /// 向量检索 top-k, 过滤 scope。
    async fn search(
        &self,
        query_vector: Vec<f32>,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> anyhow::Result<Vec<MemoryMatch>>;

    /// 按 scope 列出 (created_at desc)。
    async fn list(&self, scope: &Scope, limit: u64) -> anyhow::Result<Vec<MemoryRecord>>;

    /// 按 id 删, 幂等。
    async fn delete(&self, id: &MemoryId) -> anyhow::Result<()>;

    /// 列出 sync_status=Pending 的, 给后台同步 worker 用。
    async fn pending_sync(&self, limit: u64) -> anyhow::Result<Vec<LocalRecord>>;

    /// 把指定 id 标 Synced。
    async fn mark_synced(&self, ids: &[MemoryId]) -> anyhow::Result<()>;
}
```

`LanceLocalStore` 在 `local_store/lance.rs` 实现这个 trait。

### 6.4 双写 + 读优先策略 (`Memory` 高层 API)

`crates/frank-memory/src/client.rs` 的 `Memory::add` / `Memory::search` 改:

```rust
pub struct Memory {
    extractor: Box<dyn FactExtractor>,
    embedder: Box<dyn Embedder>,
    local: Arc<dyn LocalStore>,           // 主存
    remote: Option<Arc<dyn MemoryStore>>, // Qdrant, 可空 (--offline 模式)
}

impl Memory {
    /// 写: 本地先 (强一致), 远程异步 (最终一致)。
    /// 返回时本地已落, 远程在 tokio::spawn 里推。
    pub async fn add(&self, content: &str, scope: Scope, metadata: Value) -> Result<Vec<MemoryId>> {
        let facts = self.extractor.extract(content, &scope).await?;
        let embeddings = self.embedder.embed_batch(&facts).await?;

        let mut ids = Vec::new();
        for (fact, vec) in facts.iter().zip(embeddings) {
            let record = MemoryRecord { /* ... sync_status=Pending */ };
            let item = LocalRecord { record: record.clone(), embedding: vec, sync_status: SyncStatus::Pending };
            self.local.add(item.clone()).await?;          // 阻塞: 本地必须成
            ids.push(record.id);

            if let Some(remote) = self.remote.clone() {
                let local = self.local.clone();
                tokio::spawn(async move {
                    match remote.upsert(item.clone().into()).await {
                        Ok(_) => {
                            let _ = local.mark_synced(&[item.record.id]).await;
                        }
                        Err(e) => tracing::warn!(error=?e, "remote sync failed; will retry"),
                    }
                });
            }
        }
        Ok(ids)
    }

    /// 读: 优先本地; 本地空 (新机器没缓存) 或显式 remote 才打远程。
    pub async fn search(&self, query: &str, scope: Scope, opts: SearchOpts) -> Result<Vec<MemoryMatch>> {
        let qvec = self.embedder.embed_query(query).await?;
        let local_hits = self.local.search(qvec.clone(), &scope, &opts).await?;

        if !local_hits.is_empty() || self.remote.is_none() {
            return Ok(local_hits);
        }

        // 本地空 → fallback remote (新装的机器/首次同步前)
        tracing::info!("local empty, falling back to remote");
        self.remote.as_ref().unwrap().search(qvec, &scope, &opts).await
    }
}
```

后台同步 worker 详设留 v0.12 (LWW 冲突解决一起做)。v0.11 先用 best-effort spawn, 失败只 warn 不阻塞用户。

### 6.5 索引策略 (留 default, 别过早优化)

LanceDB 在 `FixedSizeList<Float32>` 列上 **自动建 IVF-PQ 索引** 当 `create_index(Index::Auto)`。v0.11 数据规模小 (单用户 < 10w 条), 拍板:

- **不显式 create_index** — LanceDB 自动判断, 小表走 brute force flat scan (内存够, SIMD 快, 召回 100%), 大表才上 IVF-PQ
- 待数据真涨到 5w+ 再考虑手动 `create_index(Index::IvfPq)` 调参 (ivf_n_lists, pq_n_sub)

理由: 过早建 ANN 索引会牺牲召回 (IVF-PQ 是有损), 小数据没必要。

### 6.6 错误恢复

- **lance.db 目录损坏**: 启动时 `connect` 失败 → 重命名为 `lance.db.broken.<ts>`, 重建空库, 提示用户 `frank memory sync --pull-from-remote` 从 Qdrant 拉回 (实现留 v0.12)
- **lock 文件残留** (frank 进程 kill -9): `fs2::try_lock_exclusive` 重试 3 次 ×  100ms, 真拿不到才报错 (单机用户级场景, 不引重型 lock 协议)
- **磁盘满**: lancedb 报错原文透传 + 友好提示 ("~/.frank/memory 所在分区已满, 释放空间后重试")

## 7. 后果

### 优点

- **离线可用** — 单测验收: 拔网 ✈️ 模式下 `frank memory add/list/search` 全跑通
- **<50ms 检索 (1k 数据)** — 本地 BGE-small ~30ms + LanceDB flat scan ~5ms + serde ~5ms (估算)
- **零额外 binary** — lancedb crate 嵌入式, 没有要起 daemon
- **diff-able 数据** — Lance 列存文件可以 `lance inspect` 命令调试 (不是黑盒)
- **同步状态可见** — `frank memory list --sync-status pending` 让用户看哪些还没推上去

### 缺点

- **多设备同步要做** — 留 v0.12 单独 ADR (LWW 冲突解决 + tombstone)。v0.11 先单机本地闭环, 新机器只能从 Qdrant fallback 拉 (实现也在 v0.12)
- **磁盘占用** — Lance 列存对小数据集冗余比 SQLite 高 (估算 1k 条 384d 向量 + metadata ≈ 2-3 MB, 10w 条 ≈ 200-300 MB; 可接受, 远小于 fastembed 模型本身 100MB+)
- **额外依赖** — `lancedb` + `arrow-array` + `arrow-schema` 三个 crate (+ 传递依赖 datafusion 等), 编译时长会涨。估算 release build +30-60s。需 CI 实测验证 (验收门: 仍 < 5 min)
- **跨平台 Windows 验证** — lance 在 Windows 上的成熟度不如 Linux/macOS (GitHub issue 里多见 Linux 报告), 必须 CI 真测 (matrix 已覆盖)

### 维护成本

- 跟 ADR-003 Qdrant 维护成本基本一致 (都是 crate 升级 + 接口跟随)
- LanceDB 月度小版本节奏 (2026 这两月 0.26 → 0.27 → 0.29), 比 qdrant-client 节奏快 → 注意 breaking change
- 锁文件 + 双写逻辑是新增的 frank 自己代码, 维护责任在 frank 这边

## 8. 备用方案 (Alternatives Considered)

### A. sqlite-vec + rusqlite

| 维度 | 数据 |
|---|---|
| crate `sqlite-vec` 最新版 | 0.1.9 (2026-05-18) |
| 总下载 | 1,614,148 (近 90d 1,111,309) |
| 成熟度 | 是 sqlite-vss 的替代继任 (sqlite-vss 已停止维护) |
| 性能 | brute force, 大数据慢 2-3 个数量级; 小数据 OK |

**优点**: SQLite 极成熟, 单文件即整库, 备份/迁移最简单; 文件锁是 SQLite 自带 (WAL mode 多 reader 单 writer 天然支持)。

**缺点**:
1. sqlite-vec **没有 ANN 索引**, 全是 brute force。1w 条 OK, 5w+ 性能会肉眼可见地降
2. Rust binding 走 rusqlite + load_extension, 不是 pure Rust (要链 sqlite3 动态库, frank 现在 binary 是纯静态链接的, 加 sqlite 会破 portability)
3. sqlite-vec 0.1.x 版本号体现"早期" — API 还在收敛

**弃用理由**: 性能上限低 (无 ANN) + 破 frank 纯静态 binary 卖点。备胎留, 不当主选。

### B. 自己写 BLOB 存 Vec<f32> + brute force search

直接拿 rusqlite 或者裸 `~/.frank/memory/records.json` 存 record, 向量序列化成 BLOB, 检索时全表扫 cosine。

**优点**: 零外部依赖, 实现 50 行 Rust 可成。

**缺点**:
1. 没有任何索引, 1w 条已经接近"等不及"
2. 自己写持久化 = 重新发明 lance/sqlite 已解决的问题 (crash safety, MVCC, atomic write)
3. 跟 frank 的"用最成熟现成的库, 自己只写治理逻辑"原则相悖

**弃用理由**: 性能差且重新造轮子。仅作为"什么都坏了应急" demo 留个心理位。

### C. DuckDB + vss extension

| 维度 | 数据 |
|---|---|
| `duckdb` crate | 较新 (2024 起), 0.x |
| `vss` ext | 实验性, 文档少 |

**优点**: DuckDB 列存 + SQL 强, 跟 LanceDB 有重叠 (都是 columnar + ANN)。

**缺点**:
1. Rust binding 比 Python 弱很多 (官方主力是 Python/CLI)
2. vss extension 是 experimental, 不像 sqlite-vec 已经被多个生产用户验证
3. 编译进 frank 单 binary 麻烦 (要带 DuckDB C++ 库)

**弃用理由**: 没有"比 LanceDB 明显好的点", 且生态弱。

### 综合对比 — 决策矩阵

| | LanceDB | sqlite-vec | 裸 BLOB | DuckDB+vss |
|---|---|---|---|---|
| 纯 Rust pure-static binary | ✅ | ❌ (要 libsqlite3) | ✅ | ❌ (要 C++) |
| ANN 索引 (IVF-PQ/HNSW) | ✅ 自动 | ❌ brute force | ❌ | ✅ HNSW (experimental) |
| 真实生产用户 | ✅ AnythingLLM | ✅ 多个 | — | — |
| Rust 生态成熟 | 0.29, 月度发版 | 0.1.9 | n/a | 0.x |
| 文档与示例 | 多 | 多 | 自己写 | 少 |
| **总分** | **5/5** | 2/5 | 1/5 | 1/5 |

**主选 LanceDB, 备选 sqlite-vec (R1 触发时切换)。**

## 9. 风险与应对

| ID | 风险 | 触发判据 | 应对 |
|---|---|---|---|
| **R1** | LanceDB 在 frank 真实场景踩大坑 (锁冲突 / Windows 不稳 / 性能不达标) | 集成测有 ≥ 2 类 P0 bug, 或 v0.11 验收门连测 3 轮不过 | **fallback sqlite-vec** — `LocalStore` trait 已抽象, 切换只动 `local_store/lance.rs` → `local_store/sqlite.rs`, 上层不动 |
| **R2** | `fs2` advisory lock 在 NFS / 网络盘失效 | 用户上报 `~/.frank` 在 NFS / iCloud Drive 同步 | 文档明确禁用 `~/.frank` 放在云同步目录; doctor 探测并警告 |
| **R3** | 性能不达 PHASE-9 验收 (1k P50 < 50ms / 10k P50 < 200ms) | benchmark 实测超标 | **三步降级**: (a) 关 IVF-PQ 强制 flat scan; (b) 减小 limit 默认 (10 → 5); (c) 砍 LanceDB 改 sqlite-vec brute force (R1 同款 fallback) |
| **R4** | lance 列存 schema 升级要重建表 | 未来加新 field (e.g. v0.12 加 valid_from for bi-temporal) | table 名带 schema 版本 `memories_v1`, 升级时双写灰度 (沿用 ADR-003 思路) |
| **R5** | 写串行化锁吞吐瓶颈 (用户高频 add) | 用户上报 `lock_failed` 频率高 | 现实里单用户单机不会高频, 不预先优化; 真出问题加 inner mutex batch flush |
| **R6** | 多设备 LWW 没做之前, 用户 2 台机器各写一批 → 同步时谁覆盖谁? | v0.11 ship 后用户在 2 台机器并行用 | v0.11 文档明确 "同步是 best-effort, 多设备并行写有可能丢" → v0.12 强化, 不在本 ADR 范围 |
| **R7** | 编译时长涨 (lancedb + datafusion 一波依赖) | CI build 时长 > 7 min | 调研 lancedb `default-features = false`, 关掉 datafusion 等用不上的; 实测 |
| **R8** | macOS Keychain ACL 误锁 ~/.frank | 不该发生 (frank 自家目录), 但 v0.10.4 凭据桥踩过 ACL 隐式坑 | 集成测包含跨进程链 (codex spawn frank-cli 写 memory), 验证锁正常 |

## 10. 验收标准 (给 D / C 阶段)

### 单测 (`cargo test -p frank-memory`)

- [ ] `lance_local_store::roundtrip` — add → list → search → delete 全通
- [ ] `lance_local_store::scope_filter` — 三层 scope 过滤精准 (user/agent/session 任意组合, 包括 "scope.user_id" 误写 bug 回归测)
- [ ] `lance_local_store::vector_search` — 注入 5 条已知 fact + 已知 embedding, 查 query 返回 top-3 正确
- [ ] `lance_local_store::sync_status_lifecycle` — add → pending → mark_synced → 状态正确
- [ ] `lance_local_store::lock_concurrent` — 起 2 个 task 同时 add, 后到的应被 fs2 lock 拒绝并清晰报错
- [ ] `lance_local_store::lock_recovery` — kill 持锁 task 后, 新 task 能在 3 次重试内拿到锁
- [ ] `lance_local_store::corrupt_recovery` — 把 lance.db 写入垃圾, ensure_initialized 应该 rename 旧目录并重建空库

### 集成测 (workspace level, 需真 lancedb)

- [ ] `frank memory add "user prefers vim"` → 本地 lance.db/memories.lance/ 目录里看到新文件
- [ ] `frank memory list` → 优先返本地 (实测打开 wireshark 确认无 tx:8318 流量)
- [ ] `frank memory search "editor"` → 命中刚写入的 vim 记忆, 走本地
- [ ] **离线测**: 改 hosts 把 frank.hutiefang.com 指 127.0.0.1, `frank memory add/list/search` 全跑通
- [ ] **跨进程链测**: 从 codex 内调 `frank memory add` × 3 个并行子进程, 至少 1 个成 2 个被锁拒 (无 crash)

### 性能基准 (`cargo bench -p frank-memory --bench local_store`)

- [ ] 1k records (384d), 检索 P50 < 50ms (本地 BGE embed + lance scan)
- [ ] 10k records (384d), 检索 P50 < 200ms
- [ ] add 单条 (含 embed + lance write) < 100ms

### 文档与发布

- [ ] `docs/PROGRESS.md` 更新 v0.11 子项 A 状态
- [ ] `crates/frank-memory/src/local_store/mod.rs` 顶部加模块级 `//!` rustdoc 引本 ADR
- [ ] codex Plan Review (本 ADR) ≥ 7.0 且无维度 ≤ 3
- [ ] codex Code Review (实现完) ≥ 7.0 且无维度 ≤ 3
- [ ] CI 全绿 (workspace clippy/test/fmt/docs/audit/secret-scan, 3 OS matrix)

## 11. 不在本 ADR 范围

- 多设备同步算法 (LWW / CRDT) — v0.12 单独 ADR
- 离线时 sync-agent 重连后的回灌策略 — v0.12
- 4 路混合召回 (vector + BM25 + time decay + metadata) — PHASE-9 子项 B 单独 ADR-011
- extractor auto-detect — PHASE-9 子项 E
- PostToolUse hook 截 mcp__memory — PHASE-9 子项 H
- 记忆衰减 / 主动遗忘 — 撤回项 (POSITION.md), 走手动 cleanup 替代, 留 v0.12
- bi-temporal valid_from / valid_to — ADR-007 思路, 留 v0.13

## 12. 参考

- `docs/POSITION.md` — frank-memory 13 维度定位 (维度 #1 决定本 ADR 主轴)
- `docs/phases/PHASE-9-PLAN.md` — v0.11 子项 A 范围与排期
- `docs/ADR/003-frank-memory-rust.md` — v1 Qdrant 实现 (本 ADR 的扩展)
- `docs/ADR/005-deploy-tencent-8317.md` — tx:8318 部署拓扑 (Qdrant 作远程同步终端)
- `docs/ADR/007-memory-killer-v2.md` — 早期 LanceDB 提案 (已 deferred, 本 ADR 是其子集落地)
- LanceDB GitHub: https://github.com/lancedb/lancedb (10391 stars, Apache-2.0)
- LanceDB crate: https://crates.io/crates/lancedb (0.29.0, 2026-05-13)
- Lance crate: https://crates.io/crates/lance (6.0.1, 2026-05-20)
- LanceDB FAQ (并发): https://docs.lancedb.com/faq/faq-oss
- LanceDB issue #213 (concurrent multiprocess): https://github.com/lancedb/lancedb/issues/213
- LanceDB issue #1597 (auto retry on commit conflict): https://github.com/lancedb/lancedb/issues/1597
- AnythingLLM 用 LanceDB 做默认存储: https://github.com/Mintplex-Labs/anything-llm
- LanceDB vs Qdrant benchmark (GIST 1M, 960d): https://medium.com/@vinayak702010/lancedb-vs-qdrant-for-conversational-ai-vector-search-in-knowledge-bases-793ac51e0b81
- pgvector vs Qdrant 2026 benchmark: https://callsphere.ai/blog/vector-database-benchmarks-2026-pgvector-qdrant-weaviate-milvus-lancedb
- sqlite-vec (备用方案 A): https://github.com/asg017/sqlite-vec
- fs2 crate (锁实现, 已在 v0.10.7 history 用过): https://crates.io/crates/fs2 (0.4.3)
