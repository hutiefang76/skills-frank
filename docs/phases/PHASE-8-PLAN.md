# Phase 8 PLAN — v0.10.8 模型清单从写死改用户配置动态加载

| Field | Value |
|---|---|
| Phase | 8 |
| Version | v0.10.8 |
| 工期 | 2 天 |
| Agent verdict | **Go**(`a91544a4c12d301db`)|
| 你拍板 | go/no-go |

## 🎯 这版给用户什么

**v0.10.7**:`frank ai ask --list-models` 显示 frank **写死**的 12 个 model 名,你 cc-switch 配的 12 个 provider 看不到。

**v0.10.8**:`frank ai ask --list-models` 显示**你机器上真有的所有 model**(cc-switch 配的 + 配置文件里的 + env 临时的 + 兜底默认的)。

```bash
# v0.10.8 后 (示意 — 实际拉你 cc-switch DB 的 12 个 provider):
$ frank ai ask --list-models
claude:    [来自 cc-switch] zkeys-免费/sonnet, official/opus, ...
           [配置文件] haiku
           [内置兜底] sonnet, opus
codex:     [来自 cc-switch] gpt-5.5, gpt-5.4, gpt-5.4-mini, gpt-5.3-codex
           [配置文件] gpt-5.5 (当前)
gemini:    [来自 cc-switch] gemini-3.1-pro, gemini-2.5-pro, ...
opencode:  [配置文件] xiaomi/mimo-v2-pro
```

## 🔍 cc-switch 是啥(agent 实地确认)

桌面 GUI 工具(macOS/Win/Linux),用户用它管多家 AI 账号 + 中转站。

**它怎么工作**:
- 把所有 provider 存在 `~/.cc-switch/cc-switch.db`(SQLite)
- 用户点"切到 X provider" → cc-switch **改各家 CLI 原生配置文件**(`~/.claude/settings.json` 等)

**你机器实测**:DB 里 12 行 provider,每行 `settings_config` JSON blob 里直接含 `"model":"gpt-5.4"` / `"models":[{"id":"gpt-5.4"},...]` 这种字符串 — **frank 直接读 DB 一遍就能拉出你所有备选 model**。

## 🧩 4 路合并方案(从高到低优先级)

| # | 来源 | 拉什么 |
|---|---|---|
| ① | `~/.cc-switch/cc-switch.db` SQLite | 你 cc-switch 配的**全部** provider 的 model |
| ② | 各家 CLI 原生配置文件 | `~/.claude/settings.json` 的 model / `~/.codex/config.toml` 的 model / `~/.codex/models_cache.json` / `~/.config/opencode/opencode.json` |
| ③ | env vars | `ANTHROPIC_MODEL` / `OPENAI_MODEL` 临时覆盖(显示但标"环境变量临时") |
| ④ | **frank 内置兜底**(claude=sonnet/opus/haiku 等) | 前 3 个全空时显示,**保 UX 不空** |

合并后去重,保留顺序(你配的在前,兜底在后)。

## 🔧 顺手清掉的 tech debt

v0.10.7 留了**两条分歧代码**:
- `cli/ai/models.rs`(CLI `--list-models`)
- `orchestrator_server.rs::detect_models`(Web UI `/providers`)

v0.10.8 让两条共用同一个 `collect_all` 函数,**Web UI 下拉前端不用改**,后端升级它自动拿到新清单。

## 📦 8 子任务(2 天)

| Sub | 干啥 | 工期 |
|---|---|---|
| D1 | 读 `~/.cc-switch/cc-switch.db` SQLite,挖 provider 表的 model 字段 | 0.5d |
| D2 | 读 4 家原生配置文件(`~/.claude/settings.json` 等)挖 model | 0.4d |
| D3 | 读 env vars(`ANTHROPIC_MODEL` 等),有 1 个加 1 个 | 0.1d |
| D4 | `collect_all` 4 路合并 + 内置 alias 兜底 + 去重,**删 spawn `opencode models` 子进程**(macOS TCC 弹窗坑,改读配置文件) | 0.3d |
| D5 | Web UI `detect_models` 删自己版本调 D4 同一份,两条路径合一 | 0.2d |
| D6 | Cargo.toml 加 `rusqlite`(bundled feature,无系统依赖) | 0.1d |
| D7 | 端到端真测:① 你 cc-switch 12 个全列出 ② 删 DB 后 fallback 到配置文件 ③ 删全后 fallback 到 alias ④ Web UI 跟 CLI 一致 | 0.4d |

## 🛡 3 风险大白话

| # | 风险 | 怎么办 |
|---|---|---|
| 1 | 用户没装 cc-switch + 没改配置(刚装 claude 跑 hello world) | 内置 alias 兜底 |
| 2 | cc-switch 配了"非官方"model 名(如 `kimi-k2.5`),CLI 跑时不识别 | frank **只列不校验**,失败让 CLI 自家报错 |
| 3 | cc-switch SQLite schema 升级 / 字段改名(v3.15+ 迭代很快) | frank 读 DB 用 try-catch **静默 fallback**,不让 cc-switch 失败拖垮 frank |

## 决策

✅ 采纳 agent Go + 全 8 子任务 + 4 路合并 + 内置兜底保留(防空清单)

## 不在本版本

- cc-switch 写入(只读)
- 显示每个 model "来自哪个来源"标签(可选,Web UI 加分项,留 v0.10.9)
- 模型推荐 / 切换功能(那是 cc-switch 自己的事,frank 不重复造)

## ✅ Position 对齐

- **独家定位 1 跨 AI provider 工具链统一**:强化(终于看到用户真配置而非猜)
- 不踩任何撤回项(没算 cost / 没自动淘汰 / 没 token 预算)
