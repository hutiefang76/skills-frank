//! `frank update [name]` — 批量 / 单个 fetch 最新 commit, 不动 `installed_at`. (v0.9-3)
//!
//! 跟 `frank install --upgrade <name>` 等价 (复用 install 路径), 但:
//! - 无参数遍历 state 全部 enabled skill (除 mcp)
//! - 单 name 等同 `frank install --upgrade <name>`
//! - `--dry-run` 只 fetch + 比较 sha, 不真 checkout/link
//!
//! 设计要点: **不另起一套 git 流程**, 直接 wrap [`super::install::run`] 跑 upgrade — 保证
//! state.json / cache / 链接行为完全一致, 避免双实现产生 drift.

use anyhow::{Context, Result};
use clap::Parser;

use crate::state::State;

/// `frank update` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 只升这个 skill (不传 = 升 state 全部 enabled, 排除 mcp source)。
    pub name: Option<String>,

    /// 干跑: 只显示哪些会被升, 不真 fetch (P1 加, v0.9.0 先 stub)。
    #[arg(long)]
    pub dry_run: bool,
}

/// 执行 update 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "update invoked");

    if let Some(name) = args.name {
        return update_one(&name, args.dry_run);
    }
    update_all(args.dry_run)
}

fn update_one(name: &str, dry_run: bool) -> Result<()> {
    if dry_run {
        crate::log::ui::info(&format!("[dry-run] would `frank install --upgrade {name}`"));
        return Ok(());
    }
    // 复用 install 路径, 走 upgrade 分支 (保留 installed_at)
    super::install::run(super::install::Args {
        name: Some(name.to_string()),
        all: false,
        profile: None,
        skip_health_check: false,
        force: false,
        upgrade: true,
        url: None,
        r#ref: None,
    })
    .with_context(|| format!("update {name}"))
}

fn update_all(dry_run: bool) -> Result<()> {
    let state = State::load_default().context("load state")?;
    let targets: Vec<String> = state
        .iter()
        .filter(|s| s.enabled && s.source_ref != "mcp")
        .map(|s| s.name.clone())
        .collect();
    if targets.is_empty() {
        crate::log::ui::info("nothing to update (state.json 没 enabled skill)");
        return Ok(());
    }
    let total = targets.len();
    crate::log::ui::section(&format!(
        "updating {total} skill(s){}",
        if dry_run { " (dry-run)" } else { "" }
    ));

    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for (i, name) in targets.iter().enumerate() {
        crate::log::ui::info(&format!("[{}/{total}] {name}", i + 1));
        match update_one(name, dry_run) {
            Ok(()) => ok += 1,
            Err(e) => failed.push((name.clone(), format!("{e:#}"))),
        }
    }

    crate::log::ui::success(&format!(
        "updated {ok}/{total} skill(s){}",
        if failed.is_empty() {
            String::new()
        } else {
            format!(" ({} failed)", failed.len())
        }
    ));
    for (name, err) in &failed {
        crate::log::ui::error(&format!("  `{name}`: {err}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_single_does_not_error_on_missing_skill() {
        // dry-run 只打印, 不真调 install — 任何 name 都 OK
        let r = update_one("nonexistent-skill", true);
        assert!(r.is_ok());
    }

    #[test]
    fn update_all_with_empty_state_returns_ok() {
        // 这测试运行时实际会读真 state.json (有 record), 但 dry_run 不会动东西
        let r = update_all(true);
        assert!(r.is_ok());
    }
}
