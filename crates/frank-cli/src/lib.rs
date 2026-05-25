//! frank — AI 工具链治理平台核心库。
//!
//! # 模块导览
//!
//! - `cli` — 命令行解析与 dispatch (clap derive)
//! - `log` — 统一日志 + UI 着色打印 (tracing + owo-colors)
//! - `manifest` — skill / MCP 元数据 YAML 解析
//! - `adapter` — 三平台 (Claude / codex / opencode) 渲染适配器
//! - `installer` — 安装/卸载实现 (git fetch + junction/symlink)
//! - `state` — 本地状态管理 (state.json + snapshots)
//!
//! # 设计原则
//!
//! 1. **代码结构清晰**: 每模块单一职责, 文件不超过 300 行 (用户质量基线 §1)
//! 2. **注释完整**: `#![warn(missing_docs)]`, 每个 pub item 必须有文档注释 (基线 §2)
//! 3. **打印清晰**: 业务日志走 `tracing`, UI 输出走 [`log::ui`] (基线 §3)
//!
//! 详见 `docs/DESIGN.md` 与 `docs/ADR/001-language-rust.md`。

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod adapter;
pub mod cli;
pub mod installer;
pub mod log;
pub mod machine_id;
pub mod manifest;
pub mod mcp_inspect;
pub mod scanner;
pub mod state;
pub mod sync_client;

/// 项目当前版本 (与 Cargo.toml 同步)。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
