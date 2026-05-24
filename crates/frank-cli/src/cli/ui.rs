//! `frank ui` — 一次性 Web UI (v0.10.2)。
//!
//! 用户原话: "干掉 daemon 概念". 跟 `claude_codex_bridge` 一致 — 不用 launchd daemon,
//! 而是用户主动跑 `frank ui` 临时起 axum + 自动开浏览器, Ctrl-C 退出。
//!
//! # 为什么不再 daemon
//!
//! macOS TCC 弹一堆权限对话框 (Apple Music / 照片 / 下载 / 文稿) — 因为 launchd 启
//! 动的进程不继承用户 Terminal 的 TCC 授权, spawn 任何 cli subprocess 都触发 TCC。
//! 终端跑则继承终端 TCC, **永不弹**。
//!
//! # 跟 `frank orchestrator serve` 区别
//!
//! - `frank orchestrator serve --bind XXX` — 高级用户用 (可选 bind / cors / ...)
//! - `frank ui` — 日常用户的"一句话起 UI", 内部就调 serve + open browser
//!
//! # Ctrl-C 行为
//!
//! 直接接管 SIGINT, axum 自然退出 (跟 serve 一样有 graceful shutdown)。

use std::net::SocketAddr;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use super::orchestrator_server;

/// `frank ui` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// bind 地址 (默认 127.0.0.1:7780); 改端口避开占用。
    #[arg(long, default_value = "127.0.0.1:7780")]
    pub bind: String,

    /// 不自动开浏览器 (e.g. ssh 隧道 / headless 场景)。
    #[arg(long)]
    pub no_open: bool,
}

/// 执行 `frank ui`。
pub fn run(args: Args) -> Result<()> {
    let addr: SocketAddr = args
        .bind
        .parse()
        .with_context(|| format!("parse --bind {}", args.bind))?;

    let url = format!("http://{addr}");
    crate::log::ui::section("frank ui — 一次性 Web UI (Ctrl-C 退出, 不留 daemon)");
    crate::log::ui::info(&format!("→ 监听 {url}"));
    crate::log::ui::info("→ 不弹 macOS TCC (继承终端授权; brew services 起的 daemon 才会弹)");

    // 异步开 browser 不阻塞 serve. 失败只警告.
    if !args.no_open {
        let url_open = url.clone();
        std::thread::spawn(move || {
            // 等 axum 稳起来再开 (1s 足够)
            std::thread::sleep(std::time::Duration::from_millis(800));
            if let Err(e) = open_browser(&url_open) {
                eprintln!("warn: 自动开浏览器失败: {e}; 手动打开 {url_open}");
            }
        });
    }

    // tokio runtime 跑 axum
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("init tokio runtime")?;
    rt.block_on(orchestrator_server::serve(addr))
}

/// 跨平台 open URL — macOS `open`, Linux `xdg-open`, Windows `start`。
fn open_browser(url: &str) -> Result<()> {
    let (bin, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "linux") {
        ("xdg-open", vec![url])
    } else if cfg!(target_os = "windows") {
        // Windows: cmd /C start "" "url"  — start 第一个引号是 window title
        ("cmd", vec!["/C", "start", "", url])
    } else {
        return Err(anyhow!("unsupported platform"));
    };
    std::process::Command::new(bin)
        .args(&args)
        .spawn()
        .with_context(|| format!("spawn `{bin}` to open {url}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_parses_default() {
        let args = Args {
            bind: "127.0.0.1:7780".to_string(),
            no_open: true,
        };
        let addr: SocketAddr = args.bind.parse().unwrap();
        assert_eq!(addr.port(), 7780);
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn bind_parse_fails_for_garbage() {
        let r: Result<SocketAddr, _> = "not-a-bind-addr".parse();
        assert!(r.is_err());
    }
}
