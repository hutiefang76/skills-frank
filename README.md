# Frank

> AI 工具链治理平台 — 统一管理 Claude Code / codex / opencode 三平台的 skills 与 MCP。

[![Status](https://img.shields.io/badge/status-WIP-orange)]() [![License](https://img.shields.io/badge/license-MIT-blue)]()

## 是什么

Frank 是一个 CLI 工具，帮你在多台设备 + 多个 AI CLI 之间统一管理：

- **skills**（公共 / 自研 / 公司三类来源）
- **MCP servers**
- **CLAUDE.md 规则同步**
- **分布式记忆 + 调用统计**(自建 Docker stack: qdrant + caddy + frank-sync-agent,跑在腾讯云 VM)

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
| macOS (Apple Silicon) | `frank-v0.1.0-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `frank-v0.1.0-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `frank-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `frank-v0.1.0-aarch64-unknown-linux-gnu.tar.gz` |
| Windows x86_64 | `frank-v0.1.0-x86_64-pc-windows-msvc.zip` |
| Windows aarch64 | `frank-v0.1.0-aarch64-pc-windows-msvc.zip` |

例 (macOS Apple Silicon):
```bash
curl -fsSL https://github.com/hutiefang76/skills-frank/releases/latest/download/frank-v0.1.0-aarch64-apple-darwin.tar.gz | tar xz
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

### 安装 skill

```bash
frank install doris-ops               # 公开 skill (manifest/public.yaml)
frank install kdwl:vehicle-events     # 公司 skill (~/.frank/manifests/, SSH + VPN)
```

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
