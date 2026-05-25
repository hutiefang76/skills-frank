# Phase 7 PLAN — v0.10.7 多模型 + 多窗口 + history 管理

| Field | Value |
|---|---|
| Phase | 7 |
| Version | v0.10.7 |
| 工期 | 4.5 天 |
| Agent | `a44d257c5c1d54328` 大白话报告,**Go** |
| 你拍板 | 3 个 P0/P1 痛点全做 / 还是只挑 |

## 🎯 这版给用户的 3 件事

### 1. 你能选具体模型(3.1 P0)— 0.8 天

**用户感受**:
```bash
$ frank ai ask --list-models                   # 列出 4 家所有模型
claude:    sonnet, opus, haiku, sonnet[1m], opus[1m]
codex:     gpt-5.5, gpt-5.5-pro, o3, gpt-5.4-mini
opencode:  haiku, qwen3.6, gpt-4o-mini, ... (用户自己配的 20+ 个)
gemini:    gemini-3.1-pro, gemini-2.5-pro, gemini-2.5-flash

$ frank ai ask --to opencode --model haiku "你好"
[frank] opencode/haiku ...
你好
```

**怎么实现**:
- claude/codex/gemini:frank 内置一个清单(没 CLI list 命令拿不到)
- opencode:跑 `opencode models` 实时拿(opencode 是唯一支持的)
- `~/.frank/models.yaml`:用户自己加额外 model 名,frank 合并

### 2. 同机多窗口 ask 并发不串(3.2 P0)— 0.3 天

**用户感受**:
- 你开 3 个终端同时跑 `frank ai ask --to claude "Q1"` `"Q2"` `"Q3"` → 都正常完成,**history 文件不损坏**
- 验证脚本:跑 9 次并发后 `wc -l ~/.frank/ai_history.jsonl` 应该 = 9 行,每行 JSON 都能解析

**怎么实现**:
- Mac/Linux 实际上现状已经差不多安全(`O_APPEND` 原子)
- Windows 不保证 → 加一个**文件锁**(<1ms 拿放,不影响速度)
- 跨平台都安全

### 3. 看 + 管 ask 历史 — CLI + Web UI(3.3+3.4 P1)— 3.4 天

**用户感受**:

**CLI**:
```bash
frank ai history list --provider claude --since 2026-05-01      # filter
frank ai history show 20260525-143022-a7f2                       # 看完整 prompt + reply
frank ai history delete 20260525-143022-a7f2                     # 单删
frank ai history delete --before 2026-04-01                       # 批删
frank ai history export --format md > history.md                  # 导出
```

**Web UI**(已有 Memory tab,新加 "AI History" tab):
- 顶部 filter:provider 下拉 / 状态 / 时间范围 / 清空按钮
- 表格:时间 / from→to / model / 状态 / 耗时 / 查看 / 删
- 点行展开:完整 prompt + reply + "复制"按钮 + "再问一遍"按钮

**怎么实现**:
- 当前 history 只存摘要(200 字),改成"摘要进 JSONL 索引 + 全文进 `~/.frank/ai-history-full/<id>.json` 一文件一条"
- 删起来快(删一个文件)
- 大量历史不撑爆内存(按需读)

## 🛡 5 风险大白话

| # | 风险 | 怎么办 |
|---|---|---|
| 1 | 用户没装 opencode,跑 `--list-models` 灰一片 | 没装的 CLI 标"⚠ 未装,跑 `brew install opencode`" |
| 2 | opencode 用户自配 model 名有空格/中文 | frank 不校验,直接转给 opencode,失败让 opencode 自己报错 |
| 3 | 几年下来 history 100k 条撑爆内存 | list 分页 + 50k 行时 warn 用户跑 `frank ai history delete --before` |
| 4 | 多窗口有一个 ask 超时挂住 | 不影响其他窗口(每个 ask 独立 subprocess,锁只在写 history 时拿 <1ms)|
| 5 | Web UI 删 history 误操作 | 删时弹"确认" + "全部清空"特别警告 |

## 📦 完整 8 子任务

| Sub | 干啥 | 工期 |
|---|---|---|
| D1 | `frank ai ask --list-models` 列 4 家所有模型 | 0.5d |
| D2 | `~/.frank/models.yaml` 用户自定义 | 0.3d |
| D3 | history 加文件锁(跨平台安全) | 0.3d |
| D4 | history 改造:摘要 JSONL + 全文一文件一条 + `<id>` 短码 | 0.5d |
| D5 | CLI 新命令:`history show/delete/export`(list 增 filter) | 0.7d |
| D6 | Web UI 加 "AI History" tab(仿 Memory tab) | 1.0d |
| D7 | 后端 REST 5 个端点(list/get/delete/batch-delete/export) | 0.7d |
| D8 | 端到端真测 4 CLI 模型切换 + 并发脚本 + Web UI 演示 | 0.5d |

**合计 4.5 天**(POSITION.md 路标对齐)。

## ⚖️ 怎么"对齐 POSITION.md"

| 你的独家定位 | 本版强化哪条 |
|---|---|
| 跨 AI provider 工具链统一 | ✅ 选模型 = frank 更深代理 4 家 CLI |
| device token 解耦 | ➖ 不动 |
| Rust 嵌入式单 binary | ➖ 不动 |

未踩任何撤回项(没算 cost / 没自动淘汰 / 没 token 预算)。

## ❌ 不在本版本

- 真的接管 mcp_memory(已砍掉,等 v0.11 真发力后用户自己想接管)
- 多机同步(留 v0.12)
- 本地 LanceDB(留 v0.11)

## 决策

✅ 采纳 agent GO + 全 8 子任务  
**关键**:用大白话写代码注释 + 用户文档,不让用户再次"看不懂"
