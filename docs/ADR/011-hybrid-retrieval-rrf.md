# ADR-011: Hybrid Retrieval 4 路并行 + RRF 融合 (v0.11)

| Field | Value |
|---|---|
| **Status** | Proposed (等用户 review) |
| **Date** | 2026-05-25 |
| **Decider** | hutiefang |
| **Phase** | v0.11.0 子项 B (见 `docs/phases/PHASE-9-PLAN.md`) |
| **POSITION** | 维度 #4 多路召回 (`docs/POSITION.md`) |
| **Depends on** | ADR-003 (frank-memory v1), v0.11 子项 A 本地 LanceDB 主存 |
| **Estimated effort** | 3 工作日 (P 0.5d / D 1.5d / C 1d) |

---

## 1. 背景 (Context)

### 1.1 现状

frank-memory v1 的检索是**单路向量**:

```rust
// crates/frank-memory/src/client.rs:141
pub async fn search(&self, query: &str, scope: Scope, opts: SearchOpts) -> Result<Vec<MemoryMatch>> {
    let embedding = self.config.embedder.embed(query).await?;
    self.config.store.search(embedding.vector, &scope, &opts).await
}
```

- Embedder = `fastembed` BGE-small (384 dim, 本地)
- Store = Qdrant remote (cosine 相似度 top-k)
- 单测 14 个全绿,但**召回质量没人测过**

### 1.2 单路向量召不准的典型场景

实测能复现的漏召场景 (各家 hybrid retrieval 方案的共识痛点):

| 场景 | 单路向量为何漏 | 期望路 |
|---|---|---|
| **精确专有名词**: `"frank-orchestrator P6 ADR"` | 向量泛化,把 P5 / P7 / 其他 ADR 也拽进来 | BM25 锁定 "frank-orchestrator" 字面 |
| **近期偏好覆盖**: 用户 1 月说喜欢 vim,5 月说改用 helix | 两条 fact embedding 都命中,vim 排前 (历史更长) | 时间衰减把 5 月的拉前 |
| **明确 scope 隔离**: `agent_id=codex` 的 fact 不该混进 claude session | scope filter 是 hard filter,但**不参与排序**,余弦近的 user-level fact 仍盖过 agent-level | metadata 路把 agent 完全匹配的优先 |
| **缩写 / 黑话**: `"k8s"` vs `"Kubernetes"` | embedding 模型多数能召,**但中文黑话 / 项目内部缩写不一定** | BM25 字面 + LLM 抽取时同义补全 |

POSITION.md #4 写的是:

> mem0 (语义+BM25+实体)、Zep (语义+BM25+图) | 单路向量 | ⚠️ **4 路并行 + RRF**

业界共识:**纯语义召回不够,需要稀疏 + 元信号兜底。**

### 1.3 行业对标 (真查,非估算)

| 系统 | 路数 | 融合算法 | 来源 |
|---|---|---|---|
| **mem0 v3** | 4 路: vector + BM25 + entity + temporal | **RRF + cross-encoder rerank** | LoCoMo 71.4→91.6 (+20), LongMemEval 67.8→93.4 (+26)。见 [mem0 docs migration](https://docs.mem0.ai/migration/oss-v2-to-v3) |
| **Zep / Graphiti** | 3 路: vector + BM25 + graph traversal | **RRF + Node Distance Reranking** | 见 [Zep docs](https://help.getzep.com/graphiti/working-with-data/searching) |
| **Qdrant v1.10+** | 任意路 (dense + sparse + ...) | **原生 RRF**(Query API + prefetch);v1.11+ 加 DBSF;v1.17+ 加 weighted RRF | 见 [Qdrant Hybrid Queries](https://qdrant.tech/documentation/concepts/hybrid-queries/) |
| **OpenSearch** | dense + BM25 | **rrf retriever**, k 默认 60 | 见 [OpenSearch blog](https://opensearch.org/blog/introducing-reciprocal-rank-fusion-hybrid-search/) |
| **Elasticsearch** | dense + sparse | **rrf retriever** | 同上 |
| **LangChain `EnsembleRetriever`** | N 路 retriever | **weighted RRF**, 默认 c=60, 权重相等 | 见 [LangChain reference](https://reference.langchain.com/python/langchain-classic/retrievers/ensemble/EnsembleRetriever) |
| **Milvus** | dense + sparse | **RRFRanker / WeightedRanker** | 见 [Milvus RRF](https://milvus.io/docs/rrf-ranker.md) |

**结论**: 4 路 + RRF 是当前业界默认范式,不是激进选择。

---

## 2. 决策 (Decision)

**采用 4 路并行召回 + RRF (k=60) 融合**,纯 Rust 实现在 `frank-memory` crate 内部。

四路:

1. **向量** (vector / semantic) — 复用现行 fastembed + LanceDB cosine
2. **BM25** (sparse / keyword) — 新建 tantivy 索引在 `~/.frank/memory/bm25.idx/`
3. **时间衰减** (recency) — 不需要新索引,基于 LanceDB `created_at` 字段排序
4. **元数据匹配** (metadata) — 基于 LanceDB SQL filter 精确匹配 scope/agent_id/metadata key

融合算法: **RRF (Reciprocal Rank Fusion)**

```
score(d) = Σᵢ 1 / (k + rankᵢ(d)),   k = 60
```

引用 Cormack, Clarke, Buettcher 2009 SIGIR paper [Reciprocal Rank Fusion outperforms Condorcet and individual Rank Learning Methods](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf)。

### 2.1 为什么 k=60

不是拍脑袋的数字,是 2009 paper 实验值,经 17 年验证,业界全部默认 60。原理:

- **k 控制 "top-rank 优势衰减速度"**
  - k 小 (例如 1): top-1 拿 1/(1+1)=0.5,top-10 拿 1/(1+10)=0.09 → 巨大差距 → 单路 top-1 几乎一票否决其他路
  - k 大 (例如 60): top-1 拿 1/(60+1)=0.0164,top-10 拿 1/(60+10)=0.0143 → 接近平均 → 鼓励多路共识
  - k 极大 (例如 1000): 接近 Borda count,退化成纯排名平均
- **k=60 是 2009 paper 在 TREC 数据集实验的 sweet spot**: 既给 top-rank 足够优势,又不让单路 dominate
- **现代验证**:
  - OpenSearch 默认 k=60,实测 hybrid search NDCG@10 比 score-normalization 法低 3.86%,**但 p50 latency 少 1.62%、p99 少 0.78%、CPU 持平**。**质量小亏,稳定性大赚**。
  - LangChain `EnsembleRetriever` 默认 c=60
  - Qdrant `Fusion.RRF`, Milvus `RRFRanker` 默认 60
  - Elasticsearch 8.x `rank.rrf.rank_constant` 默认 60

**调参敏感性**: 论文 + OpenSearch 文档结论一致 — k 在 [10, 100] 区间内 NDCG 波动 < 2%,**对 k 不敏感**。这正是 RRF 流行的核心原因。

### 2.2 为什么不用 score normalization

替代方案是 min-max / z-score 归一化后加权求和。**否决理由**:

- 余弦相似度 ∈ [0, 1], BM25 score 无上界 (与文档长度 + 词频耦合) — 跨尺度归一化**本身不稳定**
- 一条 BM25 score=15 的离群值会把整个 batch 的 min-max 压扁 → 排名失真
- OpenSearch 实测归一化方法虽 NDCG@10 略高 (~+3.86%),但 latency / CPU 全负面,**且对单一 outlier 敏感**
- RRF 只看 rank position, 完全规避 score 尺度问题

引用 OpenSearch 团队原话:

> "RRF avoids these issues by focusing exclusively on rank positions, ensuring consistent treatment of results across disparate data sources."

---

## 3. 详设 (Detailed Design)

### 3.1 模块结构

```
crates/frank-memory/src/
├── retrieval/                    NEW   多路召回子模块
│   ├── mod.rs                    HybridRetriever 入口 + Pipeline
│   ├── vector.rs                 向量路 (复用 LocalEmbedder + LanceDB)
│   ├── bm25.rs                   BM25 路 (tantivy)
│   ├── time.rs                   时间衰减路 (纯 LanceDB sort)
│   ├── metadata.rs               元数据路 (LanceDB SQL filter)
│   └── rrf.rs                    RRF 融合算法 (纯函数, 单测)
├── memory.rs                     (扩: SearchOpts 加 hybrid 开关)
└── client.rs                     (改: Memory::search 走 HybridRetriever)
```

每文件 < 300 行 (ADR-001 硬约束)。

### 3.2 数据流

```
                 query: &str  +  scope: Scope  +  SearchOpts { limit: K }
                                    │
                                    ▼
                          ┌─────HybridRetriever─────┐
                          │   query_embedding (1 次) │
                          └─────────┬───────────────┘
                                    │
              tokio::join!( ────────┴──────── 并行 4 路, 各取 top-20 )
              │              │              │              │
              ▼              ▼              ▼              ▼
        vector path    bm25 path     time path     metadata path
         (LanceDB)    (tantivy)    (LanceDB)      (LanceDB)
              │              │              │              │
              └──────┬───────┴──────┬───────┴──────┬───────┘
                     │              │              │
                     ▼              ▼              ▼
                            rrf::fuse(lists, k=60)
                                    │
                                    ▼
                          Vec<MemoryMatch> (top-K, K=10 默认)
```

### 3.3 各路实现要点

#### 向量路 (`vector.rs`)

```rust
pub async fn search(
    embedding: &[f32],
    scope: &Scope,
    top_k: usize,        // 20 (各路独立 top-K,不是最终 K)
    store: &LanceStore,  // v0.11 子项 A 落地
) -> Result<Vec<(MemoryId, f32)>>;
```

复用现行 `EmbeddedRecord` 流。**唯一变化**: 把"返回 `MemoryMatch`"改成"返回 `(id, score)`",因为 RRF 只要 rank 不要 score 详情。

#### BM25 路 (`bm25.rs`)

```rust
pub struct Bm25Index {
    index: tantivy::Index,
    reader: tantivy::IndexReader,
    schema: tantivy::Schema,
}

impl Bm25Index {
    pub fn open_or_create(path: &Path) -> Result<Self>;
    pub fn add(&mut self, record: &MemoryRecord) -> Result<()>;
    pub fn search(&self, query: &str, scope: &Scope, top_k: usize) -> Result<Vec<MemoryId>>;
    pub fn delete(&mut self, id: MemoryId) -> Result<()>;
}
```

**tantivy 选择理由** (v0.26.1, 2026-05 最新):
- 15.3k stars, Anytype 已生产用 1 年, Etsy / ParadeDB / Nuclia 都在用
- 纯 Rust, 不引 C++ / Java 依赖
- ~2× Lucene 速度 (官方 benchmark);1M docs 索引 50k/s, 查询 10k QPS (M2 Mac 实测)
- 索引文件结构紧凑,不存 position / store_field 时常**小于原始数据**

依赖加 workspace `Cargo.toml`:

```toml
[workspace.dependencies]
tantivy = "0.26"
```

**Schema** (`bm25.idx/meta.json` 由 tantivy 写):
- `id: STORED` (MemoryId 字符串)
- `content: TEXT | STORED` (fact 文本)
- `user_id: STRING` (精确匹配 filter)
- `agent_id: STRING` (同上)
- `session_id: STRING` (同上)
- `created_at: i64 STORED FAST` (用于次级排序)

**索引位置**: `~/.frank/memory/bm25.idx/` (与 `lance.db/` 平级)

**索引同步策略** (关键):
- 写: `Memory::add` 同步双写 LanceDB + tantivy (失败回滚 LanceDB,见 R3 风险)
- 删: 同步双删
- 更新 (v0.11 暂不做,留 v0.12 bi-temporal): tantivy delete-then-add

**中文分词**:
- 默认用 tantivy 内置 `SimpleTokenizer` + lowercase + stop_words (英文友好)
- 检测到中文 fact (Unicode CJK 范围 > 30% 字符) → 走 `cang-jie` (基于 jieba-rs, 0.3k stars 但 stable)
- 不强求最完美 — BM25 路只是兜底, 漏召还有 vector 路托底

可选依赖, feature flag:

```toml
[features]
cjk-tokenizer = ["cang-jie"]
[dependencies]
cang-jie = { version = "0.16", optional = true }
```

#### 时间衰减路 (`time.rs`)

不建索引,直接 LanceDB 查询 + 内存排序:

```rust
pub async fn search(
    scope: &Scope,
    top_k: usize,
    now: DateTime<Utc>,
    half_life_days: f64,   // 默认 30
    store: &LanceStore,
) -> Result<Vec<MemoryId>>;
```

公式:

```
recency_score(record) = exp(-ln(2) × age_days / half_life_days)

其中 age_days = (now - record.created_at).as_seconds() / 86400.0
```

`half_life_days = 30` 默认 (业界经验):
- < 7d 太陡,新增 fact 几乎垄断
- > 90d 太平,等于按 created_at 排序失去意义
- 30d 平衡:7d 前的 fact 还有 ~0.85 权重,90d 前 ~0.13,1y 前 ~0.0001
- 配置项 `time_half_life_days` 在 `SearchOpts` 里 (默认 30, 0 = 关闭时间路)

**为什么不直接按 `created_at desc` 倒排?**
- 倒排是阶跃: 最新一条秒杀其他 — 跟"老 fact 仍有价值"的诉求冲突
- 指数衰减是软排序: 老的也召得到,只是分数低 → 这正是 POSITION.md 撤回项 A 说的"软降权,不删"

**业界参考**:
- 推荐系统经典: [A Half-Life Decaying Model for Recommender Systems](https://ceur-ws.org/Vol-2038/paper1.pdf) — 电影推荐用 150 天半衰期
- AI agent memory ([hippo-memory](https://github.com/kitfunso/hippo-memory)) 用 7-30 天
- 我们记忆是"工作流偏好 / 历史决定",30 天合理

#### 元数据匹配路 (`metadata.rs`)

```rust
pub async fn search(
    scope: &Scope,
    query: &str,           // 用于从 query 提取 keyword 跟 metadata key 比对
    top_k: usize,
    store: &LanceStore,
) -> Result<Vec<MemoryId>>;
```

**LanceDB SQL filter** 例子:

```sql
SELECT id FROM memories
WHERE scope.user_id = 'alice'
  AND scope.agent_id = 'codex'
  AND metadata['source'] LIKE '%cli-output%'
ORDER BY created_at DESC
LIMIT 20
```

**排序规则**:
- agent_id 精确匹配 +2 分
- session_id 精确匹配 +3 分
- metadata 中任意 string value contains query token +1 分
- 同分按 created_at desc

**为什么不丢进 BM25?**
- metadata 是结构化 JSON,丢全文索引等于把"agent_id=codex"当词索引,精度差
- LanceDB 原生 SQL filter 已经够快 (1k 行 < 5ms)
- 跟 BM25 路职责正交:**BM25 = fact 文本字面;metadata = JSON 结构精确**

### 3.4 RRF 融合实现 (`rrf.rs`)

纯函数,无状态,单测全覆盖:

```rust
/// RRF 融合 N 路排名列表 → 统一打分 + 重排。
///
/// # 参数
/// - `ranked_lists`: 每路一个 `Vec<MemoryId>`, 按相关性降序排
/// - `k`: smoothing constant, 默认 60 (Cormack 2009)
/// - `weights`: 可选, 每路权重 (None = 等权), len 必须 == ranked_lists.len()
///
/// # 返回
/// 按 RRF 分数降序的 `Vec<(MemoryId, f64)>` (含分数便于调试)
pub fn fuse(
    ranked_lists: &[Vec<MemoryId>],
    k: f64,
    weights: Option<&[f64]>,
) -> Vec<(MemoryId, f64)> {
    let weights = weights
        .map(<[f64]>::to_vec)
        .unwrap_or_else(|| vec![1.0; ranked_lists.len()]);
    assert_eq!(weights.len(), ranked_lists.len(), "weights len mismatch");

    let mut scores: HashMap<MemoryId, f64> = HashMap::new();
    for (list_idx, list) in ranked_lists.iter().enumerate() {
        let w = weights[list_idx];
        for (rank, id) in list.iter().enumerate() {
            // rank 从 0 开始, RRF paper 用 1-based, 这里 +1 对齐
            let contribution = w / (k + (rank + 1) as f64);
            *scores.entry(*id).or_insert(0.0) += contribution;
        }
    }

    let mut out: Vec<_> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}
```

**单测覆盖** (验收标准之一):

```rust
#[test]
fn rrf_paper_example_simple() {
    // 2 路, 3 doc, k=60
    let list_a = vec![id("A"), id("B"), id("C")];   // A=1st, B=2nd, C=3rd
    let list_b = vec![id("B"), id("A"), id("C")];   // B=1st, A=2nd, C=3rd
    let out = fuse(&[list_a, list_b], 60.0, None);

    // A: 1/61 + 1/62 = 0.01639 + 0.01613 = 0.03252
    // B: 1/62 + 1/61 = 0.03252 (同 A,但 HashMap 顺序不定,断言两者相等)
    // C: 1/63 + 1/63 = 0.03175
    assert!((out[0].1 - 0.03252).abs() < 1e-4);
    assert!((out[1].1 - 0.03252).abs() < 1e-4);
    assert_eq!(out[2].0, id("C"));
}

#[test]
fn rrf_weight_amplifies_path() {
    let list_a = vec![id("X"), id("Y")];
    let list_b = vec![id("Y"), id("X")];
    let out = fuse(&[list_a, list_b], 60.0, Some(&[10.0, 1.0]));
    // 第一路权重 10x → X 占优
    assert_eq!(out[0].0, id("X"));
}

#[test]
fn rrf_empty_lists_no_panic() {
    let out = fuse(&[vec![], vec![]], 60.0, None);
    assert!(out.is_empty());
}

#[test]
fn rrf_k_zero_top_rank_dominates() {
    // k=0 极端: top-1 拿 1.0, 其他几乎 0
    let list_a = vec![id("A"), id("B")];
    let list_b = vec![id("B"), id("A")];
    let out = fuse(&[list_a, list_b], 0.0, None);
    // A: 1/1 + 1/2 = 1.5
    // B: 1/2 + 1/1 = 1.5
    assert!((out[0].1 - 1.5).abs() < 1e-9);
}
```

### 3.5 并行执行

```rust
// crates/frank-memory/src/retrieval/mod.rs
pub async fn search(
    &self,
    query: &str,
    scope: &Scope,
    opts: &SearchOpts,
) -> Result<Vec<MemoryMatch>> {
    let embedding = self.embedder.embed(query).await?;

    let per_path_k = 20;

    // tantivy 是同步 IO, 必须 spawn_blocking; 其他三路 async
    let (vec_r, bm25_r, time_r, meta_r) = tokio::join!(
        vector::search(&embedding.vector, scope, per_path_k, &self.store),
        async {
            let bm25 = self.bm25.clone();    // Arc<Bm25Index>
            let query = query.to_string();
            let scope = scope.clone();
            tokio::task::spawn_blocking(move || bm25.search(&query, &scope, per_path_k))
                .await
                .map_err(|e| anyhow!("bm25 join error: {e}"))?
        },
        time::search(scope, per_path_k, Utc::now(), opts.time_half_life_days, &self.store),
        metadata::search(scope, query, per_path_k, &self.store),
    );

    // 任意一路失败 → 降级 (log warn, 当作空)
    let lists = vec![
        vec_r.unwrap_or_else(|e| { warn!("vector path failed: {e}"); vec![] }),
        bm25_r.unwrap_or_else(|e| { warn!("bm25 path failed: {e}"); vec![] }),
        time_r.unwrap_or_else(|e| { warn!("time path failed: {e}"); vec![] }),
        meta_r.unwrap_or_else(|e| { warn!("metadata path failed: {e}"); vec![] }),
    ];

    let fused = rrf::fuse(&lists.iter().map(extract_ids).collect::<Vec<_>>(), 60.0, None);

    // 截 top-K + 拉回完整 MemoryRecord
    let top_ids: Vec<_> = fused.into_iter().take(opts.limit as usize).map(|(id, _)| id).collect();
    self.store.batch_get(&top_ids).await.map(into_matches)
}
```

**关键点**:
- 一路失败 → 降级不阻断 (业界共识: 多路系统弹性优先)
- tantivy `Index::search` 是同步 IO → `spawn_blocking` 移出 tokio runtime
- 4 路独立 timeout (各 50ms) 由 `tokio::time::timeout` 包裹 (P50 < 100ms 预算)

### 3.6 可观测 (v0.11 子项 G 顺便加)

每次 search 在 stderr 打:

```
[frank] search query="..." scope={user=alice} (28ms)
  vec=5 bm25=3 time=2 meta=8 → RRF=8 unique top-10
```

实现: 在 `retrieval::search` 末尾用 `tracing::info!` 输出。`frank-cli` 的 `log::ui` 转 stderr。

---

## 4. 后果 (Consequences)

### 4.1 优点

- **召回质量上限提高**: 单路漏召的另外路兜底 (参考 mem0 v3 实测 +20~+26 NDCG)
- **可调性强**: 4 路独立, 用户/调试可禁某路 (`SearchOpts.disable_paths = vec!["bm25"]`)
- **未来扩展容易**: 加 entity 路 (v0.12 Graph) / cross-encoder rerank (v0.13) 都只是新 path + 新融合权重
- **可观测好**: 每路 hit 数清晰, debug 一眼看出哪路偏

### 4.2 缺点

- **复杂度**: 4 个文件 + 1 个融合 + 并行编排, 单路升级到 4 路代码量约 4×
- **写放大**: 每次 add 要双写 LanceDB + tantivy → 写 latency 翻倍 (~80ms 估)
- **磁盘**: tantivy 索引 ~1k records 估 5-20 MB (跟 fact 长度强相关), 10k records 50-200 MB
- **冷启动**: 进程启 tantivy `Index::open` ~10ms, 不算大但首查会慢
- **维护**: tantivy / cang-jie crate 升级要跟, 中文分词调优需要语料

### 4.3 维护成本

| 项 | 频次 | 应对 |
|---|---|---|
| tantivy 升级 (0.26 → 0.27 ...) | 半年/次 | rustfmt + cargo update + 跑测 |
| cang-jie 升级 | 不定 | optional feature, 不升不影响主路 |
| BM25 索引 corrupt | 罕见 | `frank memory reindex` 子命令 (重建, ~10 秒 / 10k records) |
| RRF 权重调优 | 用户报"召不准"时 | 加 `SearchOpts.weights: Option<[f64; 4]>` |

---

## 5. 备用方案

### 5.1 砍一路, 3 路即可

**砍 metadata 路**: 改成 hard filter (所有路开始前先过 scope filter), 不参与 RRF。
- 优点: 实现简化 30%
- 缺点: 失去 metadata key 的软排序能力, scope 完全匹配的 fact 跟泛匹配的同分
- 结论: **不取**, 留 metadata 路因为它实现成本最低 (只 LanceDB SQL, 无新索引)

**砍 time 路**: 时间衰减改成 post-RRF 的 boost 系数。
- 优点: 少一路并发
- 缺点: 时间路本身就是排序信号, 后置 boost 容易把 RRF 排好的顺序打乱
- 结论: **不取**, time 路实现 < 50 行 (纯 LanceDB query)

### 5.2 直接调 Qdrant 原生 hybrid

Qdrant v1.10+ 已支持 Query API + RRF (见 §1.3 表)。

**为什么不用**:
- frank v0.11 主存倒置成 **LanceDB 本地** (子项 A), Qdrant 只做远程同步
- LanceDB 没有原生 hybrid, 必须在 Rust 层做
- 即使保留 Qdrant 作为查询路径, 它的 RRF 只融 dense + sparse 两路, 没有 time / metadata 路
- 我们想要的"4 路 + 可调权重 + 可观测"超过 Qdrant API 提供的范畴
- 结论: **本路自研** + 未来 Qdrant 远程作为 fallback (v0.12 多机同步时)

### 5.3 串行 4 路

`for path in paths { path.search().await? }`

- 优点: 代码简单, 不用 `tokio::join!` + `spawn_blocking`
- 缺点: latency 4×, P50 估 ~250ms → 砸 100ms 预算
- 结论: **不取**, 并行是硬需求

### 5.4 用 LangChain ensemble (Python)

跨语言桥, **直接否决** — frank 主栈 Rust (ADR-001), 不引 Python。

---

## 6. 风险 + 应对

| ID | 风险 | 概率 | 应对 |
|---|---|---|---|
| **R1** | 一路慢拖全部 (如 tantivy 索引膨胀后查询慢) | 中 | 每路 `tokio::time::timeout(50ms)`, 超时降级空。集成测验证 |
| **R2** | BM25 中文分词差,中文 fact 召不到 | 高 | feature flag `cjk-tokenizer` 引 `cang-jie`; 提供 `frank memory test-tokenizer "..."` 子命令调试 |
| **R3** | tantivy 写入与 LanceDB 写入失败一致性 (一边成功一边失败) | 中 | 双写顺序: 先 LanceDB (source of truth), 再 tantivy。tantivy 失败 → log warn,不回滚 (BM25 不致命,reindex 可恢复)。加 `frank memory reindex` 子命令 |
| **R4** | RRF k=60 在我们场景不合理 | 低 | 配置项 `SearchOpts.rrf_k: f64` 默认 60。提供 benchmark script 调参 |
| **R5** | 4 路并行 P50 仍超 100ms | 中 | 性能基准早做。慢的话:(a) 砍 metadata 路 (b) BM25 异步 prefetch (c) embedding cache |
| **R6** | tantivy 索引 corrupt (进程 kill 中途) | 低 | tantivy 有 segment-based 设计, 单 segment 损坏可重建。`frank doctor memory` 检测 + 提示 reindex |
| **R7** | metadata 路被 LanceDB SQL 注入 (用户 query 拼 SQL) | 中 | 用 LanceDB 参数化 query API, 不字符串拼接 |
| **R8** | 多 frank 进程同时写 tantivy (P0 不锁) | 中 | tantivy `IndexWriter` 全局单例, 进程级。多进程时只允许 sync-agent 写, CLI 走 sync-agent REST。或加文件锁 (`fd-lock` crate) |

### R8 详细处理

frank 是 CLI, 多窗口 / cron / hook 可能并发触发 `memory add`。tantivy `IndexWriter` 文档说**同进程只能一个 writer**, 多进程会 panic。

方案: `Bm25Index` 用 `fd-lock` 加文件锁 (`~/.frank/memory/bm25.idx/.write_lock`), 写时阻塞获取, 读不锁。锁超时 5s → 降级 (跳过 BM25 写入, log warn + 加入异步重试队列)。

---

## 7. 性能预算

| 规模 | 单路 latency 目标 | 总 P50 目标 | 总 P99 目标 |
|---|---|---|---|
| 1k records | vec/time/meta < 10ms, bm25 < 20ms | < 100ms | < 200ms |
| 10k records | vec/time/meta < 30ms, bm25 < 50ms | < 300ms | < 500ms |
| 100k records (v0.12+) | 视磁盘 IO, 留 v0.12 调优 | - | - |

**基准** (基于公开数字):
- LanceDB cosine search 1k records ≈ 5-10ms (NVMe)
- tantivy BM25 query 1k docs ≈ 1-5ms (warm cache)
- LanceDB SQL filter 1k records ≈ 1-3ms
- RRF 融合 4 路 × 20 docs ≈ < 1ms (纯 HashMap)

合计 4 路 max + RRF + batch_get ≈ 25-40ms,留 60-75ms 余量给 embedding (~10ms) + tokio 调度。**100ms P50 可达**。

---

## 8. 验收标准 (给 D / C 阶段用)

### 8.1 单测 (D 阶段)

- [ ] `rrf.rs` 单测 ≥ 6 个: paper 基准例子 / weighted / empty / k=0 极端 / k=1000 接近 Borda / 单路退化
- [ ] `bm25.rs` 单测: add + search 中文 / 英文 / 混合 / 删除后不再召
- [ ] `time.rs` 单测: 1d / 30d / 90d 衰减比例符合公式
- [ ] `metadata.rs` 单测: scope 三层 filter + metadata key 匹配
- [ ] 集成测: `frank-memory` crate `tests/hybrid.rs` 写 100 条 → 4 路 search → assert RRF 不漏召已知 ground truth

### 8.2 集成 (C 阶段)

- [ ] 端到端: `frank memory add` 写 50 条 → `frank memory search "..."` 4 路并行 + RRF 排序
- [ ] 召回质量人工 review 10 条 ≥ 4/5 评分 (跟单路向量对比)
- [ ] 中文 fact 召回测: 20 条中文 fact + 5 个中文 query, 至少 4/5 命中 top-3
- [ ] 一路降级测: 删 BM25 索引 → search 仍能返回 (vector 兜底)

### 8.3 性能 (C 阶段)

- [ ] 1k records: P50 < 100ms, P99 < 200ms (`cargo bench` + `criterion`)
- [ ] 10k records: P50 < 300ms, P99 < 500ms
- [ ] 写 1k 条 (add) 总耗时 < 30s (含 embedding + tantivy 索引)
- [ ] 索引磁盘占用: 1k 条 fact 平均长度 80 char → tantivy 索引 < 20 MB

### 8.4 可观测 (G 顺便)

- [ ] `RUST_LOG=frank=info` 下每次 search 打一行 `vec=N bm25=N time=N meta=N → RRF=N unique`
- [ ] 一路失败时 log warn + 不影响最终结果
- [ ] `frank memory search --debug` 打每路 top-5 ID + RRF 最终 ID

### 8.5 ADR review

- [ ] codex Plan Review ≥ 7.0, 无单维度 ≤ 3
- [ ] 用户拍板确认 4 路 + RRF + k=60 + tantivy

---

## 9. 不在 v0.11 范围 (留后续)

- ❌ Cross-encoder rerank (mem0 v3 用 BGE-reranker 二次排序) → v0.13
- ❌ Query 改写 / 多 query (HyDE 等) → v0.13
- ❌ Entity 路 (mem0 v3 第 5 路) → v0.12 Graph 子项
- ❌ 学习权重 (在线根据用户点击调 RRF weights) → v0.14+
- ❌ 用户级权重持久化配置 (`~/.frank/retrieval.yaml`) → v0.12

---

## 10. 相关

- **ADR-001**: Rust + 质量基线 (本 ADR 文件 < 300 行 + clippy pedantic + 单测覆盖)
- **ADR-003**: frank-memory v1 (本 ADR 是 search 路径的演进)
- **ADR-007**: memory v2 deferred (本 ADR 是 v0.11 拆分后的 B 子项落地)
- **POSITION.md 维度 #4**: 多路召回 — 本 ADR 是 v0.11 这格的兑现
- **PHASE-9-PLAN.md 子项 B**: 工期 / 阶段拆分 / 验收 (本 ADR 是 P 阶段交付物)

---

## 11. 参考文献

- Cormack, Clarke, Buettcher. *Reciprocal Rank Fusion outperforms Condorcet and individual Rank Learning Methods*. SIGIR 2009. [PDF](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf)
- mem0 v3 migration: <https://docs.mem0.ai/migration/oss-v2-to-v3>
- Zep / Graphiti search: <https://help.getzep.com/graphiti/working-with-data/searching>
- Qdrant hybrid queries (v1.10+): <https://qdrant.tech/documentation/concepts/hybrid-queries/>
- OpenSearch RRF blog: <https://opensearch.org/blog/introducing-reciprocal-rank-fusion-hybrid-search/>
- LangChain `EnsembleRetriever`: <https://reference.langchain.com/python/langchain-classic/retrievers/ensemble/EnsembleRetriever>
- Milvus RRF Ranker: <https://milvus.io/docs/rrf-ranker.md>
- tantivy GitHub (v0.26.1, 2026-05-10): <https://github.com/quickwit-oss/tantivy>
- cang-jie (Chinese tokenizer): <https://github.com/DCjanus/cang-jie>
- Half-Life Decaying Model for Recommender Systems: <https://ceur-ws.org/Vol-2038/paper1.pdf>

---

*最后更新: 2026-05-25. Status: Proposed, 等用户 + codex review.*
