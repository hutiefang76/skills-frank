//! `frank uninstall` 子命令: 从三平台移除链接 + 删 state 记录。
//!
//! 不动 `~/.frank/cache/` 下的源码 (留给 reuse / `frank update` 复用); 真要清缓存
//! 用 `frank doctor --prune` (P1)。

use anyhow::{anyhow, Result};
use clap::Parser;

use crate::installer::install as installer;
use crate::state::State;

/// `frank uninstall` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要卸载的 skill 名称。
    pub name: String,
}

/// 执行 uninstall 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "uninstall invoked");

    let mut state = State::load_default()?;
    let entry = state
        .get(&args.name)
        .ok_or_else(|| anyhow!("`{}` is not installed (no record in state.json)", args.name))?
        .clone();

    installer::uninstall_skill(&entry.name, &entry.platforms)?;
    state.remove(&args.name);
    state.save()?;

    crate::log::ui::success(&format!(
        "`{}` uninstalled from {} platform{}",
        args.name,
        entry.platforms.len(),
        if entry.platforms.len() == 1 { "" } else { "s" }
    ));
    Ok(())
}
