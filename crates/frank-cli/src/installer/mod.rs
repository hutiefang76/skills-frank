//! 安装实现: git fetch → adapter 分发 → 失败回滚。
//!
//! # 子模块
//!
//! - [`git`] — 用 `git2` 把仓库克隆/更新到 `~/.frank/cache/`
//! - [`link`] — 跨平台 symlink 工具 (unix symlink / windows symlink_dir)
//! - [`install`] — 流程编排, 调用 git + adapter, 返回安装摘要
//!
//! # 推后实现
//!
//! - `credentials`: keychain 读凭据并注入 (`kdwl` 私有 skill 时才需要)
//! - sparse-checkout: 仅当多 skill 单仓 (`subpath`) 大仓时优化, P0 doris-ops 不需要

pub mod git;
pub mod install;
pub mod link;
