//! `frank uninstall` 子命令: 从三平台移除链接 + 删 state 记录 + (可选) 删 git cache。
//!
//! v0.7.1 起加 `--all` (清掉所有 managed) 和 `--purge-cache` (顺手删 ~/.frank/cache/<hash>).

use anyhow::{anyhow, Result};
use clap::Parser;

use crate::installer::install as installer;
use crate::state::{SkillState, State};

/// `frank uninstall` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要卸载的 skill 名称 (跟 --all 二选一).
    pub name: Option<String>,

    /// 清掉所有 frank-managed skill / MCP (state.json 全部 entry).
    /// 用于 brew uninstall frank 前彻底清干净, 或换机器前清场.
    #[arg(long)]
    pub all: bool,

    /// 顺手删 ~/.frank/cache/<hash>/ 下的 git clone 缓存.
    /// 默认不删 (省得下次 install 同 skill 又 clone 一遍).
    #[arg(long)]
    pub purge_cache: bool,
}

/// 执行 uninstall 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "uninstall invoked");

    let mut state = State::load_default()?;

    let targets: Vec<SkillState> = if args.all {
        let v: Vec<_> = state.iter().cloned().collect();
        if v.is_empty() {
            crate::log::ui::info("没 managed skill 可卸 (state.json 空)");
            return Ok(());
        }
        crate::log::ui::warn(&format!("--all: 卸 {} 个 skill / MCP", v.len()));
        v
    } else {
        let name = args
            .name
            .as_ref()
            .ok_or_else(|| anyhow!("提供 skill name 或 --all (卸全部)"))?;
        let entry = state
            .get(name)
            .ok_or_else(|| anyhow!("`{name}` is not installed (no record in state.json)"))?;
        vec![entry.clone()]
    };

    let mut ok = 0;
    let mut failed: Vec<(String, String)> = Vec::new();
    for entry in &targets {
        match installer::uninstall_skill_mcp_aware(&entry.name, &entry.source_ref, &entry.platforms)
        {
            Ok(()) => {
                state.remove(&entry.name);
                ok += 1;
                if !args.all {
                    crate::log::ui::success(&format!(
                        "`{}` uninstalled from {} platform{}",
                        entry.name,
                        entry.platforms.len(),
                        if entry.platforms.len() == 1 { "" } else { "s" }
                    ));
                }
            }
            Err(e) => failed.push((entry.name.clone(), format!("{e:#}"))),
        }
    }
    state.save()?;

    if args.purge_cache {
        purge_cache_dir()?;
    }

    if args.all {
        crate::log::ui::success(&format!(
            "卸了 {ok} 个 skill ({} 个失败)",
            failed.len()
        ));
    }
    for (name, err) in &failed {
        crate::log::ui::error(&format!("`{name}`: {err}"));
    }
    Ok(())
}

/// 清 ~/.frank/cache/ 下全部子目录 (git clone 缓存). 不动 ~/.frank/.token / state.json / logs.
fn purge_cache_dir() -> Result<()> {
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    let cache = home.join(".frank").join("cache");
    if !cache.exists() {
        return Ok(());
    }
    let entries: Vec<_> = std::fs::read_dir(&cache)
        .map_err(|e| anyhow!("read {}: {e}", cache.display()))?
        .filter_map(Result::ok)
        .collect();
    let n = entries.len();
    for entry in entries {
        if let Err(e) = std::fs::remove_dir_all(entry.path()) {
            crate::log::ui::warn(&format!("删 {} 失败: {e}", entry.path().display()));
        }
    }
    crate::log::ui::success(&format!("git cache 已清 ({n} 个 repo, {})", cache.display()));
    Ok(())
}
