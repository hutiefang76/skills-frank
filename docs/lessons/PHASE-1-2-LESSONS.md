# Phase 1+2 教训 — v0.10.5 (合并 ship)

| Field | Value |
|---|---|
| Phases | 1 (token 账单) + 2 (doctor 记忆全景) |
| Version | v0.10.5(合并 ship,跳过 v0.10.6 单独发) |
| Date | 2026-05-25 |
| Duration | ~3 小时(P + D + C + A 全闭环,含 retag 修正) |

## 最大教训:工程师视角 ≠ 用户视角

### 第一版 v0.10.5 出错的地方

Phase 1 P agent 输出 GO-WITH-CAVEATS 后,我**直接进 D**,没让用户审 PLAN 细节。Plan 里有:
- 完整 2026 定价表(`pricing-2026-05.json`,7 个模型)
- `PricingTable::load_bundled / load_with_override`
- `CallReport.cost_usd` + `Confidence::High/Med/Low`
- render 时显示 `$0.0925`

D 跑完 → C 跑完 → A ship → release page 创建 → **用户睁眼一句**:"搞复杂了,中转站价格不一样,只输出 token 数即可"

### 错在哪
- **定价是 moving target**:OpenAI 2025 改了 2 次,Anthropic Opus 4.1→4.5 价格从 $15 降到 $5
- **中转站架空假设**:frank 不知道用户用 cli-proxy / claude-relay 还是官方 endpoint,算的 cost 是误导
- **复杂度无回报**:用户要的是"看见消耗",我给的是"看见消耗 + 误差≥30% 的成本估算"

### 修复决策
- gh release delete v0.10.5 --cleanup-tag(release 才创建几分钟,无人下载,撤了不破坏 immutability)
- 删 `pricing.rs` + `pricing-2026-05.json` + `cost_usd` 字段 + `Confidence` 枚举
- 重 tag v0.10.5 于新 commit(净减 -364 行)
- 重跑 release.yml(同款 Windows cache failure,rerun 即过)
- brew upgrade 真测确认无 $ 输出

## PDCA 流程教训

| 阶段 | 教训 |
|---|---|
| **P** | agent verdict 是"技术可行性",不是"产品价值"。下次 P 之后**强制 AskUserQuestion 让用户审 PLAN 内容**,不只问"go/no-go 启动 D" |
| **D** | 多 agent 并行赶工成功 — Phase 1+2 双 worker 0 文件冲突,9 commits 直进 main 跑全绿。worktree isolation 实际可能共用 main,但 commit 顺序合理就不破 |
| **C** | workspace test/clippy/fmt + 端到端真测全过 — 但只是"代码工程质量",**没拦截"产品决策错误"** |
| **A** | release page 创建后才被批评 — 流程应该把 user review 提前到 P 末尾,不是 A 之后 |

## 改进点(写入下次 Phase 3/4 强制项)

1. **P 阶段末尾必须 AskUserQuestion 让用户审 PLAN 关键决策**(不只问 go/no-go,要列出"我准备做 X / 不做 Y",让用户精修)
2. **每个"看似 nice-to-have 的全面性"提议(如 pricing 表 / Gemini 支持 / 4 OS 跨平台细节)都要先问用户**,默认走 MVP
3. **CallReport 等新数据结构,字段设计前先问用户"什么是噪音"** — frank 给的字段是给用户看的,不是给我自己看的
4. **Release page 创建前给用户 5 分钟窗口看 PR diff**(release.yml 后插一个 `manual: approve` step,或我手动 dry-run 一次给用户看)

## 工程亮点(可复用)

- **撤回 v0.10.5 干净** — `gh release delete --cleanup-tag` 一行,无遗留
- **CallReport 简化后 API 反而更顺** — 删 cost 后调用方不用算价不用查表,直接构造,代码净减
- **多 agent 并行**省时 — 双 agent 并行 ~3 小时(P+D+C+A),单线串行估 6 小时

## v0.10.5 最终输出

```bash
$ frank --version
frank 0.10.5

$ frank doctor    # Phase 2 输出
## 记忆系统全景
  • claude     ~/.claude.json                         official: memory
  • codex      ~/.codex/config.toml                   official: memory
  ...
  ─ frank CLI: 在 PATH ─ 推荐: 建议禁用 official memory MCP, 让 frank-memory 接管

$ frank ai ask --to codex "say OK"    # Phase 1 输出
[frank] codex/gpt-5.5 in=38045 out=272 16106ms sid=019e5c97
OK

$ frank memory search "test"    # Phase 1 输出
[frank] frank-memory/text-embedding-3-small in=1 out=0 1107ms
! no match
```

**核心理念落地**:用户拿到 token 数 / latency / session,自己按 endpoint 实际单价算成本。
