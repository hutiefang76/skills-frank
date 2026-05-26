//! Gemini 平台适配器 (v0.14 新加).
//!
//! 目标目录: `~/.gemini/skills/<sanitized-name>/` (symlink → frank cache)
//!
//! Gemini CLI 当前 (2026-05) skill 支持还在演化, 这里先按通用 symlink 风格 stage,
//! 真要被 Gemini 识别还得看 google/gemini-cli 后续发版. 即使暂时不识别也无害 —
//! 只是占个位置不影响其他 platform.

use std::path::{Path, PathBuf};

use crate::adapter::Adapter;

/// Gemini 适配器实例.
#[derive(Debug, Clone, Copy)]
pub struct GeminiAdapter;

impl Adapter for GeminiAdapter {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn platform_dir(&self) -> PathBuf {
        dirs::home_dir()
            .expect("HOME / USERPROFILE not set")
            .join(".gemini")
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
