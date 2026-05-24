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
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

/// `frank login` / `frank logout` 参数。
///
/// # 两类登录 (v0.10.4 ADR-009 分离)
///
/// - **sync-agent token** (默认): `frank login [--token | --from-host]` — 给 frank 后端用
/// - **provider 凭据** (新): `frank login provider <CMD>` — 给跨进程调 claude/codex/gemini/opencode 用
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令: `logout` / `provider <CMD>`。
    #[command(subcommand)]
    pub command: Option<LoginCommand>,

    /// 直接输入 sync-agent token (优先级最高, 适合手敲 / 团队分发).
    #[arg(long, conflicts_with = "from_host")]
    pub token: Option<String>,

    /// 从 SSH host (例 `tx`) 拉 `/opt/frank/.env` 里的 `FRANK_API_TOKEN`.
    ///
    /// 前提: 该 host 配了 SSH 免密 + 你部署 sync-agent 时把 token 写进
    /// `/opt/frank/.env` (`FRANK_API_TOKEN=...`).
    #[arg(long, value_name = "SSH_HOST")]
    pub from_host: Option<String>,

    /// 显示当前 sync-agent token (脱敏: 前 4 + 后 4 字符).
    #[arg(long)]
    pub show: bool,
}

/// `frank login` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum LoginCommand {
    /// 移除本机已配 sync-agent token (`~/.frank/.token`).
    Logout,

    /// 管理 provider CLI 凭据 (claude / codex / gemini / opencode)。
    ///
    /// 解决跨进程调 (Codex → frank → claude) 拿不到 Keychain token 的 ACL 问题。
    /// 详见 docs/ADR/009-cli-credential-bridge.md。
    Provider {
        /// provider 子命令
        #[command(subcommand)]
        cmd: ProviderCommand,
    },
}

/// `frank login provider` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum ProviderCommand {
    /// 列所有 provider 凭据状态 (5 层 fallback 命中表)。
    List,

    /// 删 frank store 凭据 (官方 file 不动)。
    Remove {
        /// claude / codex / gemini / opencode
        name: String,
    },

    /// 显示指定 provider 凭据状态 (脱敏)。
    Show {
        /// claude / codex / gemini / opencode
        name: String,
    },

    /// Bootstrap claude (跑 `claude setup-token` + 自动复制 token 到 frank store)。
    Claude,

    /// Bootstrap codex (跑 `codex auth login` + 自动复制 token 到 frank store)。
    Codex,

    /// Bootstrap gemini (跑 `gemini auth login` + 自动复制 token 到 frank store)。
    Gemini,

    /// Bootstrap opencode (跑 `opencode auth login` + 自动复制 token 到 frank store)。
    Opencode,
}

/// 执行 login / logout 命令。
pub fn run(args: Args) -> Result<()> {
    match args.command {
        Some(LoginCommand::Logout) => return logout(),
        Some(LoginCommand::Provider { cmd }) => return handle_provider(cmd),
        None => {}
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
    print_guide();
    Ok(())
}

// ============================================================================
// v0.10.4 ADR-009: provider 子命令组 (跨进程 CLI 凭据桥)
// ============================================================================

fn handle_provider(cmd: ProviderCommand) -> Result<()> {
    match cmd {
        ProviderCommand::List => provider_list(),
        ProviderCommand::Remove { name } => provider_remove(&name),
        ProviderCommand::Show { name } => provider_show(&name),
        ProviderCommand::Claude => provider_bootstrap(frank_cred::Provider::Claude),
        ProviderCommand::Codex => provider_bootstrap(frank_cred::Provider::Codex),
        ProviderCommand::Gemini => provider_bootstrap(frank_cred::Provider::Gemini),
        ProviderCommand::Opencode => provider_bootstrap(frank_cred::Provider::Opencode),
    }
}

/// Bootstrap: 跑 official setup 命令 + 自动复制 token 到 frank store。
fn provider_bootstrap(provider: frank_cred::Provider) -> Result<()> {
    let (bin, args) = provider.setup_command();
    crate::log::ui::section(&format!("frank login provider {provider}"));
    crate::log::ui::info(&format!("即将运行: {bin} {}", args.join(" ")));
    crate::log::ui::info(&format!(
        "完成后 frank 自动复制 token → ~/.frank/credentials/{provider}.json (mode 0600)"
    ));
    println!();

    // TTY 检测 (ADR-009 R-C2: headless 跑不通 setup-token, 给替代指引)
    if !std::io::stdin().is_terminal() {
        bail!(
            "无 TTY (ssh / 远程 / 后台 ?). {bin} {} 需要交互。\n\
             手动方案: export {}=<your-key>",
            args.join(" "),
            provider.env_var_name()
        );
    }

    let status = Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("spawn `{bin}` (是否装了? PATH 包含吗?)"))?;
    if !status.success() {
        bail!("{bin} {} 退出 {}", args.join(" "), status);
    }

    // 跑完 setup, 探 official file → 复制到 frank store
    match frank_cred::import_official_to_store(provider) {
        Ok(saved) => {
            crate::log::ui::success(&format!("token 复制到 {} (mode 0600)", saved.display()));
            crate::log::ui::info(&format!(
                "之后 frank ai ask --to {provider} 跨任意 launcher 都能用"
            ));
            Ok(())
        }
        Err(frank_cred::CredError::NotFound(_)) => {
            bail!(
                "setup 已完成但找不到 {provider} 的 official credential file. \
                 探测路径见 ADR-009. 可手动: export {}=<your-key>",
                provider.env_var_name()
            )
        }
        Err(e) => Err(e.into()),
    }
}

/// 列所有 provider 凭据状态 (5 层 fallback 命中表)。
fn provider_list() -> Result<()> {
    crate::log::ui::section("frank login provider — 凭据信任链状态");
    println!();
    println!("{:<10} {:<20} Source", "Provider", "Status");
    println!("{}", "-".repeat(60));

    for &provider in frank_cred::Provider::all() {
        let (status, source) = check_provider_status(provider);
        println!("{provider:<10} {status:<20} {source}");
    }
    println!();
    crate::log::ui::info("缺失? 跑: frank login provider <claude|codex|gemini|opencode>");
    Ok(())
}

/// 探单个 provider 状态 (用 frank-cred 的 5 层 fallback 但不注入 env)。
fn check_provider_status(provider: frank_cred::Provider) -> (String, String) {
    // 复用 5 层探测, 用 dummy Command (不实际 spawn)
    let mut probe = Command::new("true");
    match frank_cred::resolve_and_inject(&mut probe, provider) {
        Ok(report) => ("✓ 命中".to_string(), report.source.to_string()),
        Err(_) => ("✗ 缺失".to_string(), "(无)".to_string()),
    }
}

/// 显示指定 provider 凭据 (脱敏)。
fn provider_show(name: &str) -> Result<()> {
    let provider = frank_cred::Provider::parse_name(name).map_err(|e| anyhow::anyhow!(e))?;
    crate::log::ui::section(&format!("frank login provider show {provider}"));

    let mut probe = Command::new("true");
    match frank_cred::resolve_and_inject(&mut probe, provider) {
        Ok(report) => {
            crate::log::ui::info(&format!("source:   {}", report.source));
            crate::log::ui::info(&format!(
                "inject env: {} {}",
                if report.injected_env {
                    "yes"
                } else {
                    "no (OAuth session)"
                },
                report.env_var.as_deref().unwrap_or("")
            ));
            Ok(())
        }
        Err(e) => {
            crate::log::ui::warn(&format!("缺失: {e}"));
            crate::log::ui::info(&format!("跑: frank login provider {provider}"));
            Ok(())
        }
    }
}

/// 删 frank store 中的 provider 凭据 (官方 file 不动)。
fn provider_remove(name: &str) -> Result<()> {
    let provider = frank_cred::Provider::parse_name(name).map_err(|e| anyhow::anyhow!(e))?;
    let removed = frank_cred::store::remove(provider)?;
    if removed {
        crate::log::ui::success(&format!(
            "已删 frank store 中 {provider} 凭据 (官方 file 不动)"
        ));
    } else {
        crate::log::ui::warn(&format!(
            "{provider} 凭据在 frank store 中不存在 (本来就没)"
        ));
    }
    Ok(())
}

/// 无参数 `frank login` 显示的友好引导.
///
/// v0.10.4 ADR-009 起, banner 顶部明确分两类:
/// - **(1) sync-agent token** — frank 后端 memory / 跨设备
/// - **(2) provider 凭据** — 跨进程调 claude/codex/gemini/opencode 用
fn print_guide() {
    use owo_colors::{OwoColorize, Stream};

    crate::log::ui::section("frank login — 两类登录");
    println!();
    println!(
        "{}",
        "frank 有两类登录, 各管各的:".if_supports_color(Stream::Stdout, |t| t.bold())
    );
    println!();
    println!(
        "  {}  frank login [--token / --from-host / --show]",
        "(1) sync-agent token".if_supports_color(Stream::Stdout, |t| t.cyan())
    );
    println!("      给 frank 后端 (memory / 跨设备同步) 用 — 你部署的 sync-agent token");
    println!();
    println!(
        "  {}  frank login provider <claude|codex|gemini|opencode>",
        "(2) provider 凭据".if_supports_color(Stream::Stdout, |t| t.cyan())
    );
    println!("      跨进程调第三方 CLI 用 (Codex → frank → claude 链路防 Keychain ACL)");
    println!("      v0.10.4 ADR-009 新增 — frank login provider list 查状态");
    println!();
    println!(
        "{}",
        "—— (1) sync-agent token 详细用法 ——".if_supports_color(Stream::Stdout, |t| t.dimmed())
    );
    println!();
    println!(
        "  {} (从你部署的 sync-agent 拿的, 或同事给的):",
        "有 token".if_supports_color(Stream::Stdout, |t| t.bold())
    );
    println!("    frank login --token <token>");
    println!();
    println!(
        "  {} (高级):",
        "自己部署了 sync-agent + SSH 配好".if_supports_color(Stream::Stdout, |t| t.bold())
    );
    println!("    frank login --from-host <your-ssh-alias>");
    println!();
    println!(
        "  {} 部署: https://github.com/hutiefang76/skills-frank/blob/main/deploy/README.md",
        "还没 sync-agent?".if_supports_color(Stream::Stdout, |t| t.bold())
    );
    println!();
    println!("  已登录看 token (脱敏): frank login --show");
    println!("  登出:                  frank logout");
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
