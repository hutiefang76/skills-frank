//! `frank daemon` — 把 orchestrator server 注册为系统后台服务 (用户原话 Q4).
//!
//! 用户痛点: 之前要手动 `frank orchestrator serve --bind ...`, 终端窗口阻塞,
//! 关掉就死. 产品级体验应该是装 frank 后服务自启, `frank` 命令只打开浏览器.
//!
//! # 平台
//!
//! - **macOS**: `~/Library/LaunchAgents/com.frank.orchestrator.plist` + `launchctl load`
//! - **Linux**: ~/.config/systemd/user/frank-orchestrator.service + `systemctl --user enable` (留 v0.5)
//! - **Windows**: 任务计划 / 服务管理器 (留 v0.5)
//!
//! # 子命令
//!
//! - `frank daemon install` — 装 plist + load (自启 + 立即跑)
//! - `frank daemon uninstall` — unload + 删 plist
//! - `frank daemon start` / `stop` / `restart` — 控制
//! - `frank daemon status` — 看是否在跑, 占什么端口

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

const DAEMON_LABEL: &str = "com.frank.orchestrator";
const DEFAULT_PORT: u16 = 7780;

/// `frank daemon` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: DaemonCommand,
}

/// `frank daemon` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// 注册 daemon 为系统后台服务 (登录时自启)。
    Install(InstallArgs),
    /// 移除 daemon 服务注册 (unload + 删 plist/unit)。
    Uninstall,
    /// 启动 daemon (load + 立即跑)。
    Start,
    /// 停止 daemon。
    Stop,
    /// 重启 daemon。
    Restart,
    /// 查 daemon 状态 + 监听端口。
    Status,
}

/// `frank daemon install` 参数。
#[derive(Parser, Debug)]
pub struct InstallArgs {
    /// 监听端口 (默认 7780)。
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
}

/// 执行 daemon 命令。
pub fn run(args: Args) -> Result<()> {
    // v0.7: 检测到 binary 是 Homebrew 装的, 写操作 (install/start/stop/restart/uninstall)
    // 强制 fail — 防用户像 v0.6.2 用户原话那样不小心注册第二个 launchd plist 抢端口.
    // status 是 read-only, 任意环境都允许.
    if matches!(args.command, DaemonCommand::Status) {
        return status();
    }
    if is_brew_installed() {
        bail!(
            "frank 是 Homebrew 装的, `frank daemon` 写操作禁用 — 走 brew services 统一管理:\n\
             \n\
             启停服务:\n\
             \x20 brew services start frank          # 启 (注册 launchd 自启)\n\
             \x20 brew services restart frank        # 重启\n\
             \x20 brew services stop frank           # 停\n\
             \x20 brew services list                 # 状态\n\
             \n\
             看日志:\n\
             \x20 tail -f $(brew --prefix)/var/log/frank/orchestrator.log\n\
             \n\
             卸载 frank:\n\
             \x20 brew uninstall frank               # 自动 stop service + 删 binary\n\
             \n\
             `frank daemon status` 仍然可用 (read-only 查看跑了没)."
        );
    }
    match args.command {
        DaemonCommand::Install(a) => install(a.port),
        DaemonCommand::Uninstall => uninstall(),
        DaemonCommand::Start => start(),
        DaemonCommand::Stop => stop(),
        DaemonCommand::Restart => {
            let _ = stop();
            start()
        }
        DaemonCommand::Status => status(),
    }
}

/// 检测当前 binary 是不是 Homebrew Cellar 装的.
fn is_brew_installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .is_some_and(|p| {
            let s = p.to_string_lossy().to_string();
            s.contains("/Cellar/frank/") || s.contains("/homebrew/")
        })
}

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home")?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{DAEMON_LABEL}.plist")))
}

#[cfg(not(target_os = "macos"))]
fn plist_path() -> Result<PathBuf> {
    bail!("frank daemon 仅 macOS 实现 (v0.5); Linux systemd / Windows 服务留 v0.6")
}

fn frank_binary_path() -> Result<PathBuf> {
    std::env::current_exe().context("locate frank binary")
}

fn install(port: u16) -> Result<()> {
    let plist = plist_path()?;
    let bin = frank_binary_path()?;
    let log_dir = dirs::home_dir()
        .context("home")?
        .join(".frank")
        .join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_out = log_dir.join("orchestrator.out.log");
    let log_err = log_dir.join("orchestrator.err.log");

    // launchd plist 模板 (XML, macOS 标准)
    let plist_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>orchestrator</string>
        <string>serve</string>
        <string>--bind</string>
        <string>127.0.0.1:{port}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log_out}</string>
    <key>StandardErrorPath</key>
    <string>{log_err}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
        <key>HOME</key>
        <string>{home}</string>
    </dict>
</dict>
</plist>
"#,
        label = DAEMON_LABEL,
        bin = bin.display(),
        port = port,
        log_out = log_out.display(),
        log_err = log_err.display(),
        home = dirs::home_dir().context("home")?.display(),
    );

    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    std::fs::write(&plist, plist_xml).with_context(|| format!("write {}", plist.display()))?;
    crate::log::ui::success(&format!("plist 写入 {}", plist.display()));

    // load + 让它立刻 boot
    let out = Command::new("launchctl")
        .args(["load", "-w", &plist.to_string_lossy()])
        .output()
        .context("launchctl load")?;
    if !out.status.success() {
        crate::log::ui::warn(&format!(
            "launchctl load 警告: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    crate::log::ui::success(&format!(
        "daemon 启动: http://127.0.0.1:{port} (现在可以浏览器打开, 后台运行不阻塞)"
    ));
    crate::log::ui::info("登录时自动起 (KeepAlive=true, 挂了也自动重启)");
    crate::log::ui::info(&format!("日志: tail -f {}", log_out.display()));
    Ok(())
}

fn uninstall() -> Result<()> {
    let plist = plist_path()?;
    if !plist.exists() {
        crate::log::ui::warn("plist 不存在, daemon 未装");
        return Ok(());
    }
    let out = Command::new("launchctl")
        .args(["unload", "-w", &plist.to_string_lossy()])
        .output()
        .context("launchctl unload")?;
    if !out.status.success() {
        crate::log::ui::warn(&format!(
            "launchctl unload 警告: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    std::fs::remove_file(&plist).with_context(|| format!("rm {}", plist.display()))?;
    crate::log::ui::success(&format!("daemon 卸载, plist 已删: {}", plist.display()));
    Ok(())
}

fn start() -> Result<()> {
    let plist = plist_path()?;
    if !plist.exists() {
        bail!("daemon 未装, 先跑 `frank daemon install`");
    }
    let out = Command::new("launchctl")
        .args(["start", DAEMON_LABEL])
        .output()
        .context("launchctl start")?;
    if !out.status.success() {
        bail!("启动失败: {}", String::from_utf8_lossy(&out.stderr));
    }
    crate::log::ui::success("daemon 已启动");
    Ok(())
}

fn stop() -> Result<()> {
    let out = Command::new("launchctl")
        .args(["stop", DAEMON_LABEL])
        .output()
        .context("launchctl stop")?;
    if !out.status.success() {
        crate::log::ui::warn(&format!(
            "stop 警告 (可能本来就没跑): {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    crate::log::ui::success(
        "已发 SIGTERM (KeepAlive=true → launchd 会自动重启; 永久停跑 `frank daemon uninstall`)",
    );
    Ok(())
}

fn status() -> Result<()> {
    let out = Command::new("launchctl")
        .args(["list", DAEMON_LABEL])
        .output()
        .context("launchctl list")?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout);
        // launchctl list 输出含 "PID" = 数字 means running
        let running = s
            .lines()
            .find(|l| l.contains("\"PID\""))
            .and_then(|l| l.split('=').nth(1).map(str::trim))
            .is_some_and(|p| !p.starts_with('0') && p != "0;");
        if running {
            crate::log::ui::success("daemon: ✓ running");
            crate::log::ui::info(&format!("浏览器打开: http://127.0.0.1:{DEFAULT_PORT}"));
        } else {
            crate::log::ui::warn("daemon: 已注册但未跑 (跑 `frank daemon start` 启动)");
        }
        println!("{s}");
    } else {
        crate::log::ui::warn("daemon: 未注册 (跑 `frank daemon install` 装)");
    }
    Ok(())
}

/// 给 `frank` (无 args) 调用: 显示 banner / 状态 / URL, **不自动开浏览器**。
///
/// 之前直接 `open <url>` 太突兀, 用户原话: "敲 frank cli 直接跳到 web 界面? 输出介绍和帮助,
/// 服务状态? 以及页面地址?". 现在改成 banner + 状态 + 引导, 用户自己决定要不要点 URL.
pub fn open_browser_or_hint() -> Result<()> {
    use owo_colors::{OwoColorize, Stream};

    // 探测 daemon 状态
    let (status_label, status_detail) = detect_daemon_status();

    // Banner (owo_colors 链式 .x().y() 触发 borrow 问题, 只用单色 + bold)
    println!();
    let frank_title = "frank".if_supports_color(Stream::Stdout, |t| t.bright_cyan()).to_string();
    let version_dim = format!(" v{}", env!("CARGO_PKG_VERSION"))
        .if_supports_color(Stream::Stdout, |t| t.dimmed())
        .to_string();
    println!("  {frank_title}{version_dim}    — AI 工具链治理 (skill + MCP + 跨 AI ask)");
    println!();
    // 状态
    println!("  服务状态  {status_label}  {status_detail}");
    if status_label.contains("running") {
        let url = format!("http://127.0.0.1:{DEFAULT_PORT}");
        let url_colored = url.if_supports_color(Stream::Stdout, |t| t.bright_blue()).to_string();
        println!("  Web UI    {url_colored}");
    }
    println!();
    // 常用命令
    println!("  {}", "常用命令".if_supports_color(Stream::Stdout, |t| t.bold()));
    println!("    frank ai ask --to <claude|gpt|opencode|gemini> \"...\"     跨 AI 一问一答");
    println!("    frank ai history                                          查 ask 历史");
    println!("    frank list                                                列 manifest 里全部 skill");
    println!("    frank install <name>                                      装一个 skill / MCP");
    println!("    frank scan [--mcp]                                        扫本机三平台 skill / MCP");
    println!("    frank login                                               配 sync-agent token");
    println!("    frank config show / set-proxy / detect-proxy             看 / 配 proxy");
    println!("    frank daemon install / status                             装后台服务");
    println!();
    if matches!(status_label.as_str(), "✓ running") {
        println!("  {}", "在浏览器打开 Web UI:".dimmed());
        println!(
            "    {}",
            format!("open http://127.0.0.1:{DEFAULT_PORT}").if_supports_color(Stream::Stdout, |t| t.dimmed())
        );
    } else {
        println!("  {}", "服务没跑 — 启:".dimmed());
        println!("    {}", "brew services start frank".dimmed());
        println!("    {}", "  (或 frank daemon install — 非 brew 装的)".dimmed());
    }
    println!();
    Ok(())
}

/// 探测 launchd 里 daemon 状态, 返回 (label, detail).
fn detect_daemon_status() -> (String, String) {
    // 优先看 homebrew.mxcl.frank (brew services), fallback com.frank.orchestrator (frank daemon install)
    for label in ["homebrew.mxcl.frank", DAEMON_LABEL] {
        let Ok(out) = Command::new("launchctl").args(["list", label]).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        let pid = s
            .lines()
            .find(|l| l.contains("\"PID\""))
            .and_then(|l| l.split('=').nth(1).map(str::trim))
            .and_then(|p| p.trim_end_matches(';').parse::<u32>().ok())
            .filter(|p| *p != 0);
        if let Some(pid) = pid {
            return ("✓ running".to_string(), format!("(PID {pid}, 注册名 {label})"));
        }
    }
    ("✗ not running".to_string(), "(brew services start frank 或 frank daemon install)".to_string())
}
