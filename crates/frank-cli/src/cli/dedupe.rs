//! `frank dedupe` 子命令: 找同名 skill 在多个平台 target 不一致的重复并清理。
//!
//! # 流程
//!
//! 1. scan_all → find_duplicates 得到一份 "name → 多平台条目" 的清单
//! 2. 默认 dry-run, 只把清单打表给用户看
//! 3. `--keep-frank-managed` 时自动保留 `ManagedEnabled`, 删其他
//! 4. 真删走 [`crate::adapter::Adapter::uninstall`] (link) 或 `fs::remove_dir_all` (真目录)
//! 5. 二次确认: 除非 `--yes`, 默认走 stdin 输入 `yes` 才执行

use std::io::{self, BufRead, Write};

use anyhow::{anyhow, Result};
use clap::Parser;
use tabled::{Table, Tabled};

use crate::adapter;
use crate::manifest::schema::Platform;
use crate::scanner::{self, ScannedSkill, SkillStatus};
use crate::state::State;

/// `frank dedupe` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 自动保留 status=managed-enabled 的条目, 删除同名的其他平台条目。
    #[arg(long)]
    pub keep_frank_managed: bool,

    /// 跳过交互确认 (生产脚本可用)。
    #[arg(long)]
    pub yes: bool,

    /// 仅处理指定平台的重复。
    #[arg(long)]
    pub platform: Option<String>,
}

#[derive(Tabled)]
struct Row {
    name: String,
    platform: String,
    status: String,
    target_or_path: String,
}

/// 执行 dedupe 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "dedupe invoked");

    let state = State::load_default()?;
    let scanned = scanner::scan_all(&state)?;

    let filtered_platform = match args.platform.as_deref() {
        Some(s) => Some(parse_platform(s)?),
        None => None,
    };

    let dups = scanner::find_duplicates(&scanned);
    if dups.is_empty() {
        crate::log::ui::success("no duplicates found across platforms");
        return Ok(());
    }

    // 打表给用户看
    let mut rows: Vec<Row> = Vec::new();
    for (name, items) in &dups {
        for s in items {
            if filtered_platform.is_some_and(|p| p != s.platform) {
                continue;
            }
            rows.push(Row {
                name: name.clone(),
                platform: platform_label(s.platform).into(),
                status: status_label(s.status).into(),
                target_or_path: s
                    .link_target
                    .clone()
                    .unwrap_or_else(|| s.disk_path.clone())
                    .display()
                    .to_string(),
            });
        }
    }
    if rows.is_empty() {
        crate::log::ui::info("no duplicates matched the platform filter");
        return Ok(());
    }
    crate::log::ui::section(&format!("Duplicate skills ({} entries)", rows.len()));
    println!("{}", Table::new(rows));

    if !args.keep_frank_managed {
        crate::log::ui::info(
            "dry-run only — pass `--keep-frank-managed` to delete non-managed copies",
        );
        return Ok(());
    }

    // 收集要删的 (非 ManagedEnabled 的同名条目)
    let to_delete: Vec<&ScannedSkill> = dups
        .values()
        .flatten()
        .filter(|s| {
            if let Some(p) = filtered_platform {
                if s.platform != p {
                    return false;
                }
            }
            s.status != SkillStatus::ManagedEnabled
        })
        .copied()
        .collect();
    if to_delete.is_empty() {
        crate::log::ui::info("nothing to delete (no non-managed duplicates)");
        return Ok(());
    }

    crate::log::ui::warn(&format!("will delete {} entry/entries:", to_delete.len()));
    for s in &to_delete {
        crate::log::ui::warn(&format!(
            "  - {} on {} ({})",
            s.name,
            platform_label(s.platform),
            s.disk_path.display()
        ));
    }

    if !args.yes && !confirm_yes()? {
        crate::log::ui::info("aborted (no 'yes' typed)");
        return Ok(());
    }

    let mut errors: Vec<String> = Vec::new();
    for s in &to_delete {
        if let Err(e) = remove_entry(s) {
            errors.push(format!("{}: {e:#}", s.disk_path.display()));
        }
    }
    if errors.is_empty() {
        crate::log::ui::success(&format!("removed {} duplicate(s)", to_delete.len()));
        Ok(())
    } else {
        Err(anyhow!(
            "dedupe encountered errors:\n  - {}",
            errors.join("\n  - ")
        ))
    }
}

fn remove_entry(s: &ScannedSkill) -> Result<()> {
    if s.is_link {
        // 走 adapter::uninstall 走 remove_link, 安全
        adapter::for_platform(s.platform).uninstall(&s.name)
    } else {
        // 真目录, 走 fs::remove_dir_all (用户外部装的)
        std::fs::remove_dir_all(&s.disk_path)
            .map_err(|e| anyhow!("remove_dir_all {}: {e}", s.disk_path.display()))
    }
}

fn confirm_yes() -> Result<bool> {
    print!("type 'yes' to confirm: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

fn parse_platform(s: &str) -> Result<Platform> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(Platform::Claude),
        "codex" => Ok(Platform::Codex),
        "opencode" => Ok(Platform::Opencode),
        other => Err(anyhow!(
            "unknown platform `{other}`; expected: claude / codex / opencode"
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
    fn args_defaults_are_safe() {
        let a = Args::try_parse_from(["dedupe"]).unwrap();
        assert!(!a.keep_frank_managed);
        assert!(!a.yes);
        assert!(a.platform.is_none());
    }

    #[test]
    fn parse_platform_recognises_codex() {
        assert!(matches!(parse_platform("codex").unwrap(), Platform::Codex));
        assert!(parse_platform("zz").is_err());
    }
}
