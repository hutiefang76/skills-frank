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
