//! `frank enable` 子命令: 重建一个已禁用 skill 的三平台链接。
//!
//! disable 时只是删了 adapter 链接, state 与 cache 都还在; enable 即按 state 里记录的
//! `source_path` 重新建链接。

use anyhow::{anyhow, Result};
use clap::Parser;

use crate::adapter;
use crate::state::State;

/// `frank enable` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要启用的 skill 名称。
    pub name: String,
}

/// 执行 enable 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "enable invoked");

    let mut state = State::load_default()?;
    let entry = state
        .get(&args.name)
        .ok_or_else(|| {
            anyhow!(
                "`{}` is not installed (run `frank install` first)",
                args.name
            )
        })?
        .clone();

    if entry.enabled && all_links_present(&entry) {
        crate::log::ui::warn(&format!("`{}` is already enabled", args.name));
        return Ok(());
    }

    for &p in &entry.platforms {
        let adp = adapter::for_platform(p);
        adp.install(&entry.name, &entry.source_path)?;
    }

    if let Some(s) = state.get_mut(&args.name) {
        s.enabled = true;
    }
    state.save()?;

    crate::log::ui::success(&format!(
        "`{}` enabled on {} platform{}",
        args.name,
        entry.platforms.len(),
        if entry.platforms.len() == 1 { "" } else { "s" }
    ));
    Ok(())
}

fn all_links_present(entry: &crate::state::SkillState) -> bool {
    entry
        .platforms
        .iter()
        .all(|&p| adapter::for_platform(p).verify(&entry.name))
}
