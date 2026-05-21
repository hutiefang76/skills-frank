//! 跨平台符号链接 helper。
//!
//! # 平台差异
//!
//! - **Unix** (macOS / Linux): `std::os::unix::fs::symlink` — 标准 symlink, 不需特权
//! - **Windows**: `std::os::windows::fs::symlink_dir` — 创建目录 symlink,
//!   **需要开发者模式或管理员**。失败时 anyhow 错误信息会显式提示用户。
//!
//! 当前仅 symlink, Windows 不走 junction 兜底。junction 兜底等踩到 win 用户无 dev-mode
//! 的真实场景再加 (P1 可能升级到 mklink /J)。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// 创建一个指向 `target` 的链接 `link`。
///
/// 自动创建 `link` 的父目录 (`mkdir -p`)。
///
/// # 平台行为
/// - Unix: `symlink(target, link)`
/// - Windows: `symlink_dir(target, link)` (`target` 必须存在, 否则 Windows 拒绝)
pub fn make_link(target: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(target, link)
            .with_context(|| format!("create symlink {} -> {}", link.display(), target.display()))
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir;
        symlink_dir(target, link).with_context(|| {
            format!(
                "create directory symlink {} -> {} (Windows requires Developer Mode or administrator privileges)",
                link.display(),
                target.display()
            )
        })
    }
}

/// 判断路径是否是 symlink (`symlink_metadata` 不跟随)。
///
/// 路径不存在或读 metadata 失败一律视为 `false`。
#[must_use]
pub fn is_link(link: &Path) -> bool {
    fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink())
}

/// 移除一个由 [`make_link`] 创建的链接。幂等: 链接不存在时返回 Ok。
///
/// **不会** 移除真实目录 — 调用前用 [`is_link`] 校验。
pub fn remove_link(link: &Path) -> Result<()> {
    if !is_link(link) {
        return Ok(());
    }

    #[cfg(unix)]
    {
        fs::remove_file(link).with_context(|| format!("remove symlink {}", link.display()))
    }

    #[cfg(windows)]
    {
        fs::remove_dir(link).with_context(|| format!("remove directory symlink {}", link.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn make_and_remove_link_to_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-dir");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("a-link");

        make_link(&target, &link).unwrap();
        assert!(is_link(&link));

        remove_link(&link).unwrap();
        assert!(!is_link(&link));
        // 真实目录必须仍在
        assert!(target.exists());
    }

    #[test]
    fn make_link_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("nested").join("deep").join("link");

        make_link(&target, &link).unwrap();
        assert!(is_link(&link));
    }

    #[test]
    fn remove_link_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("missing-link");
        // 不存在直接调 remove_link, 应该 Ok
        remove_link(&link).unwrap();
    }

    #[test]
    fn is_link_false_for_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        File::create(&file).unwrap();
        assert!(!is_link(&file));
    }
}
