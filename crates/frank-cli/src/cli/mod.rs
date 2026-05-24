//! CLI 命令定义与 dispatcher。
//!
//! # 设计
//!
//! 用 `clap` derive 风格定义命令树 (类似 Java picocli 的注解风格)。
//! 每个子命令一个文件 (`install.rs`, `list.rs` 等), 通过私有 `Commands` 枚举聚合,
//! 在 [`run`] 函数里 dispatch。
//!
//! 这种结构的好处:
//! - 加新命令 = 加一个文件 + 一个枚举变体, 不动其他代码
//! - 单文件 < 300 行 (质量基线)
//! - 每个命令独立可测

use anyhow::Result;
use clap::{Parser, Subcommand};

// 各子命令模块声明 (P0 day3-4: install / uninstall / enable / disable / list 已落地)
pub mod ai;
pub mod cache;
pub mod config;
pub mod ui;
pub mod update;
pub mod daemon;
pub mod dedupe;
pub mod disable;
pub mod doctor;
pub mod enable;
pub mod import;
pub mod install;
pub mod list;
pub mod login;
pub mod market;
pub mod memory;
pub mod orchestrator;
pub mod orchestrator_server;
pub mod scan;
pub mod sync;
pub mod uninstall;

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
    /// 子命令 (省略时若 daemon 在跑则打开浏览器, 否则提示装 daemon)。
    #[command(subcommand)]
    command: Option<Commands>,

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

    /// 列出已知的 skills (表格输出, 支持 --profile 过滤)。
    List(list::Args),

    /// 卸载: 从三平台移除链接 + 删 state 记录 (保留 cache)。
    Uninstall(uninstall::Args),

    /// 一行清干净所有 frank 装的东西 (skill+MCP+cache) + 引导 brew uninstall。
    Cleanup,

    /// 启用: 重建已 disabled 的链接。
    Enable(enable::Args),

    /// 禁用: 移除链接但保留 state (与 uninstall 区别: 可一键恢复)。
    Disable(disable::Args),

    /// 操作分布式记忆 (frank-sync-agent REST 客户端)。
    Memory(memory::Args),

    /// 扫描三平台 skills 目录, 与 state 对照 (managed / external / 漂移)。
    Scan(scan::Args),

    /// 把外部 (用户手工装的) skill 收编进 frank 管理。
    Import(import::Args),

    /// 检测并清理同名 skill 在多平台 target 不一致的重复安装。
    Dedupe(dedupe::Args),

    /// 环境健康检查 (toolchain / 配置 / 三平台目录 / state 漂移 / sync-agent)。
    Doctor(doctor::Args),

    /// P6 多 Agent 协作: 真接本机 claude/codex/opencode CLI (Milestone 1)。
    Orchestrator(orchestrator::Args),

    /// AI 一问一答桥 — `frank ai ask --to <provider> <prompt>`,转发给目标 CLI 拿回答。
    Ai(ai::Args),

    /// 跨设备 skills 同步 — `frank sync push/pull/devices` (用户需求 2.3)。
    Sync(sync::Args),

    /// 后台 daemon 服务管理 — `frank daemon install/start/stop/status` (Q4: 自启 orchestrator)。
    Daemon(daemon::Args),

    /// 配置 sync-agent 鉴权 token (一键 `--from-host tx` 或 `--token <xxx>`)。
    Login(login::Args),

    /// 配置管理 (`frank config show / detect-proxy / set-proxy / unset-proxy`)。
    Config(config::Args),

    /// 公开市场源 sync (`frank market sync / list` — 拉 modelcontextprotocol/servers + anthropics/skills)。
    Market(market::Args),

    /// Cache 管理 (`frank cache list / clear [name]` — 看/清 ~/.frank/cache/)。
    Cache(cache::Args),

    /// 一次性 Web UI (`frank ui` — 临时起 axum + open browser + Ctrl-C 退, 不留 daemon, 不弹 TCC)。
    Ui(ui::Args),

    /// 移除已配 token (~/.frank/.token)。
    Logout,

    /// 升级 skill: 拉最新 commit (单个或全部 enabled), 复用 `install --upgrade` 路径 (v0.9-3)。
    Update(update::Args),

    /// 回滚到上一个 snapshot (P1 待实现)。
    Rollback,
}

/// CLI 入口 dispatcher。
///
/// 由 `main.rs` 调用; 解析参数并分派到具体命令的 `run()` 函数。
/// 任何错误都向上抛出 `anyhow::Error`, 由 main 统一着色打印。
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    tracing::debug!(?cli, "parsed CLI args");

    // 用户原话 Q4: 裸 `frank` 不该阻塞终端跑 server, 应该自动打开浏览器到 daemon URL.
    // 没装 daemon 时给出明确提示让用户跑 `frank daemon install`.
    let Some(command) = cli.command else {
        return daemon::open_browser_or_hint();
    };

    match command {
        Commands::Install(args) => install::run(args),
        Commands::List(args) => list::run(args),
        Commands::Uninstall(args) => uninstall::run(args),
        Commands::Cleanup => uninstall::run_cleanup(),
        Commands::Enable(args) => enable::run(args),
        Commands::Disable(args) => disable::run(args),
        Commands::Memory(args) => memory::run(args),
        Commands::Scan(args) => scan::run(args),
        Commands::Import(args) => import::run(args),
        Commands::Dedupe(args) => dedupe::run(args),
        Commands::Doctor(args) => doctor::run(args),
        Commands::Orchestrator(args) => orchestrator::run(args),
        Commands::Ai(args) => ai::run(args),
        Commands::Sync(args) => sync::run(args),
        Commands::Daemon(args) => daemon::run(args),
        Commands::Login(args) => login::run(args),
        Commands::Config(args) => config::run(args),
        Commands::Market(args) => market::run(args),
        Commands::Cache(args) => cache::run(args),
        Commands::Ui(args) => ui::run(args),
        Commands::Logout => login::run(login::Args {
            command: Some(login::LoginCommand::Logout),
            token: None,
            from_host: None,
            show: false,
        }),
        Commands::Update(args) => update::run(args),
        Commands::Rollback => stub("rollback"),
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
