//! `frank install` 子命令: 安装一个 skill / MCP。
//!
//! # 流程 (详见 docs/DESIGN.md §4.2)
//!
//! 1. 解析 manifest, 定位 skill 元数据
//! 2. (推后 P1) health-check 前置 (依赖/网络/凭据/device-allowlist)
//! 3. (推后 P1) state snapshot
//! 4. [`crate::installer::install`] 拉源 + 分发到三平台
//! 5. 写 [`crate::state::State`]
//! 6. (推后 P2) sync-client 上报云端

use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use clap::Parser;

use crate::adapter;
use crate::installer::install as installer;
use crate::manifest::{parser as mparser, resolver::Registry};
use crate::scanner;
use crate::state::{SkillState, State};

/// `frank install` 参数。
#[derive(Parser, Debug)]
#[allow(clippy::struct_excessive_bools)] // CLI flag 集合, 拆 enum 反而难用
pub struct Args {
    /// 要安装的 skill 名称, 例如 `doris-ops` 或 `kdwl:vehicle-events`。
    pub name: Option<String>,

    /// 安装某个 profile 下的全部 skills。与 `name` 互斥。(P0 day5 待实现)
    #[arg(long)]
    pub all: bool,

    /// 指定 profile (例如 `personal` / `company`)。
    #[arg(long)]
    pub profile: Option<String>,

    /// 跳过 health-check (P0 health-check 尚未接入, 此 flag 当前是占位)。
    #[arg(long)]
    pub skip_health_check: bool,

    /// 即便已安装也强行覆盖 (会重新建链, 替换 state 记录)。
    #[arg(long)]
    pub force: bool,

    /// 已安装时拉取新 commit 但保留原始 `installed_at`。
    #[arg(long)]
    pub upgrade: bool,
}

/// 执行 install 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "install invoked");

    if args.all {
        crate::log::ui::warn("`--all` not yet wired (P0 day5); pass a single skill name for now");
        return Ok(());
    }
    if args.skip_health_check {
        crate::log::ui::warn("`--skip-health-check` is a no-op (health-check not yet executed)");
    }

    let name = args
        .name
        .ok_or_else(|| anyhow!("provide a skill name (e.g. `frank install doris-ops`)"))?;

    // 1. 加载 manifest
    let manifests = mparser::discover()?;
    if manifests.is_empty() {
        bail!("no manifest found; expected manifest/public.yaml or ~/.frank/manifests/*.yaml");
    }
    let registry = Registry::new(mparser::merge(manifests));

    // 2. 找 skill
    let skill = registry
        .find(&name)
        .ok_or_else(|| anyhow!("skill `{name}` not found in any manifest"))?;

    // 3. 容错: state 已存在 / 平台上已是 external 撞名 → 友好提示
    let mut state = State::load_default()?;
    let preexisting_installed_at = preflight_state_check(&state, &name, args.force, args.upgrade)?;
    preflight_external_check(&name, args.force)?;

    // 4. 跑安装
    let started = Instant::now();
    crate::log::ui::info(&format!("Installing `{name}`..."));
    let outcome = installer::install_skill(skill)?;
    let elapsed = started.elapsed();

    // 5. 写 state.json (upgrade 时保留 installed_at)
    let installed_at = preexisting_installed_at.unwrap_or_else(Utc::now);
    state.put(SkillState {
        name: skill.name.clone(),
        source_ref: outcome.commit_sha.clone(),
        source_path: outcome.source_path.clone(),
        platforms: outcome.platforms.clone(),
        installed_at,
        enabled: true,
    });
    state.save()?;

    // 6. 用户面输出 (短 sha 7 位即可识别)
    let short_sha: String = outcome.commit_sha.chars().take(7).collect();
    let verb = if preexisting_installed_at.is_some() {
        "reinstalled"
    } else {
        "installed"
    };
    crate::log::ui::success(&format!(
        "`{name}` {verb} ({} platform{}, {:.1}s) — sha {short_sha}",
        outcome.platforms.len(),
        if outcome.platforms.len() == 1 {
            ""
        } else {
            "s"
        },
        elapsed.as_secs_f64(),
    ));

    Ok(())
}

/// 检查 state 已有该 skill 的处理: 没有 → Ok(None); 有 → 看 force/upgrade 决定。
fn preflight_state_check(
    state: &State,
    name: &str,
    force: bool,
    upgrade: bool,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(existing) = state.get(name) else {
        return Ok(None);
    };
    let short_sha: String = existing.source_ref.chars().take(7).collect();
    let when = existing.installed_at.format("%Y-%m-%d %H:%M UTC");
    if upgrade {
        crate::log::ui::info(&format!(
            "upgrading `{name}` (current sha {short_sha} from {when}); installed_at preserved"
        ));
        return Ok(Some(existing.installed_at));
    }
    if force {
        crate::log::ui::warn(&format!(
            "`{name}` already installed (sha {short_sha} from {when}); --force overwriting"
        ));
        return Ok(None);
    }
    bail!(
        "`{name}` already installed (sha {short_sha} from {when}); use `--force` to reinstall or `--upgrade` to refresh"
    );
}

/// 检查平台目录是否存在同名 external 条目 (用户手工装的撞名), 友好提示走 import / force。
///
/// P2-1 fix (codex review): 早期版本用空 state 扫描, 导致 `frank install <name> --upgrade`
/// 把自己的 managed 链接误判为 external 然后 bail. 现在用真 state, 排除"已 managed 的 link
/// 指向预期 source_path"的健康情况.
fn preflight_external_check(name: &str, force: bool) -> Result<()> {
    let real_state = State::load_default()?;
    let mut platforms: Vec<crate::manifest::schema::Platform> = Vec::new();
    for &p in scanner::ALL_PLATFORMS {
        let scanned = scanner::scan_platform(p, &real_state)?;
        if scanned
            .iter()
            .any(|s| s.name == name && matches!(s.status, scanner::SkillStatus::External))
        {
            platforms.push(p);
        }
    }
    if platforms.is_empty() {
        return Ok(());
    }
    if force {
        // 把已有 external 条目清掉, 让 link_install 不再撞
        for &p in &platforms {
            let _ = adapter::for_platform(p).uninstall(name);
            // remove_link 只删 symlink; 若是真目录, 我们不动它 (link_install 会报错)
        }
        crate::log::ui::warn(&format!(
            "`{name}` already exists at {} platform(s); --force will overwrite (existing symlinks removed)",
            platforms.len()
        ));
        return Ok(());
    }
    bail!(
        "`{name}` already exists in {} platform skills dir as an external entry; run `frank import {name}` to manage it (or pass `--force` to overwrite)",
        platforms.len()
    );
}
