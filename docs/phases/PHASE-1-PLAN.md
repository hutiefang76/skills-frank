# Phase 1 PLAN — v0.10.5 token/cost/latency 可观测性

| Field | Value |
|---|---|
| **Phase** | 1 (PDCA Plan 阶段完成) |
| **Version** | v0.10.5 |
| **Estimated** | 0.5 工作日(V2 调整后,去 gemini/opencode) |
| **Agent verdict** | **GO**(agent `afd3efe7f5a2e0e8b`;V2 用户决策只做 claude/codex) |
| **Key insight** | claude/codex 都有 native JSON 5/5 可靠;gemini/opencode 留 future(用户实际不用) |
| **V2 修订** | 用户反馈"主流是 claude/codex,gemini 暂不支持" → 删 gemini fallback + opencode 二次调用 → 风险 R-P1.1/R-P1.3 自动消失 |

## 用户需求映射

| Q 编号 | 用户原话 | 本阶段实现 |
|---|---|---|
| Q5 | "打印 frank 记忆/本地/云端/token 消耗" | `frank memory list/add/search` 输出 CallReport 单行 stderr |
| Q2.1 | "ask 调其他模型输出 token + 模型类型" | `frank ai ask` 输出 CallReport 单行 stderr,4 CLI 各家解析 |

## 技术方案

### 1. 4 CLI 实测提取方式

| CLI | flag | 输出 | 可靠性 | fallback |
|---|---|---|---|---|
| **claude** | `--print --output-format json` | 单行 JSON `{usage:{input_tokens,output_tokens,cache_*}, total_cost_usd, duration_ms, session_id}` | **5** | n/a 官方 |
| **codex** | `exec --json --skip-git-repo-check` | JSONL,**末事件** = `{type:"turn.completed", usage:{input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens}}` | **5** | 无 cost 字段,查 pricing 算 |
| **gemini** | `--prompt - --output-format json` | 单行 JSON,token 字段**未实测**(无 KEY) | **3** | `(prompt+response chars)/4` char-count + `confidence:low` |
| **opencode** | `run --format json` + `opencode export <sessionID>` | 两次调用,`export` 返回 `{info:{model,cost,tokens:{input,output,reasoning,cache}}}` | **4** | 先 parse `run` JSONL `step-finished`,fallback `export` |

**决策**: 不再 hot-path 重复调用,opencode 二次 export 走 local sqlite(~5ms,可接受)。

### 2. 2026 定价表(agent 已 fetch live)

文件: `crates/frank-cred/data/pricing-2026-05.json`(`include_str!` 嵌入二进制) + `~/.frank/pricing.json`(用户覆盖)。

| Model | input $/1M | output $/1M | Confidence |
|---|---|---|---|
| claude-opus-4-7 | 5.00 | 25.00 | high |
| claude-sonnet-4-6 | 3.00 | 15.00 | high |
| claude-haiku-4-5 | 1.00 | 5.00 | high |
| gpt-5.5 | 5.00 | 30.00 | **med** (OpenAI 官页 403,3 secondary 一致) |
| gpt-5.5-pro | 30.00 | 180.00 | med |
| gemini-3.1-pro-preview-200k | 2.00 | 12.00 | high |
| gemini-2.5-flash | 0.30 | 2.50 | high |
| text-embedding-3-small | 0.02 | 0.00 | high |
| qwen3.6 (via opencode) | 0.40 | 1.20 | **low** (proxy varies) |
| `unknown_model` (fallback) | 5.00 | 25.00 | low |

### 3. CallReport struct

**位置: `crates/frank-cred/src/report.rs`**(新模块)— 理由:
- 已有 `InjectReport` 在 frank-cred,自然延伸
- frank-cli + frank-orchestrator + frank-memory 都 depend on frank-cred
- 无需新建 `frank-observability` crate(premature)

```rust
pub struct CallReport {
    pub provider: String,           // "claude" | "codex" | "gemini" | "opencode" | "openai-embedding"
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,     // None = pricing 表无该 model
    pub latency_ms: u64,
    pub session_id: Option<String>,
    pub source: CallSource,
    pub timestamp: DateTime<Utc>,
    pub confidence: Confidence,    // High/Med/Low
}

pub enum CallSource {
    SpawnedCli { bin: String },
    LocalCache,
    RemoteQdrant { endpoint: String },
    EmbeddingApi { endpoint: String },
}

impl CallReport {
    pub fn render_oneline(&self) -> String { /* [frank-call] claude/sonnet-4-6 in=1234 out=567 cost=$0.0098 t=1250ms sid=e51d57c8 */ }
}
```

**渲染规则**:
- `cost >= 0.0001` → `${:.4}` (4 位小数)
- `cost < 0.0001` → `<$0.0001`
- 永不科学计数法
- `Option<f64>` 区分"模型未知不算价" vs "真免费"

### 4. 模块切分

```
crates/frank-cred/
├── data/
│   └── pricing-2026-05.json      NEW  (~2KB, include_str! 嵌入)
├── src/
│   ├── report.rs                 NEW  (~150 LOC, CallReport + render_oneline)
│   └── pricing.rs                NEW  (~80 LOC, parse JSON + lookup + override path)

crates/frank-cli/src/cli/
├── ai.rs                          MODIFY  (parse claude/codex/gemini/opencode JSON,emit CallReport)
└── memory/handlers.rs             MODIFY  (frank-memory CallReport 渗透打印)

crates/frank-memory/src/
├── client.rs                      MODIFY  (返回 (result, CallReport))
└── embed/openai.rs                MODIFY  (HTTP 后构造 CallReport)
```

## 风险 + 缓解(agent 5 条)

| ID | 风险 | 严重性 | 缓解 |
|---|---|---|---|
| R-P1.1 | Gemini token 字段未实测 | **HIGH** | char-count fallback + `confidence:low` + doc-test 仅 `FRANK_E2E_GEMINI_KEY` 设时跑 |
| R-P1.2 | 2026 中定价漂移 | **HIGH** | bundle JSON 不写 const,`~/.frank/pricing.json` 可 override,doctor 警告 >180 天 |
| R-P1.3 | opencode 两次调用依赖 sqlite | MED | parse `run` JSONL 优先,fallback `export`,双 fail 不阻塞 |
| R-P1.4 | claude CLI 版本 skew | MED | `claude --version` cache,1.x/2.x 各一 parser,fail-soft 仍输出 reply |
| R-P1.5 | 小数渲染 polluted stderr | MED | `${:.4}` 4 位 clamp + `<$0.0001` 兜底,unit test edge cases |

## sub-task 拆解(D 阶段)

| Sub | 内容 | 工期 |
|---|---|---|
| D1 | 新建 frank-cred/data/pricing-2026-05.json + pricing.rs(parse+lookup+override) | 0.2d |
| D2 | 新建 frank-cred/src/report.rs(CallReport + render_oneline + 单测) | 0.3d |
| D3 | frank-cli/cli/ai.rs:per-CLI parser(claude json / codex JSONL / gemini fallback / opencode 双调) | 0.3d |
| D4 | frank-memory/client.rs:CallReport 渗透(embed/extract HTTP 后构造) | 0.2d |

**合计 1.0d**(agent 估 1d,一致;3 风险 mitigation 在 PR 内消化)。

## C 阶段 ship gate

- [ ] cargo test/clippy/fmt/docs 全过(单测覆盖 4 parser + pricing lookup + render 边界)
- [ ] **端到端真测**:`frank ai ask --to claude "say hi"` 输出含
      `[frank-call] claude/claude-sonnet-4-6 in=N out=N cost=$0.00XX t=Nms sid=...`
- [ ] **端到端真测**:`frank ai ask --to codex "say hi"` 同上(codex 解析)
- [ ] **端到端真测**:`frank memory search "test"` 输出含
      `[frank-call] openai-embedding/text-embedding-3-small in=N cost=$0.0000X t=Nms`
- [ ] gemini/opencode 因无 KEY 不强测,但 unit test 覆盖 parser

## A 阶段 ship gate

- [ ] bump 0.10.4 → 0.10.5
- [ ] release.yml 6 平台全过 + 12 asset
- [ ] Formula bump + push
- [ ] brew upgrade + 真测 frank ai ask 输出含 CallReport
- [ ] (不涉及 sync-agent 改动,跳过 tx 部署)
- [ ] 写 `docs/lessons/PHASE-1-LESSONS.md`

## 不在 v0.10.5 范围

- AI call 自动落 frank-memory 持久化(Q2.2 留 Phase 3 v0.11)
- 完整 4 CLI 多模型选择 UI(留 v0.11)
- ~/.frank/pricing.json 模板生成(留 Phase 2 v0.10.6 doctor 一并)

## 决策记录

- 采纳 agent GO-WITH-CAVEATS 建议
- 模块路径 `frank-cred/src/report.rs`(agent 推荐,reuse-existing-crate)
- 引 pricing JSON 不写 Rust const(防漂移,可热替换)
- Gemini fallback 走 char-count + confidence:low(不强求完美)
- opencode 二次 export 接受(本地 sqlite 快)
