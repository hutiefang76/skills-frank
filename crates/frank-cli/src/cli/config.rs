//! `frank config` — 配置管理 (proxy 等).
//!
//! 用户原话: "为了保证兼容性。直接安装 检查是否存在 clash 或者其他常见的 vpn,
//! 尝试自动加载 也可以设置不使用代理? 然后设置的范围: frank 程序的安装升级? 还可以修改?"
//!
//! # 子命令
//!
//! - `frank config show`              — 看当前 ~/.frank/config.toml 内容
//! - `frank config detect-proxy`      — 自动扫本机常见代理 (Clash/Surge/...) 写入
//! - `frank config set-proxy <url>`   — 手敲 proxy URL 写入
//! - `frank config unset-proxy`       — 清掉 proxy 配置
//!
//! # 作用域 (config.toml [proxy])
//!
//! 当前生效:
//! - `frank ai ask` spawn 的 AI CLI 子进程 (claude/codex/opencode/gemini)
//! - frank orchestrator Web UI 提交的 job (orchestrator daemon spawn 子进程)
//!
//! 暂时不影响 (留 v0.7):
//! - `frank install` git clone 走代理 (要改 git2 配置)
//! - `brew install frank` / `cargo install` (由 brew/cargo 各自配置)

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// `frank config` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// `frank config` 子命令。
#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// 看当前 ~/.frank/config.toml 内容 (proxy / sync 等).
    Show,

    /// 自动检测本机常见代理 (Clash/Surge/V2ray 等), 写入 ~/.frank/config.toml.
    DetectProxy,

    /// 手敲 proxy URL 写入 (例 `--url http://127.0.0.1:7897`).
    SetProxy {
        /// proxy URL (http/https/all 都用同一个).
        #[arg(long)]
        url: String,

        /// 不走代理的域名列表 (逗号分隔, 默认 localhost,127.0.0.1,::1,.local).
        #[arg(long, default_value = "localhost,127.0.0.1,::1,.local")]
        no: String,
    },

    /// 清掉 proxy 配置 (子进程将不走代理).
    UnsetProxy,
}

/// 执行 config 命令。
pub fn run(args: Args) -> Result<()> {
    match args.command {
        ConfigCommand::Show => show(),
        ConfigCommand::DetectProxy => detect_proxy(),
        ConfigCommand::SetProxy { url, no } => set_proxy(&url, &no),
        ConfigCommand::UnsetProxy => unset_proxy(),
    }
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join("config.toml"))
}

fn show() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!("{} 不存在 (frank config detect-proxy 自动配)", path.display()));
        return Ok(());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    crate::log::ui::section(&format!("frank config ({})", path.display()));
    println!("{text}");
    Ok(())
}

fn set_proxy(url: &str, no: &str) -> Result<()> {
    let url = url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("socks5://") {
        anyhow::bail!("proxy URL 必须以 http:// / https:// / socks5:// 开头, 收到: `{url}`");
    }
    write_proxy(url, no)?;
    crate::log::ui::success(&format!("proxy 写入 {} (http/https/all = {url})", config_path()?.display()));
    crate::log::ui::info("重启 daemon 让新 proxy 生效: brew services restart frank");
    Ok(())
}

fn unset_proxy() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn("config.toml 不存在, 没 proxy 可清");
        return Ok(());
    }
    let text = fs::read_to_string(&path).context("read config.toml")?;
    let mut v = text.parse::<toml::Value>().unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    if let Some(t) = v.as_table_mut() {
        t.remove("proxy");
    }
    let new_text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, new_text).context("write config.toml")?;
    crate::log::ui::success("proxy 已清 (子进程将不走代理)");
    Ok(())
}

fn write_proxy(url: &str, no: &str) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut v: toml::Value = existing
        .parse()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    let table = v.as_table_mut().expect("toml root is table");
    let mut proxy = toml::map::Map::new();
    proxy.insert("http".into(), toml::Value::String(url.into()));
    proxy.insert("https".into(), toml::Value::String(url.into()));
    proxy.insert("all".into(), toml::Value::String(url.into()));
    proxy.insert("no".into(), toml::Value::String(no.into()));
    table.insert("proxy".into(), toml::Value::Table(proxy));
    let text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, text).context("write config.toml")
}

fn detect_proxy() -> Result<()> {
    crate::log::ui::section("frank config detect-proxy — 扫本机常见代理");
    // 常见代理端口 (按使用率排)
    let candidates: &[(u16, &str)] = &[
        (7897, "Clash Verge / Mihomo (新版默认)"),
        (7890, "Clash X / Clash Premium (老版默认)"),
        (1087, "ShadowsocksX-NG / V2rayU (老 macOS 默认)"),
        (6152, "Surge"),
        (10809, "v2rayN (Windows)"),
        (8080, "Charles / Fiddler 调试代理"),
        (8118, "Privoxy"),
    ];
    let mut found = Vec::new();
    for (port, label) in candidates {
        if probe_port(*port) {
            found.push((*port, *label));
            crate::log::ui::info(&format!("  ✓ 127.0.0.1:{port} ({label})"));
        }
    }
    if found.is_empty() {
        crate::log::ui::warn("没扫到任何已知代理端口 — 手动配: frank config set-proxy --url http://...");
        return Ok(());
    }
    let (port, label) = found[0];
    crate::log::ui::info(&format!("选第一个候选: 127.0.0.1:{port} ({label})"));
    let url = format!("http://127.0.0.1:{port}");
    write_proxy(&url, "localhost,127.0.0.1,::1,.local")?;
    crate::log::ui::success(&format!("proxy 写入 {} (http/https/all = {url})", config_path()?.display()));
    if found.len() > 1 {
        crate::log::ui::info(&format!(
            "(另有 {} 个候选, 想换跑 `frank config set-proxy --url http://127.0.0.1:<port>`)",
            found.len() - 1
        ));
    }
    crate::log::ui::info("重启 daemon 让新 proxy 生效: brew services restart frank");
    Ok(())
}

/// 探一下 127.0.0.1:port 是否可 TCP connect (代理客户端常在 localhost 监听).
fn probe_port(port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}
