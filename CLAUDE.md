# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Frank — a Rust CLI that governs AI toolchains (skills + MCP servers) across **three target platforms in parallel**: Claude Code, codex, opencode. It is *not* a library for one platform; almost every architectural choice exists to keep the three platforms in sync.

The repo is a **Cargo workspace** (ADR-002). Four crates under `crates/`:

- **`frank-cli/`** — CLI 主二进制 (P0, 已端到端). 历史上文中 `src/...` 路径全部指 `crates/frank-cli/src/...`.
- **`frank-memory/`** — 分布式记忆库 (P5,进行中). mem0 同思路的 Rust 重写:Qdrant 向量库 + LLM 事实抽取 + 高层 `Memory` API. 详见 ADR-003.
- **`frank-sync-agent/`** — 服务端 binary (P5). axum REST + WS, 跑在 tx:8318 (Docker Compose: caddy + qdrant + 本服务). 详见 ADR-005.
- **`frank-orchestrator/`** — 多 AI Agent 协作总线 (P6,骨架就绪 / 实现待启动). 替代 CCB tmux 路线,Web UI + axum WS API. 详见 ADR-004.

Status: P0 完整 (install/uninstall/enable/disable/list 真跑通) + P5 进行中 (frank-memory 骨架 14 单测全绿; qdrant 已部署) + P6 设计完成. `update` / `rollback` / `doctor` 仍是 stub. See `PROGRESS.md` for the current day plan and `docs/DESIGN.md` for the full design (the source of truth — read it before non-trivial changes).

The design doc, ADRs, and most commit messages are in Chinese. Match that style when editing those files; code identifiers and rustdoc remain English.

## Build / test / lint

```bash
cargo check --all-targets --all-features          # fast typecheck
cargo test --workspace --all-features -- --nocapture
cargo test parses_minimal_skill                   # single test by name (matches anywhere in path)
cargo test manifest::                             # all tests in a module
cargo clippy --workspace --all-targets --all-features -- -D warnings   # CI gate — 0 warning across all crates
cargo fmt --all -- --check
cargo doc --no-deps --all-features                # CI runs with RUSTDOCFLAGS=-D warnings
cargo run -- list                                 # end-to-end smoke (frank-cli)
RUST_LOG=frank=debug cargo run -- install foo     # verbose for one module
```

`cargo install frank` 仍然有效 (workspace 根有 `[workspace] members = ["crates/*"]`,cargo 会自动定位 `crates/frank-cli/`)。**Workspace 命令**:`cargo test --workspace`, `cargo clippy --workspace` 一次跑全部 crate。运行单个子 crate 的测试: `cargo test -p frank-memory`。

CI (`.github/workflows/ci.yml`) runs lint → 3-OS test matrix (ubuntu/windows/macos) → docs → audit → **secret-scan** (greps for internal IPs `10.0.* / 10.89.* / 10.90.*` and `password|secret|api_key` literals; fails the build). Don't add fixture data with those IPs to `*.rs` / `*.toml` / `*.yaml` outside `docs/`.

## Architecture

(paths below are inside `crates/frank-cli/`.)

Two-layer binary: `crates/frank-cli/src/main.rs` is a thin entry (init tracing → call `cli::run` → map error to `ExitCode`); all logic lives in the `frank` library (`crates/frank-cli/src/lib.rs`) so a future WebUI / integration tests can reuse it.

Modules:

- **`cli/`** — clap `derive` subcommand tree. `mod.rs` defines the `Commands` enum and dispatches; one file per subcommand (`install.rs`, `list.rs`, …). Unimplemented commands hit `stub()` which prints a yellow warning rather than panicking.
- **`manifest/`** — the **configuration centre**. `schema.rs` is the serde data model (one `Manifest` per YAML file, containing many `Skill`s with `Source` / `Visibility` / `Auth` / `Platform` / `NetworkReq` / …). `parser.rs` discovers and loads manifests; `resolver.rs` exposes a `Registry` with `find` / `all` / `by_profile`. Hardcoding skill metadata anywhere else is forbidden — add to a manifest instead.
- **`adapter/`** — the `Adapter` trait (`install` / `uninstall` / `enable` / `disable` / `verify` / `platform_dir`). Per-platform impls (`claude.rs` / `codex.rs` / `opencode.rs`) share a `link_install` / `link_uninstall` / `link_verify` helper (P0 day 3-4, 已落地). Anything platform-specific (slash command location, yaml field differences, junction vs symlink) lives behind this trait so installer code stays platform-agnostic.
- **`installer/`** — `git.rs` (git2 clone/fetch/checkout + sha256 cache key), `link.rs` (跨平台 symlink), `install.rs` (编排: device_allowlist → fetch → subpath → adapter 分发 → 失败回滚). Credentials injection (keychain) 待 private skill 触发时加。P0 day 3-4 已落地。
- **`state/`** — `~/.frank/state.json` (StateData/SkillState + 原子 tmp+rename, load/save/get/put/remove/iter). Snapshots 在 P1。P0 day 3-4 已落地。
- **`sync_client.rs`** — frank-cli 调 frank-sync-agent 的 REST blocking 客户端 (P5 联动, 用于 memory 子命令; 详见下方 Cross-crate dependencies 节)。
- **`log.rs`** — **all** user-facing output must go through `log::ui::{success, info, warn, error, section}`; never use `println!` / `eprintln!` directly in business code. `tracing` is for structured logs (stderr, gated by `RUST_LOG`); `ui::*` is for the human (auto-detects TTY / NO_COLOR). This split is a hard ADR-001 requirement.

### Manifest discovery + merge

`parser::discover()` loads in this order, and `parser::merge()` lets **later override earlier** by `name`:

1. `<repo>/manifest/public.yaml` (built into the binary's working tree — found via `CARGO_MANIFEST_DIR` in dev, `<exe>/../manifest/` in installed mode)
2. `~/.frank/manifests/*.{yaml,yml}` (user private; **company skills live here, never in the repo**)
3. `$FRANK_EXTRA_MANIFEST` (single file, for tests / CI)

**Visibility — 两层 5 档** (v0.2):

- **Layer 1: frank 内置** (项目作者 hutiefang76 维护, 装 frank 默认就有)
  - `frank-own` — 芳哥自研开源 skills
  - `frank-recommended` — 芳哥推荐的 upstream / 第三方 (如 anthropics/*)
- **Layer 2: 用户自定义** (用户自己 manifest 加, 跟项目作者无关 — `user-company` 是**用户的公司** 不是 frank 项目的)
  - `user-public` — 用户的开源 skills
  - `user-company` — 用户的公司 skills (严禁混入本仓 — 走 `~/.frank/manifests/`)
  - `user-private` — 用户的私有 skills

老 v0.1 `public` / `own-public` / `private` 通过 `#[serde(alias)]` 兼容老 manifest, 不破任何老配置。

## Quality baselines (ADR-001 — non-negotiable)

These are enforced by `[lints]` in `Cargo.toml` and the CI lint job:

- `clippy::pedantic` warn at priority `-1`; CI runs `-D warnings`. Allowed exceptions are explicitly listed in `Cargo.toml` (e.g. `module_name_repetitions`, `doc_markdown` for Chinese docs) — don't `#[allow(...)]` ad-hoc on items; if a new exception is justified, add it to the workspace `[lints.clippy]` table.
- `#![warn(missing_docs)]` + `#![forbid(unsafe_code)]` in `lib.rs`. Every `pub` item needs `///`. `unsafe` is forbidden — find another way.
- Keep each file under ~300 lines. If a module is growing, split by responsibility (the `cli/` and `manifest/` layouts are the template).
- `rustfmt.toml`: `max_width = 100`, `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"` (std → third-party → local, blank lines between). Stable formatter; just run `cargo fmt`.

## Dependency notes

- Use `serde_yml` (active fork), not `serde_yaml` (archived 2024).
- `anyhow::Result` for fallible `pub` fns at the CLI boundary; `thiserror` for typed errors inside library modules.
- `git2` 启用了 `vendored-libgit2` + `vendored-openssl` (无系统依赖,跨平台编译干净). 历史上 P0 day1-2 曾注释掉,P0 day3-4 起已开启,见 PROGRESS.md。
- `Cargo.lock` **is committed** (this is an application, not a library).
- 子 crate 共享依赖在 root `Cargo.toml` 的 `[workspace.dependencies]` 集中声明; 子 crate 用 `dep = { workspace = true }` 引用。单 crate 专属依赖(如 `clap` 只在 frank-cli, `axum` 只在 frank-sync-agent)保留在该 crate 的 `[dependencies]`。

## Cross-crate dependencies

- **`frank-cli`** 通过 `path = "../frank-memory"` 引用 **`frank-memory`**:复用 `Scope` / `MemoryRecord` / `MemoryMatch` / `MemoryId` 类型,避免双重定义。
- **`frank-cli`** 通过 **`reqwest` blocking** (rustls + webpki-roots,无 async runtime) 调 **`frank-sync-agent`** 的 REST 端点。具体见 `crates/frank-cli/src/sync_client.rs`。
- **`frank-sync-agent`** 通过 `path = "../frank-memory"` 引用 **`frank-memory`**:服务端把 `Memory` 高层 API 包装成 axum REST 路由 (`/memory/add` 等)。
- **`frank-orchestrator`** (P6) 计划同样 path 引用 frank-memory (跨 job 经验召回),并在 sync-agent 同一 binary 内挂载 `/orchestrator/*` 路由。

发版时 path 依赖需要切回 crates.io version,见 ADR-002 "影响 / 迁移" 节。

## Things to be careful with

- **Don't add real internal hostnames, IPs, or company skill URLs to `manifest/public.yaml` or any `*.rs` test fixture.** That file ships with the binary. Use `~/.frank/manifests/private-*.yaml` locally; `.gitignore` already blocks `manifest/private*.yaml`.
- **Cross-platform paths**: use `std::path::PathBuf` + `dirs::home_dir()`, never hardcode `/` or `\`. The release matrix builds for windows/macos/linux on x86_64 and aarch64.
- **`enable`/`disable` vs `install`/`uninstall`** are distinct: enable/disable toggle adapter visibility while keeping the source on disk; uninstall removes the source. Don't conflate them when implementing.
- When extending `Manifest`/`Skill` schema, bump `schema_version` only on breaking changes and keep `#[serde(default)]` on every new field for backwards compatibility (R10 risk in DESIGN.md §7.1).

## 部署

服务端 stack (caddy + qdrant + 后续 frank-sync-agent + postgres) 跑在腾讯云 VM `tx`, 唯一外网端口 `8318` (8317 被既有 cli-proxy-api 占用)。详见 `deploy/README.md` + `docs/ADR/005-deploy-tencent-8317.md`。Docker Compose 文件在 `deploy/docker-compose.yml`,Caddyfile 在 `deploy/Caddyfile`。
