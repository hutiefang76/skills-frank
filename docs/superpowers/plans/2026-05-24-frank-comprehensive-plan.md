# Frank 综合分析与详细规划 (v0.9.0 → v1.0 路线图)

> **触发**: 用户 `/maestro-plan 对目前的进行进行分析并作出详细的规划`
> **日期**: 2026-05-24
> **作者**: claude
> **方法论**: 借鉴 maestro-plan 5 phase 精神 (Context Collection → Clarification → Planning → Plan Checking → Confirmation) + superpowers (writing-plans / verification-before-completion)

---

## P1: Context Collection — 项目当前现状

### 1.1 已 ship 版本时间线

| 版本 | 日期 | 真用户价值 | 状态 |
|------|------|----------|------|
| v0.1.0 | 2026-05-21 | install/uninstall/scan + 6 平台 release | ✅ |
| v0.2.0 | | visibility 两层 5 档 | ✅ |
| v0.3-0.4 | | MCP 集成 + 4 frank-recommended | ✅ |
| v0.5.0-5.5 | | daemon 自启 + Web UI 演示版 + Homebrew tap + login | ✅ |
| v0.6.0 | | visibility 国际化 + scan type 列 + session 追溯 | ✅ |
| v0.7.0-7.3 | | proxy auto-detect + install --url + market + cleanup + uninstall 重定义 | ✅ |
| **v0.8.0** | **今天上午** | docker-compose 测官方 skill (nacos 真打通) + README 文档 + daemon 降级 + memory 共享上下文代码 | ✅ |
| **v0.8.1** | **今天下午** | sync-agent docker 真模式 (失败 — 国内 fastembed/HF 问题) | ❌ **接受失败留 v0.11** |
| **v0.9.0** | **刚才** | frank cache list/clear + --url --ref + update [name] | ✅ **just shipped** |

**今天累计**: v0.8.0 + v0.9.0 两个 release ship,各 11 + 4 commits,broker 修 8 个真 bug。

### 1.2 项目代码现状 (按 crate)

| Crate | 行数估算 | 真用户价值 | 完成度 |
|-------|---------|----------|--------|
| `frank-cli` | ~6000 | install/uninstall/scan/cache/update/ai ask/orchestrator/memory/login/doctor/market/config — 全 ship | **95%** |
| `frank-memory` | ~1500 | 14 单测全绿,Memory API 完整 | 90% (production deploy 阻塞) |
| `frank-sync-agent` | ~1200 | axum REST + qdrant + LocalEmbedder + offline from_files | 90% (国内部署阻塞) |
| `frank-orchestrator` | ~800 | LocalCliWorker + axum daemon + Web UI 演示级 | **50%** (协作模式没做) |

### 1.3 用户的真实反馈关键点 (按时间)

| 用户原话 | 我的执行情况 |
|--------|------------|
| "为什么没有 dmg" | brew install hutiefang76/frank/frank 真给了 |
| "下拉框是错误的,你管用户是什么套餐" | v0.5.5 改成自动探测本机 cli ✅ |
| "frank 申请音乐等不需要权限" | 不再 subprocess opencode models ✅ |
| "本末倒置,重点是 ask 功能" | refocus 到 ask + slash 命令 ✅ |
| "不应该本地装 cargo 编译" | v0.7.2+ 只 commit+tag+release.yml ✅ |
| "frank uninstall 直接全部删,第三方不管" | v0.7.3 重定义 ✅ |
| "GitHub Actions 我一开始就让你用" | v0.8.1 加 sync-agent-image.yml ✅ (build 通了但 fastembed 卡国内) |
| "本地下载好了压缩上传?" | v0.8.1 试了 (build #5+),最后没成因 hub layout 跟 fastembed 不对齐 |
| "效率很低下,死磕 sync-agent" | 接受 v0.8.1 失败,留 v0.11 用户介入 ✅ |
| "**按 superpowers 逻辑重新思考**" | 装了 superpowers,**v0.9 严格按 writing-plans+TDD+verification 跑** ✅ |
| 出门前: "Web UI / Windows 等还要什么没做的" | docs/STATUS.md 写完整清单 ✅ |
| Windows 笔记本要重新装 | 给了 winget 清单 + vscode tunnel 远程方案 |

### 1.4 当前关键约束

**用户视角**:
- 主力机 macOS arm64 / Homebrew 装
- 有 Windows 11 笔记本想用 (claude/codex 都过期需重装)
- tx 服务器在国内 (不通 HuggingFace)
- 用户**本人时间宝贵** (出差 / 出门频繁), 期望 AI 自驱动多

**技术视角**:
- frank-cli binary 不能拉 onnxruntime 系统依赖 (ADR-003 已锁定) → Web UI / 数据处理放服务端
- 国内服务器跑不通 fastembed (HF mirror Content-Range 缺失) → memory 真模式需要解决方案
- AI 不擅长长反馈周期 debug (e.g. docker build + tx pull 15min/次)
- RTK ask 框架依赖 tmux (新机环境 Plan Review 不通)

### 1.5 已 know 的真 bug / known issues

- ✅ [已修 v0.9.0] `frank install --url` 硬编码 `ref=main`
- ❌ RTK `ask` 依赖 tmux (v0.9+ 候选: frank 自带 review 命令)
- ❌ sync-agent 国内部署 fastembed 卡 HF (v0.11 用户介入)

---

## P2: Clarification — 借 maestro 风格, 关键决策点

(用户已答过的)

| 决策 | 用户答 | 时间 |
|------|--------|------|
| Memory 路线 (服务端 fastembed vs 客户端 vs OpenAI) | 客户端 embed (但实际 ADR-003 拒绝 — 转向用户介入 tx 真模式) | 今天 |
| v0.8.1 失败怎么收尾 | A: 立刻回滚 mock, v0.11 我亲手介入 | 刚才 |
| v0.9 优先 (多选) | cache + ref + update (推荐) + Web UI + P5/P6 + **Windows 笔记本** | 刚才 |
| v0.9 plan sign-off | 全 OK 立即开 cache | 刚才 |

**新待答 (v1.0 路线相关)**:
1. Windows 笔记本调试 — vscode tunnel 还是 RDP / 别的?
2. Web UI v0.10 范围: skill 管理 / memory 浏览 / token 配置 三个全做,还是分次?
3. tx 介入时间窗口 — 你有 30min 我们一起 ssh 解 fastembed 吗?

---

## P3: Planning — 详细规划

### 设计原则 (apply to all phases)

1. **每个版本独立可发**: v0.10 / v0.11 / v0.12 各自有完整价值,中间任何一步卡了不阻塞别的
2. **bite-sized 任务**: 每 commit < 100 行,可独立 review
3. **AI 强项优先**: 文档 / 写代码 / cli 增强 由我做; 部署 / 真机器调试 / 跨平台测试 你做
4. **TDD + verification**: 每命令先 test → 实现 → 真 smoke (按 superpowers 流程)

### v0.10: Web UI 补实 (我做,可分批 ship)

#### Wave 1 (v0.10.0): Skill 管理 UI (~3-4h)
- Web UI 加 `/skills` 路由 + 页面
- 表格列: name / visibility / platforms / source_ref / enabled
- 按钮: install (输入 name 或 --url) / uninstall / enable / disable
- 后端 axum 加 `GET /api/skills` `POST /api/skills/install` etc.
- 调用 frank-cli 内部函数 (复用 install::run 等)
- 单测 + clippy + 浏览器真 smoke

#### Wave 2 (v0.10.1): Memory 浏览 UI (~2-3h)
- `/memory` 路由
- list 表格 + search 框 + scope filter (user/agent/session)
- 调用 sync_client (复用现有 reqwest blocking)
- 没 sync-agent token 时 graceful 提示

#### Wave 3 (v0.10.2): 设置 UI (~2h)
- `/settings` 路由
- token (sync-agent) / proxy / OPENAI key 表单
- 写 `~/.frank/.token` `~/.frank/config.toml`

**v0.10 合计**: ~7-9h, 拆 3 个 patch release ship

### v0.11: Memory 真模式 (需要你介入)

**前置**: 30min 一起 ssh tx 解 fastembed cache layout

**步骤** (按 systematic-debugging Phase 1 RCA):
1. 进当前 sync-agent container 看 binary 实际查 cache 路径 (`strace` / `lsof`)
2. 改 frank-sync-agent 显式 set fastembed cache_dir + commit hash bypass refs/main fetch
3. 重 build + scp tx + restart
4. 验证 `frank memory add "vim是好编辑器" + search "editor"` 真召回

完成后 v0.11.0 release。

### v0.12: Windows 真测专项 (你+我)

**前提**: 你 Windows 笔记本装 vscode + `code tunnel`

**步骤**:
1. Windows 装 git/node/python/claude/codex/gh (按我之前给的清单)
2. 装 frank.exe (release page archive)
3. e2e 测 `frank install nacos-ops`, 看 Windows symlink 行为
4. 发现问题: 我远程 (via vscode tunnel) 修代码 + 你 build 测
5. 加 `scripts/uninstall-frank.bat` (Windows 版)
6. (可选) chocolatey / winget formula

预估: 2-3 个晚间 session

### v1.0.0: 真正稳定可发布版

**触发条件** (全部满足):
- ✅ v0.10 + v0.11 + v0.12 全 ship
- ✅ macOS / Linux / Windows 三平台都有真用户验证
- ✅ memory 真模式 production 可用
- ✅ README 中文版 + 视频 demo
- ✅ 至少 5 个真 user issue 在 GitHub 关闭过 (说明真有人用)

### 不做 (明确 scope out)

- ❌ P6 自动协作 (接力/投票/对辩) — 用户改了需求, 当前 frank ai ask --context-from 共享上下文已够
- ❌ rollback 命令 — 没有真用户提过, 留 stub
- ❌ frank-sync-agent 跨设备 skill 同步 (sync_skills_push/pull) — sync-agent 真模式上线后再说
- ❌ 中文 README 翻译 — 等 v1.0 之前最后做

---

## P4: Plan Checking — 自检

| 维度 | 自评 | 备注 |
|------|------|------|
| Requirements coverage | 90% | 覆盖了用户出门前提的所有诉求 (Web UI / Windows / cache / update) |
| Task quality (具体可执行) | 85% | v0.10 wave 1-3 都有明确文件路径 + 测试目标 |
| Dependency correctness | 95% | v0.10 / v0.11 / v0.12 独立无 hard dep, 顺序灵活 |
| Time estimation | 70% | v0.10 ~7-9h 是合理, v0.11 取决于 fastembed 真原因 (RCA 待你介入), v0.12 取决于 Windows symlink 实际 bug |
| Collision safety | 100% | v0.10 改 Web UI, v0.11 改 sync-agent, v0.12 加 Windows, 三者文件完全不重叠 |
| **总分** | **88%** | **PASS** |

### Pressure Pass: 最复杂任务 (v0.10 Wave 1)

`read_first[]`:
- `crates/frank-cli/src/cli/orchestrator_server.rs` (现有 Web 后端)
- `crates/frank-cli/src/cli/orchestrator_index.html` (现有前端)
- `crates/frank-cli/src/cli/install.rs` (复用 install logic)
- `crates/frank-cli/src/cli/list.rs` (复用 list logic)

`convergence.criteria`:
- `curl http://localhost:7780/api/skills | jq '.skills | length'` > 0
- `curl -X POST http://localhost:7780/api/skills/install -d '{"name":"frank-ask-gpt"}'` returns 200
- 浏览器开 http://localhost:7780/skills 真显示 9+ skills

✅ pressure pass 通过 — 标准明确, executor 能直接干。

---

## P5: Confirmation

### 立即推进 (我自驱不需要你确认)

- ✅ v0.9.0 ship 完 + Formula upgrade 完, 你 `brew upgrade frank` 就拿到 0.9.0

### 需要你 sign off 的决策

<决策表见下面的 AskUserQuestion>
