//! 平台适配器: 把统一 skill 渲染到 Claude / codex / opencode 各自目录格式。
//!
//! # 为什么需要 adapter
//!
//! 三平台 skill yaml 字段不完全兼容, 路径布局也不同 (claude 有 commands/,
//! codex 有 prompts/, opencode 无 slash 概念)。adapter 把差异封装在每个实现里,
//! 上层 installer 只调 trait, 不关心目标平台细节。
//!
//! # 接口
//!
//! 所有平台适配实现 [`Adapter`] trait, 调用方按 skill 的 `target_platforms`
//! 选择性调用。
//!
//! 具体实现 (claude.rs / codex.rs / opencode.rs) 待 P0 day3-4 完成。

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::manifest::schema::Skill;

/// 三平台适配统一接口。
pub trait Adapter {
    /// 平台标识, 例如 `"claude"` / `"codex"` / `"opencode"`。
    fn name(&self) -> &'static str;

    /// 平台 skills 目录, 例如 `~/.claude/skills/`。
    fn platform_dir(&self) -> PathBuf;

    /// 把一个 skill 渲染到当前平台。
    ///
    /// `source` 是 frank cache 中已 clone/checkout 好的源目录,
    /// adapter 负责创建 junction/symlink + 渲染 slash command 等。
    fn install(&self, skill: &Skill, source: &Path) -> Result<()>;

    /// 从当前平台移除。
    fn uninstall(&self, skill: &Skill) -> Result<()>;

    /// 启用 (装上但当前禁用时调用)。
    fn enable(&self, skill: &Skill) -> Result<()>;

    /// 禁用 (保留源, 仅从 adapter 视角断开)。
    fn disable(&self, skill: &Skill) -> Result<()>;

    /// 检查是否已正确安装 (用于 doctor / verify)。
    fn verify(&self, skill: &Skill) -> Result<bool>;
}
