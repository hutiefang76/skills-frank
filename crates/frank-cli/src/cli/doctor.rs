//! `frank doctor` 子命令 — 环境健康检查。
//!
//! 走 5 大类检查, 每类输出一条带 ✓/✗/! 前缀, 末尾给总结。不修复 — 仅诊断。
//! 修复指引交给 `install.sh` 或文档。
//!
//! # 覆盖
//!
//! 1. **toolchain** — `git`, `cargo` 命令存在 + 版本
//! 2. **配置目录** — `~/.frank/` / `state.json` / `manifests/` / `cache/`
//! 3. **三平台 skills 目录** — `~/.{claude,codex,opencode}/skills/`, 用 `scanner::scan_all`
//! 4. **state 漂移** — state 里有但盘上没 link 的 (ManagedMissing, 不可见所以 scanner 漏报)
//! 5. **sync-agent 连通** — `SyncClient::healthz` (有 token 时, 否则跳过)
//!
//! 退出码: 0 全过, 1 有任何 ✗ 或 !。

use std::process::Command;

use anyhow::Result;
use clap::Parser;

use crate::adapter;
use crate::manifest::schema::Platform;
use crate::mcp_inspect::{self, ProviderMemory, Recommendation};
use crate::scanner::{self, SkillStatus};
use crate::state::State;
use crate::sync_client::SyncClient;

/// `frank doctor` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 跳过网络相关检查 (sync-agent / git 解析)。
    #[arg(long)]
    pub offline: bool,
}

/// 单条检查结果。
struct Check {
    label: String,
    status: Status,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Check {
    fn ok(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Ok,
            detail: detail.into(),
        }
    }
    fn warn(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Warn,
            detail: detail.into(),
        }
    }
    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Fail,
            detail: detail.into(),
        }
    }
}

/// 执行 doctor 命令。
pub fn run(args: Args) -> Result<()> {
    crate::log::ui::section("frank doctor");

    let mut checks: Vec<Check> = Vec::new();

    checks.extend(check_toolchain());
    checks.extend(check_frank_home());
    checks.extend(check_platform_dirs());
    checks.extend(check_state_drift());
    checks.extend(check_local_daemon());
    checks.extend(check_credentials()); // v0.10.4 ADR-009: 凭据信任链
    if !args.offline {
        checks.extend(check_sync_agent());
    }

    let mut ok_count = 0usize;
    let mut warn_count = 0usize;
    let mut fail_count = 0usize;
    for c in &checks {
        let prefix = match c.status {
            Status::Ok => {
                ok_count += 1;
                "✓"
            }
            Status::Warn => {
                warn_count += 1;
                "!"
            }
            Status::Fail => {
                fail_count += 1;
                "✗"
            }
        };
        println!("  {prefix} {:<28} {}", c.label, c.detail);
    }

    // v0.10.6 P2 D3: 单独节, 展示 4 provider 的 MCP memory 状态 + 推荐
    print_memory_inspection();

    println!();
    println!(
        "  Summary: {ok_count} ok, {warn_count} warn, {fail_count} fail (total {})",
        checks.len()
    );

    if fail_count > 0 || warn_count > 0 {
        crate::log::ui::warn("see install.sh / README for fixes");
        std::process::exit(1);
    }
    crate::log::ui::success("all checks passed");
    Ok(())
}

/// v0.10.6 P2 D3: `## 记忆系统全景` 节 — 展示 4 provider MCP memory 探测 + 推荐。
///
/// 不算入 ok/warn/fail count (它是参考信息, 不是 pass/fail 检查), 仅给用户
/// "现在 4 provider 各自记忆 MCP 状态" 一览, 与"建议下一步"。
fn print_memory_inspection() {
    let setups = mcp_inspect::inspect_all();
    let rec = mcp_inspect::recommend(&setups);

    crate::log::ui::section("## 记忆系统全景");
    for s in &setups {
        let path_label = s
            .config_path
            .as_ref()
            .map_or_else(|| "(no home)".to_string(), |p| short_path(p));
        let mem_label = memory_status_label(s);
        println!("  • {:<10} {:<38} {mem_label}", s.provider.name(), path_label);
    }

    println!();
    let frank_label = if mcp_inspect::frank_cli_in_path() {
        "frank CLI: 在 PATH"
    } else {
        "frank CLI: 不在 PATH"
    };
    println!("  ─ {frank_label} ─ 推荐: {}", rec.summary());

    // DisableOfficial: 对每个装了 official 的 provider 给禁用命令
    if matches!(rec, Recommendation::DisableOfficial) {
        for s in &setups {
            if let Some(off) = &s.official_mcp {
                println!(
                    "    → {}",
                    Recommendation::disable_hint(s.provider, &off.entry_name)
                );
            }
        }
    }
}

/// 把 `~/.gemini/settings.json` 这种长 path 压缩成 `~/.gemini/settings.json` 给终端表显示。
fn short_path(p: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(stripped) = p.strip_prefix(&home) {
            return format!("~/{}", stripped.display());
        }
    }
    p.display().to_string()
}

/// 单 provider 的"memory MCP 状态"短标签。
fn memory_status_label(s: &ProviderMemory) -> String {
    match (&s.official_mcp, &s.frank_mcp) {
        (Some(off), _) => {
            let suffix = if off.disabled { " (disabled)" } else { "" };
            format!("official: {}{suffix}", off.entry_name)
        }
        (None, Some(_)) => "frank-mcp 已装".to_string(),
        (None, None) => "无 memory MCP".to_string(),
    }
}

// ---- 各类检查 ----

fn check_toolchain() -> Vec<Check> {
    let mut out = Vec::new();
    // 必装 (用户直接调或 frank install 后续 cargo build 等可能需要)
    for (name, args) in [("git", &["--version"][..]), ("cargo", &["--version"][..])] {
        let res = Command::new(name).args(args).output();
        match res {
            Ok(o) if o.status.success() => {
                let v = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("(no output)")
                    .to_string();
                out.push(Check::ok(format!("toolchain {name}"), v));
            }
            _ => out.push(Check::warn(
                format!("toolchain {name}"),
                format!("{name} not in PATH (frank install 用 libgit2 仍 work, 但建议装 `brew install {name}`)"),
            )),
        }
    }
    // 可选 (装上更好, 不装也 fallback)
    for (name, args, hint) in [
        (
            "gh",
            &["--version"][..],
            "GitHub CLI — frank market 用它的 token 防 60/h 限速, 装: `brew install gh && gh auth login`",
        ),
        (
            "curl",
            &["--version"][..],
            "用于 install.sh / 健康检查 — macOS/linux 一般自带",
        ),
    ] {
        let res = Command::new(name).args(args).output();
        match res {
            Ok(o) if o.status.success() => {
                let v = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("(no output)")
                    .to_string();
                out.push(Check::ok(format!("optional {name}"), v));
            }
            _ => out.push(Check::warn(format!("optional {name}"), hint.to_string())),
        }
    }
    out
}

fn check_frank_home() -> Vec<Check> {
    let Some(home) = dirs::home_dir() else {
        return vec![Check::fail("frank home", "no home dir".to_string())];
    };
    let frank = home.join(".frank");
    let state = frank.join("state.json");
    let manifests = frank.join("manifests");
    let cache = frank.join("cache");

    vec![
        if frank.exists() {
            Check::ok("~/.frank/", "exists".to_string())
        } else {
            Check::warn(
                "~/.frank/",
                "missing; install.sh 会建; 或手动 mkdir -p ~/.frank/manifests".to_string(),
            )
        },
        if state.exists() {
            let n = State::load(state.clone()).map_or(0, |s| s.len());
            Check::ok("state.json", format!("{n} skill(s) tracked"))
        } else {
            Check::warn("state.json", "missing (空; install 一个 skill 后会建)")
        },
        if manifests.exists() {
            let n = std::fs::read_dir(&manifests).map_or(0, |d| {
                d.filter_map(Result::ok)
                    .filter(|e| {
                        e.path()
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e == "yaml" || e == "yml")
                    })
                    .count()
            });
            Check::ok("manifests/", format!("{n} private manifest(s)"))
        } else {
            Check::warn(
                "manifests/",
                "missing (公司 / 私有 skill 放这里, 见 docs/ADR/003)",
            )
        },
        if cache.exists() {
            let n = std::fs::read_dir(&cache).map_or(0, Iterator::count);
            Check::ok("cache/", format!("{n} cached repo(s)"))
        } else {
            Check::ok("cache/", "empty (首次 install 后会建)")
        },
    ]
}

fn check_platform_dirs() -> Vec<Check> {
    let mut out = Vec::new();
    for p in [Platform::Claude, Platform::Codex, Platform::Opencode] {
        let adp = adapter::for_platform(p);
        let dir = adp.platform_dir();
        if dir.exists() {
            let n = std::fs::read_dir(&dir).map_or(0, Iterator::count);
            out.push(Check::ok(
                format!("platform {}", adp.name()),
                format!("{} ({n} skill(s))", dir.display()),
            ));
        } else {
            out.push(Check::warn(
                format!("platform {}", adp.name()),
                format!("{} missing (该平台未装?)", dir.display()),
            ));
        }
    }
    out
}

fn check_state_drift() -> Vec<Check> {
    let Ok(state) = State::load_default() else {
        return vec![Check::warn("state drift", "load state failed")];
    };
    let scanned = match scanner::scan_all(&state) {
        Ok(v) => v,
        Err(e) => return vec![Check::warn("state drift", format!("scan failed: {e:#}"))],
    };

    let mut missing: Vec<&str> = Vec::new();
    for s in &scanned {
        if s.status == SkillStatus::ManagedMissing {
            missing.push(s.name.as_str());
        }
    }
    // P2-3 followup: scan 只看 disk → state, 漏报 "state 有 disk 没" 的 drift.
    // 这里反向遍历一遍补全.
    //
    // v0.6 修: MCP server (source_ref = "mcp") **不在 ~/.{claude,codex,opencode}/skills/** 目录,
    // 它们写到 ~/.claude.json mcpServers + ~/.codex/config.toml [mcp_servers.*]. scan_all 永远
    // 不会 return MCP entry → 之前误报 mcp-time 为 orphan. 这里跳过 mcp 类 source 避免误报.
    let scanned_names: std::collections::HashSet<&str> =
        scanned.iter().map(|s| s.name.as_str()).collect();
    let mut state_orphans: Vec<String> = Vec::new();
    for entry in state.iter().filter(|e| e.enabled) {
        if entry.source_ref == "mcp" {
            continue; // MCP server 装到平台 config 文件不是 skills/, skip drift check.
        }
        let sanitized = adapter::sanitize_name(&entry.name);
        if !scanned_names.contains(entry.name.as_str())
            && !scanned_names.contains(sanitized.as_str())
        {
            state_orphans.push(entry.name.clone());
        }
    }

    if missing.is_empty() && state_orphans.is_empty() {
        vec![Check::ok("state drift", "no drift")]
    } else {
        use std::fmt::Write as _;
        let mut detail = String::new();
        if !missing.is_empty() {
            let _ = write!(detail, "ManagedMissing: {}", missing.join(", "));
        }
        if !state_orphans.is_empty() {
            if !detail.is_empty() {
                detail.push_str("; ");
            }
            let _ = write!(detail, "orphan in state: {}", state_orphans.join(", "));
        }
        vec![Check::warn("state drift", detail)]
    }
}

/// 本机 daemon (orchestrator serve) 状态. 开/不开都不算 fail — daemon 是可选的.
/// 仅给用户一个 "Web UI 可用 / 不可用" 的可见提示.
fn check_local_daemon() -> Vec<Check> {
    let url = "http://127.0.0.1:7780/";
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
    else {
        return vec![Check::ok("daemon", "(skip: http client init failed)")];
    };
    match client.get(url).send() {
        Ok(_) => vec![Check::ok(
            "daemon",
            "running on 127.0.0.1:7780 (Web UI 可用)",
        )],
        Err(_) => vec![Check::ok(
            "daemon",
            "off (CLI 全部命令可用; 想要 Web UI 跑 `brew services start frank`)",
        )],
    }
}

/// v0.10.4 ADR-009: 凭据信任链状态 — 对每个 provider 探 5 层 fallback。
///
/// 不实际 spawn child, 仅探测能否拿到 token + 显示命中层 + 提示。
fn check_credentials() -> Vec<Check> {
    use std::process::Command;
    let mut out = Vec::new();

    for &provider in frank_cred::Provider::all() {
        let label = format!("credential: {provider}");
        // dummy command (不会 spawn) — 只借 resolve_and_inject 探 fallback chain
        let mut probe = Command::new("true");
        match frank_cred::resolve_and_inject(&mut probe, provider) {
            Ok(report) => {
                let detail = format!("✓ {} ({})", report.source, summarize_inject(&report));
                out.push(Check {
                    label,
                    status: Status::Ok,
                    detail,
                });
            }
            Err(_) => {
                out.push(Check {
                    label,
                    status: Status::Warn,
                    detail: format!(
                        "5 层 fallback 全 miss — 跑 `frank login provider {provider}` 配置"
                    ),
                });
            }
        }
    }

    out
}

fn summarize_inject(r: &frank_cred::InjectReport) -> String {
    if r.injected_env {
        format!("inject env: {}", r.env_var.as_deref().unwrap_or("?"))
    } else {
        "no env inject (OAuth file mode)".to_string()
    }
}

fn check_sync_agent() -> Vec<Check> {
    let client = match SyncClient::from_env_or_config() {
        Ok(c) => c,
        Err(e) => return vec![Check::warn("sync-agent", format!("client init: {e:#}"))],
    };
    match client.healthz() {
        Ok(body) => vec![Check::ok(
            "sync-agent",
            format!("{} → {body}", client.base_url()),
        )],
        Err(e) => vec![Check::warn(
            "sync-agent",
            format!("{} unreachable: {e:#}", client.base_url()),
        )],
    }
}
