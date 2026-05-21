//! 平台适配器: 把统一 skill 渲染到 Claude / codex / opencode 各自目录格式。
//!
//! # 为什么需要 adapter
//!
//! 三平台 skill 目录布局不完全一致 (claude 有 `commands/`, codex 有 `prompts/`, opencode
//! 无 slash 概念)。adapter 把差异封装在每个实现里, 上层 [`crate::installer`] 只调 trait,
//! 不关心目标平台细节。
//!
//! # trait 范围 (P0)
//!
//! P0 只暴露 `install` / `uninstall` / `verify` 三个核心动作。
//! enable / disable 是 CLI 层的"高级语义" (需要从 state 里查 source_path), 不在
//! adapter trait 上, 由 [`crate::cli`] 编排 state + adapter。

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::installer::link;
use crate::manifest::schema::Platform;

pub mod claude;
pub mod codex;
pub mod opencode;

/// 三平台适配统一接口。
///
/// P0 trait 故意保持极简: 只暴露 "建/删/查链接" 三个原子操作, 用 `(name, source)`
/// 两个简单参数即可调用。enable 与 install 共用 `install`; disable 与 uninstall
/// 共用 `uninstall` — 区别只在 CLI 层是否同时更新 state。
///
/// 未来 P1 加 slash_command 渲染时, 单独加一个 `render_slash_command` 方法或扩展
/// trait, 不污染本接口。
pub trait Adapter {
    /// 平台标识, 例如 `"claude"` / `"codex"` / `"opencode"`。
    fn name(&self) -> &'static str;

    /// 平台 skills 目录, 例如 `~/.claude/skills/`。
    fn platform_dir(&self) -> PathBuf;

    /// 在平台目录里建一条链接, 指向 `source` (frank cache + subpath)。
    ///
    /// 若目标已是 frank 链接 → 覆盖; 若是真实目录/文件 → 报错让用户手动处理。
    fn install(&self, name: &str, source: &Path) -> Result<()>;

    /// 从当前平台移除链接 (不动 cache)。幂等。
    fn uninstall(&self, name: &str) -> Result<()>;

    /// 检查链接是否已建立 (用于 doctor / `list --installed`)。
    fn verify(&self, name: &str) -> bool;
}

/// 按 [`Platform`] 取对应 adapter 实例。
#[must_use]
pub fn for_platform(p: Platform) -> Box<dyn Adapter> {
    match p {
        Platform::Claude => Box::new(claude::ClaudeAdapter),
        Platform::Codex => Box::new(codex::CodexAdapter),
        Platform::Opencode => Box::new(opencode::OpencodeAdapter),
    }
}

/// 把 skill name 转换成各平台目录名 (替换不友好字符)。
///
/// 三平台 `skills/` 目录都不爱 `:`, 替换为 `-`。其他字符直接透传。
/// 例: `kdwl:vehicle-events` → `kdwl-vehicle-events`。
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    name.replace(':', "-")
}

// ---- 三平台共享的 link 操作 ----
//
// 三家 adapter 唯一差别就是 `platform_dir`, 其他全是 "向目标目录建/删/验链接"。
// 提取私有 helper 减少 3x 重复, 同时把 "目录里若有真实文件就报错而不是覆盖" 的关键安全
// 检查放在一处, 避免某天补 adapter 时漏掉。

pub(crate) fn link_install(platform_dir: &Path, name: &str, source: &Path) -> Result<()> {
    let dest = platform_dir.join(sanitize_name(name));
    if dest.exists() || link::is_link(&dest) {
        if link::is_link(&dest) {
            // 已是 frank 链接, 覆盖即可
            link::remove_link(&dest)?;
        } else {
            // 真实文件/目录 — 拒绝覆盖
            bail!(
                "{} already exists and is not a frank symlink; please remove it manually",
                dest.display()
            );
        }
    }
    link::make_link(source, &dest)
}

pub(crate) fn link_uninstall(platform_dir: &Path, name: &str) -> Result<()> {
    let dest = platform_dir.join(sanitize_name(name));
    link::remove_link(&dest)
}

#[must_use]
pub(crate) fn link_verify(platform_dir: &Path, name: &str) -> bool {
    let dest = platform_dir.join(sanitize_name(name));
    link::is_link(&dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_colons() {
        assert_eq!(sanitize_name("kdwl:vehicle-events"), "kdwl-vehicle-events");
        assert_eq!(sanitize_name("doris-ops"), "doris-ops");
    }

    #[test]
    fn for_platform_returns_correct_name() {
        assert_eq!(for_platform(Platform::Claude).name(), "claude");
        assert_eq!(for_platform(Platform::Codex).name(), "codex");
        assert_eq!(for_platform(Platform::Opencode).name(), "opencode");
    }
}
