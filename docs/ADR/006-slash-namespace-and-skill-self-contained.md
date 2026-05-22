# ADR-006: Slash 命名空间 + Skill 自含原则 + 三入口分层

| Field | Value |
|---|---|
| **Status** | Accepted (slash 散装版已落 v0.4; plugin 机制 + skill 自含 留 v0.5) |
| **Date** | 2026-05-23 |
| **Decider** | hutiefang |
| **背景** | 用户 8 条产品反馈(2026-05-23 凌晨) |

## 用户原话(8 条全列, 不删)

1. slash 命名空间应像 `/frank:ask:gpt` / `/frank:ask:claude` / `/frank:ask:opus` / `/frank:ask:haiku` / `/frank:ask:gpt5.5` 等独立命名 — **问"最佳方案"**
2. 缺 `/frank:mem:list` / `/frank:memory:list` 等**用户主动操作**的 slash, 不只一问一答
3. CLI / Web UI / slash 三入口都能查询状态 / 管理网址 / 操作命名 — **不推荐请告诉**
4. `frank orchestrator serve --bind` 用户**不该自己启** — 应该是 daemon 自动跑, `frank` CLI 只**打开浏览器**到 daemon URL
5. 列出芳哥**自研** + 芳哥**推荐**两类 skills
6. **托管全机器所有 skills + MCP** — 含 frank 装前 + 非 frank 装的 (现有 ~/.claude.json mcpServers / ~/.claude/skills/* 等)
7. **安装管理** skills 是 CLI / 接口 / 界面的功能, **不是 claude slash**
8. **slash = 使用**, CLI / 接口 / 界面 = **管理** (同时也能简单使用 / 测试)

## 决策

### Q1 — Slash 命名空间最佳方案 = Claude Plugin (留 v0.5 真接)

**短期 (v0.4 已落地)**: 散装 SKILL.md, 文件夹用 `frank-ask-gpt` / `frank-mem-list` 等横线命名, description 里写 `/frank:ask:gpt` 等触发关键词. claude code 看 description 触发, 命名空间用 description 模拟. **缺点**: 命名扁平 (`frank-ask-gpt` 而不是 `frank:ask:gpt`), 7+ 个独立 skill 视觉噪音.

**长期 (v0.5 真做)**: 把 frank 做成 **Claude Plugin**, 跟 `anthropic-skills:*` 同款机制:

```
~/.claude/plugins/installed/frank/
├── .claude-plugin/
│   ├── plugin.json        # name=frank, version, description, ...
│   └── marketplace.json   # 注册到 marketplace (关键, 否则不识别 namespace)
└── skills/
    ├── ask-gpt/SKILL.md   # → 用户看 /frank:ask-gpt
    ├── ask-claude/SKILL.md
    ├── ask-opencode/SKILL.md
    ├── ask-gemini/SKILL.md
    ├── mem-list/SKILL.md
    └── mem-search/SKILL.md
```

用户命名空间生效后: `/frank:ask-gpt <prompt>` 真触发, system-reminder 显示 `frank:ask-gpt` (跟 `anthropic-skills:pdf` 同款).

**风险 / 不好的地方** (用户原话"如果不好告诉我"):
1. **多层冒号 `frank:ask:gpt` (两层) 操作系统不支持文件名带 `:` (macOS Finder, Windows NTFS)**.
   解决: 单层 `frank:ask-gpt` (用横线代第二层) 是 claude plugin **官方支持**的最深名空间.
2. **skill 数量爆炸**: 当 frank 加 10+ slash 命令时, claude 启动扫描 + 用户视觉负担涨.
   缓解: **只把"用户高频主动操作"做成 slash** (Q7 / Q8 — slash=使用不是管理), 装 / 卸 / 同步留 CLI.
3. **plugin 注册需 marketplace** (我刚试 `cp` 到 installed/ 下 claude 不自动识别 namespace);
   v0.5 要么走 `claude plugin install` cli, 要么发个 marketplace.json 到 GitHub Pages 让用户 `/plugin marketplace add ...`.

### Q2 — `/frank:mem:list` 等管理类 slash

v0.4 已加 散装 2 个 (`frank-mem-list`, `frank-mem-search`), v0.5 plugin 化变 `frank:mem-list` / `frank:mem-search`.

**注意 Q7 + Q8 约束**: 只做"用户主动 read 操作", **不做** `frank:install` `frank:uninstall` (那是装管, 应该 CLI / Web UI).

### Q3 — 三入口对等

**修正**: 不是三入口完全对等, 是**用户视角统一**:

| 入口 | 主战场 | 也能做 |
|---|---|---|
| **slash** (claude / codex / opencode 里输 `/frank:...`) | 快速**使用** (问 AI / 查记忆) | 简单测试 |
| **CLI** (`frank ...`) | **装管**全套 (install/uninstall/scan/import/dedupe/list/doctor) | 也能 `frank ai ask` 测试 |
| **Web UI** (`frank` 命令打开浏览器, 后台 daemon) | **可视化装管** + job 看板 + 记忆浏览 | 也能跟 slash 一样使用 |

→ **Web UI 加 skills 管理 + memory 浏览面板** 留 v0.5 (现 orchestrator UI 只有 Job 看板).

### Q4 — daemon 自启 + frank 命令只打开浏览器

**改架构**:

```
v0.4 (错的): 用户手动 `frank orchestrator serve --bind 127.0.0.1:7780`
            → 终端窗口阻塞 → 关窗口 daemon 死

v0.5 (对的):
  install.sh / cargo install frank 装完
    ↓ 触发
  frank daemon install        # 注册 launchd plist (macOS) / systemd unit (Linux) / Windows 服务
    ↓ 自启
  ~/.local/Library/LaunchAgents/com.frank.orchestrator.plist
    ↓ launchd 跑
  frank-orchestrator-daemon --port 7780 (后台)
    ↓
  用户跑 `frank`  →  打开浏览器 http://127.0.0.1:7780 (不阻塞)
```

**风险**:
- macOS launchd / Linux systemd / Windows 三套机制都要写 — 工作量 2 天
- 卸 frank 时要清 daemon plist — 加 uninstall hook

### Q5 — 芳哥 own + recommended 真列表

见 `crates/frank-cli/manifest/builtin.yaml` 实际内容.

**真存在的**:
- frank-own: `nacos-ops`, `streampark-ops` (2 个)
- frank-recommended: `skill-creator`, `superpowers`, `mcp-time`, `mcp-sequential-thinking`, `mcp-fetch`, `mcp-context7` (6 个)

**手册声明的但仓库还没发 (404, 留占位)**: `doris-ops`, `feishu-read`, `dolphinscheduler-ops`.
后续你发布 git 仓库后取消 builtin.yaml 里对应注释即可启用.

### Q6 — 托管 frank 装前 / 非 frank 装的 skills + MCP

**v0.4 已部分做**: `frank scan` 扫三平台 `~/.{claude,codex,opencode}/skills/` 真目录, 识别 external skill (frank state.json 里没记的).

**v0.4 缺**: 不扫 **MCP** (`~/.claude.json mcpServers` / `~/.codex/config.toml [mcp_servers.*]`).

**v0.5 做**:
- `frank scan --mcp` 扫两平台 mcp 配置, 输出每个 mcp server 状态 (managed by frank / external / 重复定义等)
- `frank import-mcp <name>` 收编 external MCP 到 state, frank 接管
- 跟现有 `frank scan` / `frank import` 行为对齐

### Q7 + Q8 — 产品定位分层 (硬规约)

- **slash 命令 = 使用 / 测试**:
  - `/frank:ask-gpt <prompt>` 一问一答
  - `/frank:mem-search <query>` 查记忆
  - **不做** `/frank:install xxx` (那是装管)
- **CLI / Web UI / REST API = 装管主战场**:
  - `frank install / uninstall / enable / disable / scan / import / dedupe / list / doctor / sync`
  - Web UI 同款图形化
  - REST API (sync-agent 已暴露 /memory/* /sync/*) 给集成方调

---

## frank-own skill 自含原则 (用户原话核心诉求)

> "因为和 skills-kdwl (私有仓库) 深度绑定不可直接使用, 你需要修改, 把相关的代码、依赖安装等内容移动到这些 skills 内部, skills-frank 只是辅助安装和管理."

**强制约束**:

1. 每个 `frank-own` skill 的 git 仓库 **自含**:
   - `SKILL.md` (claude 入口)
   - `install.sh` / `setup.sh` (用户机器跑一遍, 装 venv / npm / 依赖)
   - 任何 .py / .ts / 配置文件 — **全部在该 skill 仓库内**
2. **不依赖** skills-kdwl 私有仓 / 其他公司内部仓
3. **不依赖** skills-frank 主仓的代码

**frank install 做的事 (仅这些)**:
- `git clone <skill_url>` 到 `~/.frank/cache/<hash>/`
- `ln -s cache/<hash> ~/.{claude,codex,opencode}/skills/<name>`
- **不跑** skill 内的 install.sh (用户自己手动跑, frank doctor 检测装没装)

**未来 v0.5**: 可选 `frank install --run-setup` 自动跑 skill 内 install.sh (有交互式 prompt 时需用户确认).

---

## 实施路线 (v0.4 已落 / v0.5 待做)

| Q | v0.4 状态 | v0.5 计划 |
|---|---|---|
| 1 (slash 命名空间) | 散装 7 SKILL.md, 命名 `frank-ask-*` / `frank-mem-*` | Plugin 化, 命名 `frank:ask-*` / `frank:mem-*` (跟 anthropic-skills:* 同款) |
| 2 (mem slash) | ✅ frank-mem-list + frank-mem-search 已装 | Plugin 化重命名 |
| 3 (Web UI 三入口) | orchestrator UI 只 Job 看板 | 加 skills 管理 + memory 浏览面板 |
| 4 (daemon 自启) | ❌ 还要用户手动 `serve` | launchd + systemd + Windows 服务三套 + `frank` 命令打开浏览器 |
| 5 (真列表) | ✅ builtin.yaml 已修正名字 + 注释 3 个未发的 | 等 doris-ops / feishu-read / dolphinscheduler-ops git 仓真发 |
| 6 (扫 MCP) | ✅ scan 扫 skills 目录 / ❌ 不扫 MCP 配置 | `frank scan --mcp` + `frank import-mcp` |
| 7 (装管 ≠ slash) | ✅ 文档化 | — |
| 8 (slash = 使用) | ✅ 文档化 | — |
| skill 自含 | ✅ ADR 写入硬规约 / ❌ 仓库还没真自含 | 你 push doris-ops 等仓库时按这规约写 |
