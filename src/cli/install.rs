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

use crate::installer::install as installer;
use crate::manifest::{parser as mparser, resolver::Registry};
use crate::state::{SkillState, State};

/// `frank install` 参数。
#[derive(Parser, Debug)]
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

    // 3. 跑安装
    let started = Instant::now();
    crate::log::ui::info(&format!("Installing `{name}`..."));
    let outcome = installer::install_skill(skill)?;
    let elapsed = started.elapsed();

    // 4. 写 state.json
    let mut state = State::load_default()?;
    state.put(SkillState {
        name: skill.name.clone(),
        source_ref: outcome.commit_sha.clone(),
        source_path: outcome.source_path.clone(),
        platforms: outcome.platforms.clone(),
        installed_at: Utc::now(),
        enabled: true,
    });
    state.save()?;

    // 5. 用户面输出 (短 sha 7 位即可识别)
    let short_sha: String = outcome.commit_sha.chars().take(7).collect();
    crate::log::ui::success(&format!(
        "`{name}` installed ({} platform{}, {:.1}s) — sha {short_sha}",
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
