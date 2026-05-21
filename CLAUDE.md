# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Frank — a Rust CLI that governs AI toolchains (skills + MCP servers) across **three target platforms in parallel**: Claude Code, codex, opencode. It is *not* a library for one platform; almost every architectural choice exists to keep the three platforms in sync.

Status: P0 scaffold. `frank list` is end-to-end working; `install` / `uninstall` / `enable` / `disable` / `update` / `rollback` / `doctor` are stubs that print a warning. See `PROGRESS.md` for the current day plan and `docs/DESIGN.md` for the full 14-chapter design (the source of truth — read it before non-trivial changes).

The design doc, ADRs, and most commit messages are in Chinese. Match that style when editing those files; code identifiers and rustdoc remain English.

## Build / test / lint

```bash
cargo check --all-targets --all-features          # fast typecheck
cargo test --workspace --all-features -- --nocapture
cargo test parses_minimal_skill                   # single test by name (matches anywhere in path)
cargo test manifest::                             # all tests in a module
cargo clippy --all-targets --all-features -- -D warnings   # CI gate — must be 0 warnings
cargo fmt --all -- --check
cargo doc --no-deps --all-features                # CI runs with RUSTDOCFLAGS=-D warnings
cargo run -- list                                 # end-to-end smoke
RUST_LOG=frank=debug cargo run -- install foo     # verbose for one module
```

CI (`.github/workflows/ci.yml`) runs lint → 3-OS test matrix (ubuntu/windows/macos) → docs → audit → **secret-scan** (greps for internal IPs `10.0.* / 10.89.* / 10.90.*` and `password|secret|api_key` literals; fails the build). Don't add fixture data with those IPs to `*.rs` / `*.toml` / `*.yaml` outside `docs/`.

## Architecture

Two-layer binary: `src/main.rs` is a thin entry (init tracing → call `cli::run` → map error to `ExitCode`); all logic lives in the `frank` library (`src/lib.rs`) so a future WebUI / integration tests can reuse it.

Modules:

- **`cli/`** — clap `derive` subcommand tree. `mod.rs` defines the `Commands` enum and dispatches; one file per subcommand (`install.rs`, `list.rs`, …). Unimplemented commands hit `stub()` which prints a yellow warning rather than panicking.
- **`manifest/`** — the **configuration centre**. `schema.rs` is the serde data model (one `Manifest` per YAML file, containing many `Skill`s with `Source` / `Visibility` / `Auth` / `Platform` / `NetworkReq` / …). `parser.rs` discovers and loads manifests; `resolver.rs` exposes a `Registry` with `find` / `all` / `by_profile`. Hardcoding skill metadata anywhere else is forbidden — add to a manifest instead.
- **`adapter/`** — the `Adapter` trait (`install` / `uninstall` / `enable` / `disable` / `verify` / `platform_dir`). Per-platform impls (`claude.rs` / `codex.rs` / `opencode.rs`) are not yet written. Anything platform-specific (slash command location, yaml field differences, junction vs symlink) lives behind this trait so installer code stays platform-agnostic.
- **`installer/`** — git fetch + sparse-checkout, credential injection (keychain), junction/symlink. Placeholder; planned for P0 day 3–4.
- **`state/`** — `~/.frank/state.json` + snapshots under `~/.frank/snapshots/<ts>/`. File-locked, rotates last N=10. Placeholder.
- **`log.rs`** — **all** user-facing output must go through `log::ui::{success, info, warn, error, section}`; never use `println!` / `eprintln!` directly in business code. `tracing` is for structured logs (stderr, gated by `RUST_LOG`); `ui::*` is for the human (auto-detects TTY / NO_COLOR). This split is a hard ADR-001 requirement.

### Manifest discovery + merge

`parser::discover()` loads in this order, and `parser::merge()` lets **later override earlier** by `name`:

1. `<repo>/manifest/public.yaml` (built into the binary's working tree — found via `CARGO_MANIFEST_DIR` in dev, `<exe>/../manifest/` in installed mode)
2. `~/.frank/manifests/*.{yaml,yml}` (user private; **company skills live here, never in the repo**)
3. `$FRANK_EXTRA_MANIFEST` (single file, for tests / CI)

Three `Visibility` tiers drive auth and CI behaviour: `public` (HTTPS clone, no creds) → `own-public` (HTTPS/SSH, optional PAT) → `private` (SSH only, keychain required, must not appear in this public repo).

## Quality baselines (ADR-001 — non-negotiable)

These are enforced by `[lints]` in `Cargo.toml` and the CI lint job:

- `clippy::pedantic` warn at priority `-1`; CI runs `-D warnings`. Allowed exceptions are explicitly listed in `Cargo.toml` (e.g. `module_name_repetitions`, `doc_markdown` for Chinese docs) — don't `#[allow(...)]` ad-hoc on items; if a new exception is justified, add it to the workspace `[lints.clippy]` table.
- `#![warn(missing_docs)]` + `#![forbid(unsafe_code)]` in `lib.rs`. Every `pub` item needs `///`. `unsafe` is forbidden — find another way.
- Keep each file under ~300 lines. If a module is growing, split by responsibility (the `cli/` and `manifest/` layouts are the template).
- `rustfmt.toml`: `max_width = 100`, `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"` (std → third-party → local, blank lines between). Stable formatter; just run `cargo fmt`.

## Dependency notes

- Use `serde_yml` (active fork), not `serde_yaml` (archived 2024).
- `anyhow::Result` for fallible `pub` fns at the CLI boundary; `thiserror` for typed errors inside library modules.
- `git2` is intentionally **commented out** in `Cargo.toml` until the installer needs it — re-enable with `vendored-libgit2` so we keep zero system deps.
- `Cargo.lock` **is committed** (this is an application, not a library).

## Things to be careful with

- **Don't add real internal hostnames, IPs, or company skill URLs to `manifest/public.yaml` or any `*.rs` test fixture.** That file ships with the binary. Use `~/.frank/manifests/private-*.yaml` locally; `.gitignore` already blocks `manifest/private*.yaml`.
- **Cross-platform paths**: use `std::path::PathBuf` + `dirs::home_dir()`, never hardcode `/` or `\`. The release matrix builds for windows/macos/linux on x86_64 and aarch64.
- **`enable`/`disable` vs `install`/`uninstall`** are distinct: enable/disable toggle adapter visibility while keeping the source on disk; uninstall removes the source. Don't conflate them when implementing.
- When extending `Manifest`/`Skill` schema, bump `schema_version` only on breaking changes and keep `#[serde(default)]` on every new field for backwards compatibility (R10 risk in DESIGN.md §7.1).
