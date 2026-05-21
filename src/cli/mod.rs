//! CLI 命令定义与 dispatcher。
//!
//! # 设计
//!
//! 用 `clap` derive 风格定义命令树 (类似 Java picocli 的注解风格)。
//! 每个子命令一个文件 (`install.rs`, `list.rs` 等), 通过 [`Commands`] 枚举聚合,
//! 在 [`run`] 函数里 dispatch。
//!
//! 这种结构的好处:
//! - 加新命令 = 加一个文件 + 一个枚举变体, 不动其他代码
//! - 单文件 < 300 行 (质量基线)
//! - 每个命令独立可测

use anyhow::Result;
use clap::{Parser, Subcommand};

// 各子命令模块声明 (P0 day1: 仅骨架, 实现逐步填充)
pub mod install;

/// frank — AI 工具链治理平台 CLI。
///
/// 统一管理 Claude Code / codex / opencode 三平台的 skills 与 MCP。
#[derive(Parser, Debug)]
#[command(
    name = "frank",
    version,
    about = "AI toolchain governance: skill/MCP management across Claude / codex / opencode",
    long_about = None,
    propagate_version = true,
)]
struct Cli {
    /// 子命令。
    #[command(subcommand)]
    command: Commands,

    /// 启用详细日志 (等价于 RUST_LOG=debug)。
    #[arg(short, long, global = true)]
    verbose: bool,
}

/// frank 支持的子命令清单。
///
/// 每个变体的文档注释会自动出现在 `frank <cmd> --help` 中, 所以注释要面向用户写。
#[derive(Subcommand, Debug)]
enum Commands {
    /// 安装一个 skill 或 MCP server。
    ///
    /// 解析 manifest, 拉取源码, 渲染到三平台目录。
    Install(install::Args),

    // ----- 以下为占位, P0 后续 day 实现 -----
    /// 卸载 (P0 待实现)。
    Uninstall,
    /// 列出已知的 skills (P0 待实现)。
    List,
    /// 启用一个已安装的 skill (P0 待实现)。
    Enable,
    /// 禁用一个已安装的 skill (P0 待实现)。
    Disable,
    /// 升级到最新版本 (P1 待实现)。
    Update,
    /// 回滚到上一个 snapshot (P1 待实现)。
    Rollback,
    /// 健康检查 (P1 待实现)。
    Doctor,
}

/// CLI 入口 dispatcher。
///
/// 由 `main.rs` 调用; 解析参数并分派到具体命令的 `run()` 函数。
/// 任何错误都向上抛出 `anyhow::Error`, 由 main 统一着色打印。
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    tracing::debug!(?cli, "parsed CLI args");

    match cli.command {
        Commands::Install(args) => install::run(args),
        Commands::Uninstall => stub("uninstall"),
        Commands::List => stub("list"),
        Commands::Enable => stub("enable"),
        Commands::Disable => stub("disable"),
        Commands::Update => stub("update"),
        Commands::Rollback => stub("rollback"),
        Commands::Doctor => stub("doctor"),
    }
}

/// 占位命令: P0 后续 day 实现。
///
/// 打印一条友好提示而非 panic, 让用户知道命令存在但还没到。
fn stub(name: &str) -> Result<()> {
    crate::log::ui::warn(&format!(
        "`frank {name}` not yet implemented (P0 scaffolding); see docs/DESIGN.md §10 for roadmap"
    ));
    Ok(())
}
