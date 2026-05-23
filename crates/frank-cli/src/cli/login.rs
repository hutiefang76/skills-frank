//! `frank login` — 配置 sync-agent 鉴权 token (用户原话 v0.5: "不该手抠 token").
//!
//! # 背景
//!
//! sync-agent 在 Caddy 层用 `X-Frank-Token` header 守 `/memory/*` `/orchestrator/*`,
//! frank-cli 找不到 token 就发不出 header → Caddy 401. 之前用户得自己 ssh 服务器
//! 抓 token + chmod 600 写 `~/.frank/.token`, 摩擦太大.
//!
//! `frank login` 把这个过程一键化:
//!
//! - `frank login --from-host tx` — ssh 到 deploy host, 从 `/opt/frank/.env`
//!   抓 `FRANK_API_TOKEN`, 写本机 `~/.frank/.token` (600 权限)
//! - `frank login --token <xxx>` — 直接手敲 token (适合 1Password / 团队分发)
//! - `frank login --show` — 看当前 token (脱敏: 前 4 + 后 4)
//! - `frank logout` — 删本机 token
//!
//! # 安全
//!
//! - 文件权限 600 (仅自己读写) — Unix only, Windows 走默认 ACL
//! - **从不 echo 完整 token** 到 stdout/log — `--show` 只显示前 4 + 后 4
//! - SSH 拉 token 时, 命令在远端跑 `grep | cut`, 不在本地 shell 历史留 token

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

/// `frank login` / `frank logout` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令 (目前只有 logout, 后续 v0.7 加 1Password / OAuth).
    #[command(subcommand)]
    pub command: Option<LoginCommand>,

    /// 直接输入 token (优先级最高, 适合手敲 / 团队分发).
    #[arg(long, conflicts_with = "from_host")]
    pub token: Option<String>,

    /// 从 SSH host (例 `tx`) 拉 `/opt/frank/.env` 里的 `FRANK_API_TOKEN`.
    ///
    /// 前提: 该 host 配了 SSH 免密 + 你部署 sync-agent 时把 token 写进
    /// `/opt/frank/.env` (`FRANK_API_TOKEN=...`).
    #[arg(long, value_name = "SSH_HOST")]
    pub from_host: Option<String>,

    /// 显示当前 token (脱敏: 前 4 + 后 4 字符).
    #[arg(long)]
    pub show: bool,
}

/// `frank login` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum LoginCommand {
    /// 移除本机已配 token (~/.frank/.token).
    Logout,
}

/// 执行 login / logout 命令。
pub fn run(args: Args) -> Result<()> {
    if matches!(args.command, Some(LoginCommand::Logout)) {
        return logout();
    }
    if args.show {
        return show();
    }
    if let Some(token) = args.token {
        return write_token(token.trim());
    }
    if let Some(host) = args.from_host {
        return login_from_host(&host);
    }
    bail!(
        "用法:\n  frank login --from-host <ssh-host>   # ssh 拉服务器 .env 里的 token\n  frank login --token <token>          # 直接手敲\n  frank login --show                   # 看当前 token (脱敏)\n  frank logout                         # 删 token"
    )
}

fn token_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join(".token"))
}

fn write_token(token: &str) -> Result<()> {
    if token.is_empty() {
        bail!("token 为空, 拒绝写入");
    }
    let path = token_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    fs::write(&path, token).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    crate::log::ui::success(&format!("token 写入 {} (600 权限)", path.display()));
    crate::log::ui::info(&format!("脱敏: {}", masked(token)));
    crate::log::ui::info("跑 `frank memory list --user <you>` 验证");
    Ok(())
}

fn login_from_host(host: &str) -> Result<()> {
    crate::log::ui::info(&format!("ssh {host} 拉 token 中..."));
    // 远端跑 grep, token 不进本地 shell history; 用 --batch 防交互式 prompt 卡住
    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            host,
            "grep '^FRANK_API_TOKEN=' /opt/frank/.env | cut -d= -f2-",
        ])
        .output()
        .context("run ssh")?;
    if !out.status.success() {
        bail!(
            "ssh {host} 失败 ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if token.is_empty() {
        bail!("ssh {host} 拉到空 token; 检查远端 /opt/frank/.env 里是否有 FRANK_API_TOKEN=... 行");
    }
    write_token(&token)
}

fn logout() -> Result<()> {
    let path = token_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("rm {}", path.display()))?;
        crate::log::ui::success(&format!("token 已删: {}", path.display()));
    } else {
        crate::log::ui::warn("token 文件不存在 (本来就没登录)");
    }
    Ok(())
}

fn show() -> Result<()> {
    let path = token_path()?;
    if !path.exists() {
        crate::log::ui::warn(
            "未登录 — 跑 `frank login --from-host tx` 或 `frank login --token <...>`",
        );
        return Ok(());
    }
    let content = fs::read_to_string(&path).context("read token")?;
    let token = content.trim();
    crate::log::ui::section("frank login 状态");
    crate::log::ui::info(&format!("文件: {}", path.display()));
    crate::log::ui::info(&format!("长度: {} 字符", token.len()));
    crate::log::ui::info(&format!("脱敏: {}", masked(token)));
    Ok(())
}

/// 脱敏: 前 4 + ... + 后 4 字符. token 太短返回 "<too short>".
fn masked(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n < 12 {
        return "<too short to mask safely>".to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail} ({n} chars)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_short_token_says_too_short() {
        assert_eq!(masked("abc"), "<too short to mask safely>");
        assert_eq!(masked("12345678901"), "<too short to mask safely>");
    }

    #[test]
    fn masked_long_token_keeps_only_4_each_side() {
        let out = masked("ecb9b0cc68a707ec3096bfb27b4e21a444d38a4413ad96fe741bab20e95259d4");
        assert!(out.starts_with("ecb9…"), "got: {out}");
        assert!(out.contains("…59d4"), "got: {out}");
        assert!(out.contains("64 chars"), "got: {out}");
    }
}
