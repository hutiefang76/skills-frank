//! `frank import <name>` 子命令: 把外部 (用户手工装的) skill 收编进 frank 管理。
//!
//! 扫描三平台目录, 把所有 `status == External` 的同名条目聚合, 写一条 [`SkillState`]:
//! - `source_path` = 若是 link 用 link_target, 否则 disk_path
//! - `source_ref` = `"imported"` (不属于 git fetch)
//! - `platforms` = 找到该 name 的所有平台
//! - `enabled = true`

use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use clap::Parser;

use crate::scanner::{self, ScannedSkill, SkillStatus};
use crate::state::{SkillState, State};

/// `frank import` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要收编的 skill 名 (即 `~/.<plat>/skills/<name>/` 里的目录名)。
    pub name: String,

    /// 仅打印会发生什么, 不真写 state。
    #[arg(long)]
    pub dry_run: bool,
}

/// 执行 import 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "import invoked");

    let mut state = State::load_default()?;
    if state.get(&args.name).is_some() {
        bail!(
            "`{}` is already managed by frank (use `frank list --installed` to view); refusing to overwrite",
            args.name
        );
    }

    let scanned = scanner::scan_all(&state)?;
    let externals: Vec<&ScannedSkill> = scanned
        .iter()
        .filter(|s| s.name == args.name && s.status == SkillStatus::External)
        .collect();

    if externals.is_empty() {
        return Err(anyhow!(
            "skill `{}` not found as an external entry in any platform skills dir",
            args.name
        ));
    }

    // 用第一条作为 source_path 来源 (link target 优先, 否则 disk_path)
    let first = externals[0];
    let source_path = first
        .link_target
        .clone()
        .unwrap_or_else(|| first.disk_path.clone());
    let platforms: Vec<_> = externals.iter().map(|s| s.platform).collect();

    crate::log::ui::info(&format!(
        "found `{}` on {} platform(s): {}",
        args.name,
        platforms.len(),
        platforms
            .iter()
            .map(|p| format!("{p:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    crate::log::ui::info(&format!("source_path = {}", source_path.display()));

    if args.dry_run {
        crate::log::ui::warn("--dry-run set; state.json will NOT be modified");
        return Ok(());
    }

    state.put(SkillState {
        name: args.name.clone(),
        source_ref: "imported".to_string(),
        source_path,
        platforms,
        installed_at: Utc::now(),
        enabled: true,
    });
    state.save()?;

    crate::log::ui::success(&format!(
        "`{}` imported from {} platform(s); frank now manages it",
        args.name,
        externals.len()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parses_dry_run_flag() {
        let a = Args::try_parse_from(["import", "foo", "--dry-run"]).unwrap();
        assert_eq!(a.name, "foo");
        assert!(a.dry_run);
    }

    #[test]
    fn args_requires_name() {
        let a = Args::try_parse_from(["import"]);
        assert!(a.is_err());
    }
}
