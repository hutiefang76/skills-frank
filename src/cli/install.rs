//! `frank install` 子命令: 安装一个 skill / MCP。
//!
//! # 流程 (详见 docs/DESIGN.md §4.2)
//!
//! 1. 解析 manifest, 定位 skill 元数据
//! 2. health-check 前置 (依赖/网络/凭据/device-allowlist)
//! 3. state manager 创建 snapshot (失败可回滚)
//! 4. installer 拉取源码 (git fetch + subpath checkout)
//! 5. adapter 分发到三平台 (junction/symlink + slash command)
//! 6. state.json 更新
//! 7. sync-client 上报云端 (P2 才启用)
//!
//! P0 day1 状态: 仅骨架与参数定义, 实际流程待 day3-4 实现。

use anyhow::Result;
use clap::Parser;

/// `frank install` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 要安装的 skill 名称, 例如 `doris-ops` 或 `kdwl:vehicle-events`。
    pub name: Option<String>,

    /// 安装某个 profile 下的全部 skills。与 `name` 互斥。
    #[arg(long)]
    pub all: bool,

    /// 指定 profile (例如 `personal` / `company` / 自定义)。
    /// 缺省: state.json 中的 active_profile, 或 `personal`。
    #[arg(long)]
    pub profile: Option<String>,

    /// 跳过 health-check (不推荐, 仅用于调试)。
    #[arg(long)]
    pub skip_health_check: bool,
}

/// 执行 install 命令。
pub fn run(args: Args) -> Result<()> {
    tracing::info!(?args, "install command invoked");

    // P0 day1 占位: 实际流程待 manifest / installer / adapter 模块完成。
    crate::log::ui::info(&format!(
        "(scaffold) would install: name={:?}, all={}, profile={:?}",
        args.name, args.all, args.profile
    ));
    crate::log::ui::warn("install flow not yet wired up — see ADR roadmap (P0 day3-4)");

    Ok(())
}
