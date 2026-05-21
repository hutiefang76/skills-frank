//! `frank disable` 子命令: 移除三平台链接, 保留 state 记录 (标 enabled=false)。
//!
//! 与 `uninstall` 区别: uninstall 会从 state 里删掉记录, disable 只是"暂时关掉"
//! 等 `frank enable` 一键打开。cache 与 state 都不动。

use anyhow::{anyhow, Result};
use clap::Parser;

use crate::installer::install as installer;
use crate::state::State;

/// `frank disable` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要禁用的 skill 名称。
    pub name: String,
}

/// 执行 disable 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "disable invoked");

    let mut state = State::load_default()?;
    let entry = state
        .get(&args.name)
        .ok_or_else(|| anyhow!("`{}` is not installed", args.name))?
        .clone();

    if !entry.enabled {
        crate::log::ui::warn(&format!("`{}` is already disabled", args.name));
        return Ok(());
    }

    installer::uninstall_skill(&entry.name, &entry.platforms)?;

    if let Some(s) = state.get_mut(&args.name) {
        s.enabled = false;
    }
    state.save()?;

    crate::log::ui::success(&format!(
        "`{}` disabled (state preserved; run `frank enable {}` to reactivate)",
        args.name, args.name
    ));
    Ok(())
}
