# Phase 2 PLAN — v0.10.6 doctor 记忆全景 + CLAUDE.md 模板

| Field | Value |
|---|---|
| **Phase** | 2 (PDCA Plan 阶段完成) |
| **Version** | v0.10.6 |
| **Estimated** | 1 工作日 |
| **Agent verdict** | **GO** (agent `a729c15fc44a3a8eb`,无单维度风险高于 mitigation) |
| **Key insight** | `installer/mcp.rs` + `scan.rs` 已有 claude/codex 读取代码 — 仅需补 gemini/opencode + 检测逻辑 + 模板 |

## 用户需求映射

| Q 编号 | 用户原话 | 本阶段实现 |
|---|---|---|
| Q1 Layer 2 | "提示词注入 — 在 CLAUDE.md 加规则优先 frank" | 生成 `~/.frank/claude-template.md` 让用户复制 |
| Q3 配套 | "frank doctor 列出 mcp__memory vs frank-memory 推荐 disable" | 新增 `## 记忆系统全景` 节,4 行表格 |

## 技术方案

### 1. MCP 配置位置(4 provider)

| Provider | 路径 | 格式 | 已有读取? |
|---|---|---|---|
| claude | `~/.claude.json` | JSON,top + nested `projects.<path>.mcpServers` | ✅ `installer/mcp.rs` 已有 |
| codex | `~/.codex/config.toml` | TOML `[mcp_servers.<name>]` | ✅ `scan.rs` 已有 |
| gemini | `~/.gemini/settings.json` | JSON `mcpServers.<name>` | ❌ 新增 |
| opencode | `~/.config/opencode/opencode.json` | JSONC `mcp.<name>` | ❌ 新增 |

**全部 read-only**: `fs::read_to_string` → parse → `Ok(Some) / Ok(None) / log warn`。**永不写**,防破坏用户配置(R-P2.1)。

### 2. 模块切分(新)

```
crates/frank-cli/src/mcp_inspect/   NEW (~300 LOC)
├── mod.rs           Provider + MemorySetup + Recommendation
├── claude.rs        复用 installer/mcp.rs reader,适配 mcp_inspect 接口
├── codex.rs         复用 scan.rs reader
├── gemini.rs        NEW (~50 LOC, JSON 解析)
├── opencode.rs      NEW (~60 LOC, JSONC 解析,用 json5 crate)
└── memory.rs        Memory 检测 + Recommendation matrix
```

### 3. 数据模型

```rust
pub struct ProviderMemory {
    pub provider: &'static str,
    pub config_path: Option<PathBuf>,
    pub official_mcp: Option<OfficialMcp>,    // 装了 @modelcontextprotocol/server-memory?
    pub frank_mcp: Option<FrankMcp>,          // v0.12 预留
}

pub enum Recommendation {
    NoChange,          // 干净 (无 official + frank CLI 在)
    DisableOfficial,   // official 装了 + frank CLI 在 → 推荐关 official
    InstallFrank,      // 都没装 → 推荐 brew install frank
    KeepBoth,          // 用户已显式 keep
}
```

### 4. doctor 输出格式

```
## 记忆系统全景

Provider   Config              Official MCP    Frank MCP    Action
--------------------------------------------------------------------
claude     ~/.claude.json      ✗ npx @mcp/...  -            (官方 mcp 已装, 建议 disable)
codex      ~/.codex/config     ✗ enabled       -            (同上, codex auth login --token)
gemini     ~/.gemini/settings  -               -            (无记忆 mcp, frank CLI 接管)
opencode   ~/.config/opencode  -               -            (同上)

→ 建议: claude mcp remove memory --scope user (关 claude 官方 memory MCP)
→ 建议: 编辑 ~/.codex/config.toml [mcp_servers.memory] enabled=false
```

### 5. CLAUDE.md 模板(25 行,<600 字节)

```markdown
<!-- BEGIN frank-memory-preference v1 -->
## 记忆与知识系统 (frank-memory)

本机已装 `frank` CLI, 配套分布式向量记忆。优先级如下:

1. **首选**: 用 `frank memory add <内容>` 持久化, `frank memory search <q>` 召回。
   - slash: `/frank-mem-list` 查全部, `/frank-mem-search <q>` 语义检索。
   - 跨工具 (claude/codex/gemini) 共享同一记忆库。
2. **不要主动调用** `mcp__memory__create_entities` / `mcp__memory__add_observations`
   (Anthropic 官方 MCP memory) — 它仅本机 JSON, 不跨工具, 与 frank-memory 冲突。
   除非用户明说 "用官方 memory" 才用。
3. **何时记**: 用户说出偏好 ("我是芳哥, 偏好 Rust") / 决策 / 个人事实 时,
   立刻 `frank memory add` 一条, 无需询问。技术性中间结果不要存。

frank-recommended skills (anthropic skills 如 pdf/docx/xlsx) 保持启用。
<!-- END frank-memory-preference -->
```

**幂等插入策略**:
- v0.10.6 **仅生成** `~/.frank/claude-template.md`,**不自动 append** 到用户 `~/.claude/CLAUDE.md` (防 R-P2.1 风险)
- `frank install` 完成后 stderr 打: `提示: cat ~/.frank/claude-template.md >> ~/.claude/CLAUDE.md 让 Claude 优先用 frank-memory (可选)`
- v0.10.7+ 可加 `frank doctor --fix claude-md` 自动 append(基于 `<!-- BEGIN frank-memory-preference v1 -->` 标记检测幂等)

## 风险 + 缓解(agent 5 条)

| ID | 风险 | 严重性 | 缓解 |
|---|---|---|---|
| R-P2.1 | 写 ~/.claude.json 破坏用户状态 | **HIGH** | v0.10.6 仅读不写;模板独立文件 |
| R-P2.2 | JSONC 让 serde_json 崩 | MED | 引 `json5 = "0.4"` crate(~30KB)处理 opencode |
| R-P2.3 | 跨平台路径(Win `%APPDATA%`) | MED | `dirs::home_dir()` 通吃,Win CI 兜底 |
| R-P2.4 | CLAUDE.md 指令不必然被遵守 | LOW | 模板 opt-in,后续 wiki 追踪命中率,v0.11 迭代 |
| R-P2.5 | 与 cursor/windsurf 同文件指令冲突 | LOW | `<!-- BEGIN -->` marker 防文本冲突 |

## sub-task 拆解(D 阶段)

| Sub | 内容 | 工期 |
|---|---|---|
| D1 | 新建 `mcp_inspect/` 模块 + 4 provider reader(claude/codex 复用,gemini/opencode 新写) | 0.3d |
| D2 | `Recommendation` matrix + provider-specific disable hint | 0.2d |
| D3 | `frank doctor` 加 `## 记忆系统全景` 节 | 0.2d |
| D4 | `frank install` 末尾生成 `~/.frank/claude-template.md`(幂等检查) | 0.2d |
| D5 | 文档 + 单测(≥10 unit test 覆盖 4 provider reader) | 0.1d |

**合计 1.0d**(agent 估 1d,一致)。

## C 阶段 ship gate

- [ ] cargo test/clippy/fmt/docs 全过
- [ ] **端到端真测**:`frank doctor` 输出含"记忆系统全景"节,显示当前 4 provider 状态
- [ ] **端到端真测**:`frank install <任意 skill>` 后 `~/.frank/claude-template.md` 文件存在
- [ ] `cat ~/.frank/claude-template.md` 显示 25 行 frank-memory 偏好模板

## A 阶段 ship gate

- [ ] bump 0.10.5 → 0.10.6
- [ ] release.yml 6 平台全过 + 12 asset
- [ ] Formula bump + push
- [ ] brew upgrade + 真测 doctor 输出
- [ ] (不涉及 sync-agent 改动,跳过 tx 部署)
- [ ] 写 `docs/lessons/PHASE-2-LESSONS.md`

## 不在 v0.10.6 范围

- 自动 append CLAUDE.md(留 v0.10.7+,看用户实际是否手动 cat)
- frank-mem MCP server 协议兼容(留 Phase 4 v0.12)
- Memory state.json 持久化"用户已显式 keep both"hint(留 v0.11)

## 决策记录(我的判断)

- 采纳 agent GO 建议
- 模块路径 `mcp_inspect/`(agent 推荐),不混 `installer/`(那里是写,这里是读)
- 引 `json5` crate 不引 `serde_jsonc`(crate 更小,生态稳)
- CLAUDE.md 模板**不自动 append**(R-P2.1 太重,1 行 cat 命令用户成本极低)
- 4 provider 一起做(reader 复用率高,分批反而费工)
