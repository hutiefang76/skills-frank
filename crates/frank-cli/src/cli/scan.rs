//! `frank scan` 子命令: 扫描三平台 skills 目录, 与 state 对照打表。
//!
//! 用于发现 "用户手工装的 (external)" / "state 漂移的 (managed-missing)" /
//! "重复装的 (duplicate)" — 是 [`crate::cli::import`] 与 [`crate::cli::dedupe`]
//! 的肉眼前置。

use anyhow::{anyhow, Result};
use clap::Parser;
use tabled::{Table, Tabled};

use crate::manifest::schema::Platform;
use crate::scanner::{self, ScannedSkill, SkillStatus};
use crate::state::State;

/// `frank scan` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 仅扫指定平台 (`claude` / `codex` / `opencode`)。
    #[arg(long)]
    pub platform: Option<String>,

    /// 仅显示状态为 external 的条目 (用户手工装的)。
    #[arg(long)]
    pub external_only: bool,

    /// 同时扫 MCP 配置文件 (~/.claude.json mcpServers + ~/.codex/config.toml [mcp_servers.*])
    /// 显示**所有**注册的 MCP server (含 frank 装前 / 非 frank 装的, 用户原话 Q6).
    #[arg(long)]
    pub mcp: bool,
}

#[derive(Tabled)]
struct Row {
    platform: String,
    name: String,
    status: String,
    source: String,
}

impl Row {
    fn from_scanned(s: &ScannedSkill, state: &State) -> Self {
        Self {
            platform: platform_label(s.platform).to_string(),
            name: s.name.clone(),
            status: status_label(s.status).to_string(),
            source: s.display_source(state).display().to_string(),
        }
    }
}

/// 执行 scan 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "scan invoked");

    let state = State::load_default()?;

    // ─── v0.4.3 Q6: 扫 MCP 配置 (用户原话 "托管 frank 装前的 MCP") ───
    if args.mcp {
        scan_mcp(&state)?;
        // --mcp 单独跑, 不再做 skills 扫描 (用户要么 frank scan 看 skills 要么 --mcp 看 MCP)
        return Ok(());
    }

    let all = if let Some(p) = args.platform.as_deref() {
        let plat = parse_platform(p)?;
        scanner::scan_platform(plat, &state)?
    } else {
        scanner::scan_all(&state)?
    };

    let filtered: Vec<&ScannedSkill> = all
        .iter()
        .filter(|s| !args.external_only || s.status == SkillStatus::External)
        .collect();

    if filtered.is_empty() {
        crate::log::ui::info("no skills found in any platform skills dir");
        return Ok(());
    }

    let rows: Vec<Row> = filtered
        .iter()
        .map(|s| Row::from_scanned(s, &state))
        .collect();
    crate::log::ui::section(&format!("Scanned skills ({} total)", rows.len()));
    println!("{}", Table::new(rows));

    // 帮助文案: 如果扫到 external / duplicate 给一个提示
    let external_count = all
        .iter()
        .filter(|s| s.status == SkillStatus::External)
        .count();
    if external_count > 0 {
        crate::log::ui::info(&format!(
            "{external_count} external skill(s) — use `frank import <name>` to manage them with frank"
        ));
    }
    let dup_groups = scanner::find_duplicates(&all);
    if !dup_groups.is_empty() {
        crate::log::ui::warn(&format!(
            "{} skill(s) detected with divergent platform sources — run `frank dedupe` to review",
            dup_groups.len()
        ));
    }

    Ok(())
}

fn parse_platform(s: &str) -> Result<Platform> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(Platform::Claude),
        "codex" => Ok(Platform::Codex),
        "opencode" => Ok(Platform::Opencode),
        other => Err(anyhow!(
            "unknown platform `{other}`; expected one of: claude / codex / opencode"
        )),
    }
}

/// `frank scan --mcp`: 扫两平台 MCP 配置文件, 列出全部 MCP server + 标记
/// 哪个由 frank 管理 (state.json source_ref=="mcp") vs external.
fn scan_mcp(state: &State) -> Result<()> {
    let claude = crate::installer::mcp::list_claude().unwrap_or_default();
    let codex = crate::installer::mcp::list_codex().unwrap_or_default();

    if claude.is_empty() && codex.is_empty() {
        crate::log::ui::info("no MCP servers configured (~/.claude.json 与 ~/.codex/config.toml 都没找到 mcpServers)");
        return Ok(());
    }

    #[derive(Tabled)]
    struct McpRow {
        platform: String,
        name: String,
        status: String,
        command: String,
    }

    let mut rows: Vec<McpRow> = Vec::new();
    for entry in &claude {
        let status = mcp_status(&entry.name, state);
        rows.push(McpRow {
            platform: "claude".into(),
            name: entry.name.clone(),
            status,
            command: format!(
                "{} {}",
                entry.command,
                entry.args.join(" ").chars().take(50).collect::<String>()
            ),
        });
    }
    for entry in &codex {
        let status = mcp_status(&entry.name, state);
        rows.push(McpRow {
            platform: "codex".into(),
            name: entry.name.clone(),
            status,
            command: format!(
                "{} {}",
                entry.command,
                entry.args.join(" ").chars().take(50).collect::<String>()
            ),
        });
    }
    rows.sort_by_key(|r| (r.platform.clone(), r.name.clone()));

    crate::log::ui::section(&format!("MCP servers ({} total)", rows.len()));
    println!("{}", Table::new(rows));

    let total_external = claude
        .iter()
        .chain(codex.iter())
        .filter(|e| matches!(mcp_status(&e.name, state).as_str(), "external"))
        .count();
    if total_external > 0 {
        crate::log::ui::info(&format!(
            "{total_external} external MCP — frank 未管理 (装 frank 前 / 手动加 / 其他工具加的)"
        ));
        crate::log::ui::info("用 `frank import-mcp <name>` 收编进 frank (v0.5 todo, 现在手动加 manifest)");
    }
    Ok(())
}

/// 判断 MCP 是不是由 frank install 装的 (state.json 含同名 + source_ref=mcp)。
fn mcp_status(name: &str, state: &State) -> String {
    match state.get(name) {
        Some(s) if s.source_ref == "mcp" => "managed".to_string(),
        Some(_) => "managed-but-not-mcp".to_string(), // 名字撞了
        None => "external".to_string(),
    }
}

fn platform_label(p: Platform) -> &'static str {
    match p {
        Platform::Claude => "claude",
        Platform::Codex => "codex",
        Platform::Opencode => "opencode",
    }
}

fn status_label(s: SkillStatus) -> &'static str {
    match s {
        SkillStatus::ManagedEnabled => "managed-enabled",
        SkillStatus::ManagedDisabled => "managed-disabled",
        SkillStatus::ManagedMissing => "managed-missing",
        SkillStatus::External => "external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_platform_recognises_all_three() {
        assert!(matches!(
            parse_platform("claude").unwrap(),
            Platform::Claude
        ));
        assert!(matches!(parse_platform("CODEX").unwrap(), Platform::Codex));
        assert!(matches!(
            parse_platform("opencode").unwrap(),
            Platform::Opencode
        ));
        assert!(parse_platform("foo").is_err());
    }
}
