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

    // 如果用户走 Homebrew 装的, 优先推荐 brew services (它会管 launchd plist + brew uninstall 时
    // 自动 stop). 这里不强制阻止 — 自部署 / Linux / Windows 用户必须靠 daemon install.
    if bin.to_string_lossy().contains("/Cellar/frank/") || bin.to_string_lossy().contains("/opt/homebrew/")
    {
        crate::log::ui::warn(
            "检测到 frank 是 Homebrew 装的 — 推荐用 `brew services start frank` 启动",
        );
        crate::log::ui::info("Homebrew 模式: 启停走 brew, `brew uninstall frank` 自动清服务");
        crate::log::ui::info("  brew services start frank      # 启 (自动注册 launchd)");
        crate::log::ui::info("  brew services list             # 状态");
        crate::log::ui::info("  brew services stop frank       # 停");
        crate::log::ui::info("");
        crate::log::ui::info("继续 `frank daemon install` 也能用 — 但会注册第二个 launchd 项, 跟");
        crate::log::ui::info("brew services 抢端口. 如果你坚持自管, 先 `brew services stop frank`.");
        crate::log::ui::info("");
    }
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

/// 给 `frank` (无 args) 调用: 检查 daemon 是否在跑, 在跑就打开浏览器, 否则提示装。
pub fn open_browser_or_hint() -> Result<()> {
    let out = Command::new("launchctl")
        .args(["list", DAEMON_LABEL])
        .output()
        .ok();
    let running = out.as_ref().is_some_and(|o| {
        o.status.success()
            && String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.contains("\"PID\"") && !l.contains("= 0;"))
    });

    if running {
        let url = format!("http://127.0.0.1:{DEFAULT_PORT}");
        crate::log::ui::info(&format!("打开浏览器 → {url}"));
        // macOS open <url>
        #[cfg(target_os = "macos")]
        let _ = Command::new("open").arg(&url).status();
        #[cfg(target_os = "linux")]
        let _ = Command::new("xdg-open").arg(&url).status();
        #[cfg(target_os = "windows")]
        let _ = Command::new("cmd").args(["/C", "start", "", &url]).status();
        Ok(())
    } else {
        crate::log::ui::warn("daemon 未跑");
        crate::log::ui::info("装 daemon: `frank daemon install` (会自启 + 永远在后台)");
        crate::log::ui::info("或临时跑: `frank orchestrator serve --bind 127.0.0.1:7780`");
        Ok(())
    }
}
