//! `frank uninstall` 子命令: 从三平台移除链接 + 删 state 记录 + (可选) 删 git cache。
//!
//! # v0.7.3 产品定义重新对齐 (用户原话: "frank uninstall 直接全部删除, 第三方 skills 不要管理")
//!
//! - `frank uninstall` — **默认** 清 frank 官方装的 (frank-official + frank-recommended)
//!   + git cache. 用户 --url 装的 community/team/private **不动**.
//! - `frank uninstall <name>` — 单卸某个 (任何 visibility 都行).
//! - `frank uninstall --including-3rd-party` — 也清 community/team/private.
//! - `frank uninstall --keep-cache` — 不删 ~/.frank/cache/<hash>/.

use anyhow::{anyhow, Result};
use clap::Parser;

use crate::installer::install as installer;
use crate::manifest::schema::Visibility;
use crate::state::{SkillState, State};

/// `frank uninstall` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要卸载的 skill 名称 (不传 = 清 frank 官方的全部).
    pub name: Option<String>,

    /// 也清第三方 skill (community/team/private, 用户 frank install --url 装的).
    /// 默认不动这些 — 用户自己装的, 用户自己负责卸载.
    #[arg(long)]
    pub including_3rd_party: bool,

    /// 不删 ~/.frank/cache/<hash>/ 下的 git clone 缓存.
    /// 默认 **删** (跟 v0.7.1 反着, 用户原话 "直接全部删除").
    #[arg(long)]
    pub keep_cache: bool,
}

/// `frank cleanup` — 一行清 frank 官方装的 (frank-official + frank-recommended) + 引导 brew uninstall.
///
/// 等价 `frank uninstall` 无参数 (v0.7.3 起默认清 frank 官方), 加 brew 引导提示.
/// 第三方 skill (community/team/private) **不动** — 用户自己装的自己卸.
pub fn run_cleanup() -> Result<()> {
    crate::log::ui::section("frank cleanup — 清 frank 官方装的全部 (第三方 skill 不动)");
    run(Args {
        name: None,
        including_3rd_party: false,
        keep_cache: false,
    })?;
    println!();
    crate::log::ui::info("剩下两步 (Homebrew 自己的事, frank 帮不上):");
    println!("  brew services stop frank          # 停 launchd 服务");
    println!("  brew uninstall frank              # 删 binary (brew 自动 untap)");
    println!();
    crate::log::ui::info("可选: 清 ~/.frank/ (token / state / logs)");
    println!("  rm -rf ~/.frank/                  # 保留则重装直接接管");
    Ok(())
}

/// 执行 uninstall 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "uninstall invoked");

    let mut state = State::load_default()?;

    let targets: Vec<SkillState> = if let Some(name) = args.name.as_ref() {
        // 单卸: 用户显式指定, 任何 visibility 都行 (community 自己装的也能单卸)
        let entry = state.get(name).ok_or_else(|| {
            anyhow::anyhow!("`{name}` is not installed (no record in state.json)")
        })?;
        vec![entry.clone()]
    } else {
        // 无参数 = 清 frank 官方装的全部 (frank-official + frank-recommended)
        let entries: Vec<SkillState> = state
            .iter()
            .filter(|e| is_frank_owned(e, args.including_3rd_party))
            .cloned()
            .collect();
        if entries.is_empty() {
            crate::log::ui::info(
                "没 frank 官方 skill 可卸 (state.json 没 frank-official / frank-recommended)",
            );
            return Ok(());
        }
        let total = state.iter().count();
        let skipped = total - entries.len();
        if args.including_3rd_party {
            crate::log::ui::warn(&format!(
                "--including-3rd-party: 卸 {} 个 (含第三方)",
                entries.len()
            ));
        } else if skipped > 0 {
            crate::log::ui::warn(&format!(
                "卸 {} 个 frank 官方 skill — {} 个第三方 (community/team/private) 保留 (加 --including-3rd-party 也清掉)",
                entries.len(), skipped
            ));
        } else {
            crate::log::ui::warn(&format!("卸 {} 个 frank 官方 skill", entries.len()));
        }
        entries
    };

    let single_target = args.name.is_some();
    let mut ok = 0;
    let mut failed: Vec<(String, String)> = Vec::new();
    for entry in &targets {
        match installer::uninstall_skill_mcp_aware(&entry.name, &entry.source_ref, &entry.platforms)
        {
            Ok(()) => {
                state.remove(&entry.name);
                ok += 1;
                if single_target {
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

    // 默认删 cache (v0.7.3 起改了), --keep-cache 才保留
    if !args.keep_cache && !single_target {
        purge_cache_dir()?;
    }

    if !single_target {
        crate::log::ui::success(&format!("卸了 {ok} 个 skill ({} 个失败)", failed.len()));
    }
    for (name, err) in &failed {
        crate::log::ui::error(&format!("`{name}`: {err}"));
    }
    Ok(())
}

/// 判断 state entry 是不是 frank 自家装的 (frank-official 或 frank-recommended).
///
/// 老 state.json 没 visibility 字段 (反序列化为 None) → 通过 manifest 反查 fallback.
/// 还查不到 (例 v0.7.0 之前装的, manifest 也没 — 比如老 frank-bridge) → 当成 frank 装的清掉.
fn is_frank_owned(entry: &SkillState, including_3rd_party: bool) -> bool {
    if including_3rd_party {
        return true;
    }
    // 优先用 state 里记的 visibility
    if let Some(vis) = entry.visibility {
        return matches!(
            vis,
            Visibility::FrankOfficial | Visibility::FrankRecommended
        );
    }
    // fallback: manifest 找
    let manifests = crate::manifest::parser::discover().unwrap_or_default();
    let skills = crate::manifest::parser::merge(manifests);
    if let Some(skill) = skills.iter().find(|s| s.name == entry.name) {
        return matches!(
            skill.visibility,
            Visibility::FrankOfficial | Visibility::FrankRecommended
        );
    }
    // 都查不到 — 老 state 又不在 manifest, 保守判定为"老的 frank 装的"清掉
    // (用户的 --url 装的 v0.7+ 都有 state.visibility=Community, 不会走到这里)
    true
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
    crate::log::ui::success(&format!(
        "git cache 已清 ({n} 个 repo, {})",
        cache.display()
    ));
    Ok(())
}
