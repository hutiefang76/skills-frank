//! codex 平台适配器。
//!
//! 目标目录: `~/.codex/skills/<sanitized-name>/` (symlink → frank cache)
//!
//! P0 仅做 skill 本体。`~/.codex/prompts/*.md` slash command 渲染延后到 P1。

use std::path::{Path, PathBuf};

use crate::adapter::Adapter;

/// codex 适配器实例。
#[derive(Debug, Clone, Copy)]
pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn platform_dir(&self) -> PathBuf {
        dirs::home_dir()
            .expect("HOME / USERPROFILE not set")
            .join(".codex")
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
