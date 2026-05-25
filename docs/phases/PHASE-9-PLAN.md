# PHASE-9 计划: v0.11.0 — 真正的差异化 (本地主存 + 多路召回 + 三类记忆)

> **本文必须对照 `docs/POSITION.md` 13 维度 + 3 独家定位 + 撤回项校验。**
> 任何脱离定位文档的"全面性"需求都得先回去 review POSITION.md。

---

## 0. 战略反省 (2026-05-25 用户提醒后)

**v0.1-v0.10 全部花在: 地基 + 体验补漏。** 102 个 task 里 0 个是真正的"比 mcp__memory 强"的差异化。

**v0.11 不能再偏。** 这一版就一个目标:

> 让用户能说"frank-memory 比 mcp__memory 强,因为它有 ___" — 这个 ___ 必须落到代码。

按 POSITION.md 第 0 优先级是"**多设备同步**",但纯多设备 = 工程量大 + 端到端验证难。
**先做 v0.11 的几样能在单机就证明价值的东西** → 验证 → 再上 v0.12 的多设备。

---

## 1. 范围 (按 POSITION.md → v0.11 行)

POSITION.md `v0.11.0` 这行原话:

> 本地 LanceDB 主存倒置 + 多路召回 4 路 + extractor auto-detect + **三类记忆** + **三层 session** + 可观测细化 + 软降权检索 + 手动 cleanup

拆成 8 个独立子项,**砍掉 / 合并 / 排序** 如下:

| # | 子项 | POSITION 维度 | 是否做 | 工期 | 价值 |
|---|---|---|---|---|---|
| A | **本地 LanceDB 主存** | #1 存储倒置 | ✅ 做 | 2d | ⭐⭐⭐ 决定后续召回/缓存基础 |
| B | **Hybrid Retrieval 4 路 + RRF** | #4 多路召回 | ✅ 做 | 3d | ⭐⭐⭐⭐⭐ 召回质量直接体感 |
| C | **三层 session (Core/Recall/Archival)** | #7 | 🟡 减配 → 仅 Core+Archival 两层 | 1d | ⭐⭐ Recall 复杂度高 / 价值边际 |
| D | **三类记忆 (semantic/episodic/procedural)** | #6 | 🟡 减配 → semantic+episodic 两类 | 1d | ⭐⭐ procedural 留 v0.12 |
| E | **extractor auto-detect** | #2 抽取 | ✅ 做 | 1d | ⭐⭐⭐ codex 用户走 codex 抽 |
| F | **手动 frank memory cleanup + 检索软降权** | 撤回项 A 替代 | ✅ 做 | 1d | ⭐⭐ 长期数据增长应对 |
| G | **可观测细化 (召回路径 / cache hit)** | #12 | ✅ 做 (随做 B/A 加) | 0.5d | ⭐⭐ debug 必备 |
| H | **PostToolUse hook 截 mcp__memory** | POSITION v0.12 (#10) | 🟡 **提前到 v0.11**? | 2d | ⭐⭐⭐⭐ 真正比 mcp_memory 强的零成本切换 |

### 我的执行建议

**最小可证明价值的子集 (推荐):**
- **A** (LanceDB 本地) → 基础设施,后面全靠它
- **B** (4 路召回 + RRF) → 这是直接看得见的"召回质量比 mcp_memory 强"
- **E** (extractor auto-detect) → 跟 v0.10.8 已做的"动态模型加载"呼应
- **H** (PostToolUse hook) → 让 Claude Code 用户**无痛切换** (老 mcp_memory 习惯不变)
- **G** (可观测) — 随 A/B 加,不单独排工

**剩下 (D/C/F) 留 v0.12 一起做:**
- D (三类记忆) → 跟 v0.12 的 Graph 一起,因为 episodic 跟 Graph 强耦合
- C (三层 session) → 跟 v0.12 多设备一起,session 跨机才有意义
- F (cleanup) → 等真数据多了再做,现在数据少没必要

**v0.11 范围 = A + B + E + H + G**, 工期 **6-7 天**。

---

## 2. 子项详设 (P/D/C/A — 启动/规划/执行/收尾)

### A. 本地 LanceDB 主存 (2d)

**P 详设要点:**
- LanceDB Rust crate (`lancedb` 或 `lance`,选 stable 那个)
- 数据存 `~/.frank/memory/lance.db/`
- 表结构: 一张 `memories` 表,字段 = `MemoryRecord` (id/scope/fact/embedding/metadata/created_at)
- embedding 沿用现行 `LocalEmbedder` (fastembed BGE-small 384d)

**D 实现:**
1. `crates/frank-memory/src/local_store.rs` — LanceDB 操作封装 (add/search/list/delete)
2. `Memory` 高层 API 改: 写 → 双写 (local LanceDB + remote Qdrant via sync-agent)
3. 读 → 优先 local, fallback remote (POSITION #1 "本地主存 远程辅助")
4. 标志位 `sync_status` (synced / pending / failed) 决定后续同步

**C 测试:**
- 单测: local store roundtrip
- 集成: `frank memory add` 写本地 + sync-agent 转,`frank memory list` 优先本地
- 性能: 1k records 检索 P50 < 50ms

**A 风险:**
- LanceDB Rust 生态成熟度 — 先查 GitHub stars + issue 数
- 文件锁: 多 frank cli 进程同时读写 LanceDB 是否安全 → 调研
- 备用: 如果 LanceDB 坑太多,fallback 用 SQLite + 简单向量字段

---

### B. Hybrid Retrieval 4 路并行 + RRF (3d) — 🔥 最关键

**P 详设要点:**
- 4 路:
  1. **向量** (语义,fastembed embedding cosine)
  2. **BM25** (关键词,tantivy 全文检索)
  3. **时间衰减** (created_at 倒序权重)
  4. **元数据匹配** (scope / agent / metadata json 精确)
- 4 路各取 top-K (K=20)
- **RRF (Reciprocal Rank Fusion)** 融合: 每个 doc 的最终 score = Σ 1/(60 + rank_i)

**D 实现:**
1. `crates/frank-memory/src/retrieval/` 新建子模块
2. `vector_search.rs` — 走现行 LocalEmbedder + LanceDB
3. `bm25_search.rs` — tantivy crate, 索引建在 `~/.frank/memory/bm25.idx/`
4. `time_decay.rs` — 简单时间排序 (不需要索引)
5. `metadata_filter.rs` — LanceDB SQL filter
6. `rrf.rs` — 融合算法 (纯函数, 单测覆盖)
7. `Memory::search` 改: 并行调 4 路 (tokio join + spawn_blocking 给 BM25), RRF 融合

**C 测试:**
- 单测: RRF 公式正确性 (基准 paper 例子)
- 集成: 同 query 在 4 路 vs 单路向量 → 评估召回质量 (人工 review 10 条)
- 性能: 1k records, 4 路并行 P50 < 100ms

**A 可观测 (G 顺便加):**
```
[frank] search query="..." (28ms) — vec=5 bm25=3 time=2 meta=8 → RRF=8 unique
```

---

### E. extractor auto-detect (1d)

**P 详设要点:**
- 当前: `frank-cli` 抽 fact 时永远调 claude haiku (写死)
- 改: 探用户当前正在用的 cli (跟 v0.10.8 的 sources 模块呼应)
- 优先级: 用户指定 `--extractor=codex` > 当前 shell 的 active cli (`FRANK_AI_PROVIDER` env) > 检测到的第一个登录的 cli > claude haiku fallback

**D 实现:**
1. `crates/frank-cli/src/cli/memory/add.rs` 改: 在调 LLM 抽 fact 前先探可用 provider
2. 复用 `sources::detect_active_provider()` (v0.10.8 已有)
3. 传给 extract step
4. extractor prompt 跟当前 fact 抽 prompt 一致 (各家 cli 都接 stdin 不耦合)

**C 测试:**
- 单测: auto-detect 优先级链
- 集成: 配 codex 但没 claude → 抽 fact 走 codex; 都配 → claude 优先 (默认)

---

### H. PostToolUse hook 截 mcp__memory (2d) — 提前到 v0.11

**P 详设要点:**
- Claude Code 支持 `hooks/post_tool_use.sh` (在工具调用后执行)
- 注册:在 `~/.claude/settings.json` 加 `hooks.post_tool_use` 指向脚本
- 脚本判断: 如果是 `mcp__memory__*` 工具 → 把同样的数据双写到 frank-memory
- 用户无需切换 (mcp__memory 仍跑,frank-memory 默默累积)

**D 实现:**
1. `frank install hook --post-tool-use` 子命令 — 一行写好 settings.json
2. 实际 hook 脚本: `~/.frank/hooks/post-tool-use.sh` (用 frank-cli 包装)
3. 脚本读 stdin (Claude Code 传的工具调用 JSON) → 检测 `mcp__memory__add_observations` → 调 `frank memory add_raw`
4. 失败不阻断 (用户 mcp_memory 体验不受影响)

**C 测试:**
- 单测: hook 脚本 JSON 解析
- 集成: 模拟 PostToolUse 事件 → frank memory list 看到双写记录
- 真测: 让 Claude Code 真跑一次,看 hook 触发

**A 风险:**
- Claude Code hook 协议: 输入输出格式可能 v0.x 阶段变 — 写 ADR 记录 spec 版本
- 性能: hook 串行调用 → 慢的话 Claude Code 体验差 → frank-cli 必须 < 100ms exit

---

## 3. 执行顺序 + 多 Agent 并行

**Wave 1 (并行)** — Day 1-2:
- Agent 1: A 详设 (LanceDB 调研 + 数据模型) + Agent 2: B 详设 (tantivy + RRF paper)
- 同时跑 P 阶段,互不依赖

**Wave 2 (并行)** — Day 3-4:
- Agent 1: A 实现 (LanceDB 接入)
- Agent 2: H 实现 (hook 脚本 + frank install hook 子命令) — 跟 A 完全独立

**Wave 3 (并行)** — Day 5-6:
- Agent 1: B 实现 (4 路 + RRF, 基于 A 的 LanceDB)
- Agent 2: E 实现 (extractor auto-detect)

**Wave 4 (串行)** — Day 7:
- 端到端测 (frank memory add → 看 4 路召回 → hook 验证)
- bump v0.11.0 + tag + push
- Formula bump

**关键路径**: A → B (B 依赖 A)
**独立线**: E, H (任何时候都能插)

---

## 4. PDCA 检查点

| 阶段 | 必交付物 | 必通过 |
|---|---|---|
| P 启动 | `docs/ADR/010-lancedb-store.md` (A) + `docs/ADR/011-hybrid-retrieval.md` (B) | codex review ≥ 7.0 |
| D 实施 | 代码 + 单测 + cargo clippy 0 warnings | 222+ tests pass |
| C 检查 | 端到端真测 + 性能基准 + 召回质量人工 review | P50 < 100ms / 召回质量评分 ≥ 4/5 |
| A 收尾 | release 笔记 + brew upgrade + frank.hutiefang.com 部署 | 用户真用一周 |

---

## 5. 风险 / 撤回项的红线

按 POSITION.md 撤回项,以下不许做:
- ❌ 自动淘汰 (LFU/LRU/Agentic) — 替代方案是 F (手动 cleanup + 软降权,留 v0.12)
- ❌ pricing 表 / cost 计算 — 只显 token+latency
- ❌ token 预算预估 — 只显已花

新风险:
- **LanceDB Rust 不成熟** → 调研先行,有备胎
- **hook 协议不稳** → 写 ADR + spec 版本字段
- **多路召回慢** → 性能基准要早做,慢就降级 (砍 BM25 路只留 3 路)

---

## 6. 不在本 phase 的事 (明确划线)

留给 v0.12:
- 多设备同步强化 (LWW)
- device token 解耦
- 轻量 Graph (haiku 本地)
- MCP server 协议兼容 (frank-mem MCP)
- procedural memory (LangMem 第 3 类)
- 三层 session 完整版

留给 v0.13:
- 时间维度 bi-temporal
- Windows 真测

---

*最后更新: 2026-05-25 (v0.10.10 ship 之后, v0.11 启动前). 改动需要 PR + 用户 review.*
