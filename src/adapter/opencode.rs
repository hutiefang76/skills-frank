//! opencode 平台适配器。
//!
//! 目标目录: `~/.opencode/skills/<sanitized-name>/` (symlink → frank cache)
//!
//! opencode 不支持 slash command, P0 与 P1 都只装 skill 本体。

use std::path::{Path, PathBuf};

use crate::adapter::Adapter;

/// opencode 适配器实例。
#[derive(Debug, Clone, Copy)]
pub struct OpencodeAdapter;

impl Adapter for OpencodeAdapter {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn platform_dir(&self) -> PathBuf {
        dirs::home_dir()
            .expect("HOME / USERPROFILE not set")
            .join(".opencode")
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
