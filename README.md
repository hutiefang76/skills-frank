# Frank

> AI 工具链治理平台 — 统一管理 Claude Code / codex / opencode 三平台的 skills 与 MCP。

[![Status](https://img.shields.io/badge/status-WIP-orange)]() [![License](https://img.shields.io/badge/license-MIT-blue)]()

## 是什么

Frank 是一个 CLI 工具，帮你在多台设备 + 多个 AI CLI 之间统一管理：

- **skills**（公共 / 自研 / 公司三类来源）
- **MCP servers**
- **CLAUDE.md 规则同步**
- **分布式记忆 + 调用统计**（腾讯云后端）

## 核心特性

- 🚀 一键安装/卸载/启用/禁用 skills，三平台同步
- 🔒 公司 skills 与公开 repo 严格分仓，杜绝信息泄露
- ↩️ 任何操作前自动 snapshot，60 秒回滚
- ☁️ CLAUDE.md / memory / session log / 调用统计 同步到腾讯云
- 🤖 AI 可写 feature 分支自动改进 skill 描述（不可写 main）
- 🎛️ 三种交互方式：CLI / Slash Command / WebUI（P3）

## 状态

- 🟢 **P0 (1 周)** — 进行中：scaffold + manifest + 三平台 adapter
- ⚪ P1 (3 天) — auto-update + rollback
- ⚪ P2 (1 周) — 腾讯云 sync-agent
- ⚪ P3 (1 周) — Tauri WebUI
- ⚪ P4 (1 周) — AI 自维护 PR

## 快速上手（P0 完成后）

```bash
# 安装
cargo install frank
# 或: brew install frank / scoop install frank / npm i -g @hutiefang/frank-cli

# 列出已知 skills
frank list

# 安装一个公开 skill
frank install doris-ops

# 安装公司 skill (需 SSH + VPN)
frank install kdwl:vehicle-events
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

- [`docs/DESIGN.md`](docs/DESIGN.md) — 完整设计文档（14 章）
- [`docs/ADR/`](docs/ADR/) — 架构决策记录
- [`CHANGELOG.md`](CHANGELOG.md) — 变更历史（待建）

## 技术栈

- **CLI**: Rust 1.75+ · clap · serde · tracing · owo-colors · git2
- **sync-agent**: Rust · axum · TencentDB · COS · KMS（P2）
- **WebUI**: Tauri + React（P3）

## License

MIT © hutiefang
