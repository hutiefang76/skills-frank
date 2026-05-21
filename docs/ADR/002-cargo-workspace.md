# ADR-002: 切换到 Cargo workspace 多 crate 布局

| Field | Value |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-05-21 |
| **Decider** | hutiefang |
| **Supersedes** | (无, 扩展 ADR-001) |

## 背景

P0 阶段 frank 是单 crate (单二进制), 现在要往三个方向扩张:

1. **frank-memory** (P5): mem0 同思路的 Rust 重写, 给 AI agent 提供分布式记忆
2. **frank-orchestrator** (P6): 多 AI provider 协作总线 (替代 CCB 的 tmux 路线)
3. **frank-sync-agent** (P2+P5): 跑在腾讯云的 axum 服务, 把 memory + 任务调度暴露给本机 CLI

这些是 frank 治理范畴的"亲族" crate, 共享:
- ADR-001 质量基线 (pedantic / missing_docs / forbid unsafe)
- 编译 profile (体积优先 release)
- 通用依赖 (anyhow / serde / tokio / tracing)

但每个都是独立 binary / lib, 独立版本号。

## 候选

| 方案 | 优点 | 缺点 |
|---|---|---|
| **A. 单 repo Cargo workspace, 子 crate** | 共享 CI / lockfile / lints; 改动跨 crate 一次 PR | 仓库变大, CI 编译时间累加 |
| B. 多 repo (skills-frank-memory 等独立) | 严格隔离, 独立 release 节奏 | 跨 crate 联动改痛苦, manifest schema 同步麻烦 |
| C. 单 crate, 模块化 | 最简单 | 二进制大, 一改动重编全部 |

## 决策

**采用方案 A** (workspace + 子 crate)。`Cargo.toml` 根为 workspace, 全部 crate 放 `crates/<name>/`。

## 布局

```
skills-frank/
├── Cargo.toml                  # [workspace] members + shared lints + profile
├── Cargo.lock                  # 共享锁文件
├── crates/
│   ├── frank-cli/              # CLI 主二进制 (P0, 已落地)
│   │   ├── Cargo.toml          # [package] frank, [[bin]] frank
│   │   ├── src/
│   │   └── manifest/public.yaml
│   ├── frank-memory/           # mem0 Rust 重写 (P5)
│   ├── frank-orchestrator/     # 多 agent 总线 (P6)
│   └── frank-sync-agent/       # 服务端 host (P2+P5)
├── deploy/                     # docker-compose / nginx / 部署脚本
├── docs/                       # 设计文档 + ADR
└── .github/workflows/          # CI
```

## 共享配置实现

### 顶层 Cargo.toml 关键节

```toml
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.75"

[workspace.dependencies]
anyhow = "1.0"
tokio = { version = "1.40", features = [...] }
# ... 跨 crate 共用的依赖在此

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
# ... 例外列表

[profile.release]
opt-level = "z"
lto = true
# ...
```

### 子 crate 继承

```toml
# crates/frank-cli/Cargo.toml
[package]
name = "frank"
version.workspace = true
edition.workspace = true
# ...

[dependencies]
anyhow = { workspace = true }
# ... crate 专属依赖独立写

[lints]
workspace = true   # 继承 [workspace.lints.*]
```

## 不在范围

- **不强行统一所有依赖版本到 [workspace.dependencies]**: 仅跨 crate 出现的放进去; CLI 专属 (clap, tabled) 不入。避免给单 crate 拉无关的依赖图。
- **不立刻拆 frank-cli 进一步**: 子模块 (manifest / installer / adapter / state) 继续在 frank-cli 内, 不为了拆而拆。

## 影响 / 迁移

- ✅ 现有 `cargo build` / `cargo test` / `cargo clippy --workspace` 命令保持有效, 无破坏
- ✅ CI `.github/workflows/ci.yml` 已经用 `--workspace --all-features`, 无需改
- ⚠️ `cargo install frank` 仍工作 (workspace 不影响 install), 但 `Cargo.toml` 路径变 `crates/frank-cli/Cargo.toml`; 用户从 git 装时无感
- ⚠️ `CARGO_MANIFEST_DIR` 在 frank-cli 里指向 `crates/frank-cli/`, manifest/public.yaml 同步搬到这里, parser.rs 无需改

## 后续动作

- [x] 顶层 Cargo.toml 改 [workspace]
- [x] src/ manifest/ Cargo.toml 全部 git mv 到 crates/frank-cli/
- [x] cargo build / test / clippy 在 macOS 0 回归
- [ ] CI 三平台 matrix 真跑
- [ ] 加 frank-memory / frank-orchestrator / frank-sync-agent 三个 crate (ADR-003 / 004)
