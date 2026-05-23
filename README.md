# Frank

> AI 工具链治理平台 — 统一管理 Claude Code / codex / opencode 三平台的 skills 与 MCP。

[![Status](https://img.shields.io/badge/status-WIP-orange)]() [![License](https://img.shields.io/badge/license-MIT-blue)]()

## 是什么

Frank 是一个 CLI 工具，帮你在多台设备 + 多个 AI CLI 之间统一管理：

- **skills**(见下方 "Skills 两层 5 档分类")
- **MCP servers**
- **CLAUDE.md 规则同步**
- **分布式记忆 + 调用统计**(自建 Docker stack: qdrant + caddy + frank-sync-agent,跑在腾讯云 VM)

### Skills 两层 5 档分类

frank 治理两种来源的 skills:

**Layer 1 — frank 内置(项目作者维护,装 frank 默认就有):**

| visibility | 含义 | 维护者 |
|---|---|---|
| `frank-own` | **芳哥自研** — frank 项目作者 (hutiefang76) 自己写的开源 skills | 项目作者 |
| `frank-recommended` | **芳哥推荐** — 项目作者推荐的 upstream / 第三方 skills (如 anthropics/*) | 项目作者列名单 |

→ 装 frank 立刻就有这两类(`frank list` 默认展示),一键 install。

**Layer 2 — 用户自定义(用户自己 manifest 加,跟项目作者无关):**

| visibility | 含义 |
|---|---|
| `user-public` | 用户自己开源的 skills (公开 git URL) |
| `user-company` | 用户**自己公司**的 skills (跟 frank 项目方无关,严禁混入本仓!放 `~/.frank/manifests/`) |
| `user-private` | 用户自己机密的 skills (个人凭据) |

→ 放在 `~/.frank/manifests/*.yaml`,frank 启动时自动合并。

> v0.1 老 `public` / `own-public` / `private` 通过 serde alias 兼容,不破老配置。

## 核心特性

- 🚀 一键安装/卸载/启用/禁用 skills，三平台同步
- 🔒 公司 skills 与公开 repo 严格分仓，杜绝信息泄露
- ↩️ 任何操作前自动 snapshot，60 秒回滚 (P1)
- ☁️ CLAUDE.md / memory / 调用统计 跨设备同步 (走自建 sync-agent,tx:8318)
- 🤖 AI 可写 feature 分支自动改进 skill 描述（不可写 main）
- 🎛️ 三种交互方式：CLI / Slash Command / WebUI（P3）

## 状态

- 🟢 **P0** — 完成：scaffold + manifest + 三平台 adapter + install/uninstall/enable/disable/list 端到端
- ⚪ P1 — auto-update + rollback + doctor
- 🟢 **P2** — 部分完成：qdrant + caddy 已部署到 tx:8318
- ⚪ P3 — Tauri WebUI
- ⚪ P4 — AI 自维护 PR
- 🟢 **P5 (frank-memory)** — 进行中：mem0 同思路 Rust 重写,骨架 + 14 单测全绿
- ⚪ **P6 (frank-orchestrator)** — 设计完成 (ADR-004),实现待启动

## 快速上手

### 安装

**推荐 — 一键脚本** (自动选最快路径:先尝试下预编译 binary,失败再 cargo build)

```bash
curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/install.sh | bash
```

**或手动下预编译 binary** (无需 Rust toolchain)

到 [Releases](https://github.com/hutiefang76/skills-frank/releases/latest) 下对应平台的包,解压把 `frank` 丢进 `$PATH`:

| 平台 | archive |
|---|---|
| macOS (Apple Silicon) | `frank-v0.4.0-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `frank-v0.4.0-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `frank-v0.4.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `frank-v0.4.0-aarch64-unknown-linux-gnu.tar.gz` |
| Windows x86_64 | `frank-v0.4.0-x86_64-pc-windows-msvc.zip` |
| Windows aarch64 | `frank-v0.4.0-aarch64-pc-windows-msvc.zip` |

例 (macOS Apple Silicon):
```bash
curl -fsSL https://github.com/hutiefang76/skills-frank/releases/latest/download/frank-v0.4.0-aarch64-apple-darwin.tar.gz | tar xz
sudo install -m 755 frank /usr/local/bin/    # 或: mv frank ~/.local/bin/
frank doctor                                 # 验证安装
```

**仅开发者** — 源码 build (修代码 / 跑测试)

```bash
git clone https://github.com/hutiefang76/skills-frank.git
cd skills-frank
cargo install --path crates/frank-cli --locked    # 全局装
# 或 cargo run -- doctor                          # 不装直接跑
```

> ⚠️ `cargo install frank` **永久不可用** — crates.io 上 `frank` 这个名字早在 2019 年被别人占了 (跟本项目无关)。发布到 crates.io 需要改名 (如 `frank-cli`),P1 决定。
>
> ⚠️ `brew install frank` / `npm i -g @hutiefang/frank-cli` 也不可用 — homebrew-tap / npm wrapper 留到 P1。

### 验证安装

```bash
frank doctor          # 11 项环境健康检查 (toolchain / ~/.frank/ / 三平台目录 / sync-agent)
frank scan            # 扫本机三平台 skills 目录
frank list            # 列 manifest 里的 skills
```

### 配置 sync-agent token (v0.5.1, `frank login`)

sync-agent 在 Caddy 层用 `X-Frank-Token` header 守 `/memory/*` `/orchestrator/*`,
没 token 直接 401。**装完 frank 跑 `frank login` 一次,以后所有 memory / orchestrator
命令都自动带 token**:

```bash
frank login --from-host tx        # ssh 拉 /opt/frank/.env 里的 FRANK_API_TOKEN, 写本机 ~/.frank/.token (600 权限)
frank login --token <xxx>         # 手敲 token (适合 1Password / 团队分发)
frank login --show                # 看当前 token (脱敏: 前 4 + 后 4)
frank logout                      # 删本机 token
```

> **注意**: `X-Frank-Token` 是访问**你公网 sync-agent** 的鉴权票据,**跟 LLM 计费 token
> 完全无关**。`frank memory list/search` 只读 qdrant 不调 LLM,**零计费**;
> 调 LLM 的只有 `frank ai ask` 和 `frank memory add` (后者要 LLM 抽 fact)。

### daemon 自启 + Web UI (v0.5 新增, macOS launchd)

`frank` 设计上是后台 daemon: 注册 launchd 一次就**永远在跑** (登录自启, 挂了自动重启),
日常你只敲 `frank` (无参数) 自动开浏览器到 Web UI。**不用再手动 `orchestrator serve` 阻塞终端**。

```bash
# 装一次 (写 ~/Library/LaunchAgents/com.frank.orchestrator.plist + 立刻起)
frank daemon install              # 默认 127.0.0.1:7780
frank daemon install --port 7799  # 自定义端口

# 日常用
frank                             # 无参数: 自动开浏览器到 daemon URL
frank daemon status               # 看 PID + 端口
frank daemon stop / start         # KeepAlive=true, stop 后会自动拉起
frank daemon uninstall            # 移除 launchd 注册 + 删 plist

# 日志
tail -f ~/.frank/logs/orchestrator.out.log
```

> **Linux / Windows**: v0.5 仅 macOS launchd 真接, systemd user unit + Windows 服务留 v0.6。
> 临时跑可继续用 `frank orchestrator serve --bind 127.0.0.1:7780` (阻塞终端)。

### 安装 skill

```bash
frank install doris-ops               # 公开 skill (manifest/public.yaml)
frank install kdwl:vehicle-events     # 公司 skill (~/.frank/manifests/, SSH + VPN)
```

### orchestrator 子命令 (P6 M1 真接本机 CLI subprocess)

`frank orchestrator` 真起本机 `claude` / `codex` / `opencode` / `gemini` 子进程,
**走你已付费的订阅(Pro/Plus/Go)**,不重复花 API key 的钱。多 Job 各自 subprocess,
**OS pid 级隔离, 多任务天然不串**。

```bash
# 看本机装了哪些 CLI
frank orchestrator providers

# 真跑一次 codex (gpt-5.5 Plus 订阅)
frank orchestrator demo --provider codex --prompt "Say hi" --timeout 300

# 真跑一次 claude (需要先 claude setup-token 登录 CLI, 见故障排查)
frank orchestrator demo --provider claude --prompt "Say hi"
```

#### 故障排查

| 现象 | 原因 | 修复 |
|---|---|---|
| `claude` exit 1 / 401 auth error | 你机器只登录了 Claude Desktop App,**`claude` CLI 自己没登录** (`~/.claude/.credentials*` 不存在) | `claude setup-token` 一次性登录 CLI(订阅自动识别) |
| `claude` 走 API key 而不是 OAuth | env 里有 `ANTHROPIC_API_KEY` 但是空值 (会覆盖 OAuth) | `unset ANTHROPIC_API_KEY` 然后再跑 |
| `codex` timeout | high-reasoning 慢,默认 120s 不够 | `--timeout 300` 或更高 |
| `frank orchestrator providers` 显示 ✗ | CLI 没装在 PATH 里 | `brew install` / `npm i -g` 对应 CLI |

### memory 子命令（P5 进行中,需 sync-agent 在线）

```bash
# 写入一条记忆
frank memory add "user prefers vim over emacs" --user alice

# 语义检索
frank memory search "editor preference" --user alice

# 列出
frank memory list --user alice
```

## 开发

```bash
git clone git@github.com:hutiefang76/skills-frank.git
cd skills-frank
cargo build --release        # ./target/release/frank
cargo test                   # 跑单元 + 集成测试
cargo clippy -- -D warnings  # lint
cargo run -- --help          # 跑 CLI
```

## 文档

- [`docs/DESIGN.md`](docs/DESIGN.md) — 完整设计文档（14 章 + 版本演进）
- [`docs/ADR/`](docs/ADR/) — 架构决策记录
  - [001-language-rust](docs/ADR/001-language-rust.md) · [002-cargo-workspace](docs/ADR/002-cargo-workspace.md) · [003-frank-memory-rust](docs/ADR/003-frank-memory-rust.md) · [004-frank-orchestrator](docs/ADR/004-frank-orchestrator.md) · [005-deploy-tencent-8317](docs/ADR/005-deploy-tencent-8317.md)
- [`deploy/README.md`](deploy/README.md) — 服务端部署 (tx:8318)
- [`CHANGELOG.md`](CHANGELOG.md) — 变更历史（待建）

## 技术栈

- **CLI**: Rust 1.75+, Cargo workspace · clap · serde · tracing · owo-colors · git2
- **frank-memory** (P5): Rust · qdrant-client · OpenAI/Anthropic API · async-trait
- **frank-sync-agent** (P5): Rust · axum · 部署在 tx:8318 (Qdrant + Caddy + axum,Docker Compose)
- **frank-orchestrator** (P6): Rust · axum WS · Postgres · 多 provider Worker trait
- **WebUI**: 静态 SPA / Tauri (P3 / orchestrator UI)

## License

MIT © hutiefang
