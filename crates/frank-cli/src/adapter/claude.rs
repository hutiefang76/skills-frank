//! Claude Code 平台适配器。
//!
//! 目标目录: `~/.claude/skills/<sanitized-name>/` (symlink → frank cache)
//!
//! P0 仅做 skill 本体 (链接源目录)。slash command 注册 (`~/.claude/commands/*.md`)
//! 延后到 P1 与 `health_check` / `slash_command` schema 字段一并落地。

use std::path::{Path, PathBuf};

use crate::adapter::Adapter;

/// Claude Code 适配器实例。
#[derive(Debug, Clone, Copy)]
pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn platform_dir(&self) -> PathBuf {
        // home dir 缺失意味着 frank 整体跑不起来, panic 反而是清晰信号
        dirs::home_dir()
            .expect("HOME / USERPROFILE not set")
            .join(".claude")
            .join("skills")
    }

    fn install(&self, name: &str, source: &Path) -> anyhow::Result<()> {
        super::link_install(&self.platform_dir(), name, source)
    }

    fn uninstall(&self, name: &str) -> anyhow::Result<()> {
        super::link_uninstall(&self.platform_dir(), name)
    }

    fn verify(&self, name: &str) -> bool {
        super::link_verify(&self.platform_dir(), name)
    }
}
