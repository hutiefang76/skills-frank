//! 本地 skills 目录扫描器。
//!
//! 列出三平台 (claude / codex / opencode) `~/.<plat>/skills/` 下的所有条目, 与
//! [`crate::state`] 对照, 给每条标 `managed-enabled` / `managed-disabled` /
//! `managed-missing` / `external` / `duplicate`。
//!
//! 调用方:
//! - [`crate::cli::scan`] — `frank scan` 打表展示
//! - [`crate::cli::import`] — 把 external 收编进 state
//! - [`crate::cli::dedupe`] — 找同名异源的重复并清理

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapter;
use crate::installer::link;
use crate::manifest::schema::Platform;
use crate::state::State;

/// 三平台清单 (固定顺序: Claude → Codex → Opencode), 给上层批量遍历用。
pub const ALL_PLATFORMS: &[Platform] = &[Platform::Claude, Platform::Codex, Platform::Opencode];

/// 单条 skill 在某平台上的健康/归属状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillStatus {
    /// state 有记录, 当前 `enabled=true`, 平台目录 symlink 健康且 target 命中 state.source_path。
    ManagedEnabled,
    /// state 有记录但 `enabled=false` (用户 `frank disable` 过)。
    ManagedDisabled,
    /// state 里 enabled=true, 但平台目录里没找到对应链接 (link 断了 / 被人手删)。
    ManagedMissing,
    /// 平台目录里有条目, state 里没记录 — 用户手工装的, 可被 `frank import` 收编。
    External,
}

/// 一条被扫到的 skill 条目 (单平台一条)。
#[derive(Debug, Clone)]
pub struct ScannedSkill {
    /// skill 目录名 (即平台目录下的文件名)。
    pub name: String,
    /// 所在平台。
    pub platform: Platform,
    /// 实际盘上路径 (`~/.<plat>/skills/<name>`)。
    pub disk_path: PathBuf,
    /// `disk_path` 是否是 symlink。
    pub is_link: bool,
    /// 若 `is_link == true`, readlink 的目标 (绝对或相对原样保留)。
    pub link_target: Option<PathBuf>,
    /// 归属/健康状态。
    pub status: SkillStatus,
}

impl ScannedSkill {
    /// 给 UI 显示的 source 列: managed 显示 state.source_path; external 显示 disk_path。
    #[must_use]
    pub fn display_source(&self, state: &State) -> PathBuf {
        if let Some(s) = state.get(&self.name) {
            s.source_path.clone()
        } else {
            self.disk_path.clone()
        }
    }
}

/// 扫描全部三平台目录, 与 `state` 对照返回扁平清单。
pub fn scan_all(state: &State) -> Result<Vec<ScannedSkill>> {
    let mut out = Vec::new();
    for &p in ALL_PLATFORMS {
        out.extend(scan_platform(p, state)?);
    }
    Ok(out)
}

/// 扫一个平台的 `~/.<plat>/skills/` 目录。目录不存在 → 返回空 vec, 不算错。
pub fn scan_platform(p: Platform, state: &State) -> Result<Vec<ScannedSkill>> {
    let adp = adapter::for_platform(p);
    let dir = adp.platform_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("iterate {}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // 跳隐藏文件 (.DS_Store 之类)
        if name.starts_with('.') {
            continue;
        }
        let is_link = link::is_link(&path);
        let link_target = if is_link {
            fs::read_link(&path).ok()
        } else {
            None
        };
        let status = classify(&name, is_link, link_target.as_deref(), state);
        out.push(ScannedSkill {
            name,
            platform: p,
            disk_path: path,
            is_link,
            link_target,
            status,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 根据 state 记录与 link 状态判断单条 skill 的归属。
///
/// `name` 是 sanitize 后的目录名 (例如 `kdwl-vehicle-events`), state 里 key 是原 manifest
/// 名 (`kdwl:vehicle-events`); 这里先查直接匹配, 再 fallback 遍历 state 找
/// `sanitize(state.name) == name` (P2-2 fix: codex review 指出原版只查直接匹配, 导致含冒号
/// 的 skill 健康 link 被误判为 external).
fn classify(name: &str, is_link: bool, link_target: Option<&Path>, state: &State) -> SkillStatus {
    let entry = state.get(name).or_else(|| {
        state
            .iter()
            .find(|s| crate::adapter::sanitize_name(&s.name) == name)
    });
    match entry {
        None => SkillStatus::External,
        Some(s) if !s.enabled => SkillStatus::ManagedDisabled,
        Some(s) => {
            // enabled=true: 看 link 是否健康并指向 state.source_path
            if is_link && link_target.is_some_and(|t| t == s.source_path) {
                SkillStatus::ManagedEnabled
            } else {
                SkillStatus::ManagedMissing
            }
        }
    }
}

/// 找出"同名 skill 在多平台 link_target 不一致"的重复组。
///
/// 返回 `name -> Vec<&ScannedSkill>`。只保留满足下列任一条件的组:
/// - 出现在多个平台, 且各平台 `link_target` (或 disk_path) 不完全一致
/// - 同一平台出现多次 (不可能, fs 同名冲突, 兜底)
#[must_use]
pub fn find_duplicates<'a>(scanned: &'a [ScannedSkill]) -> BTreeMap<String, Vec<&'a ScannedSkill>> {
    let mut by_name: BTreeMap<String, Vec<&'a ScannedSkill>> = BTreeMap::new();
    for s in scanned {
        by_name.entry(s.name.clone()).or_default().push(s);
    }
    by_name.retain(|_, v| {
        if v.len() <= 1 {
            return false;
        }
        // 一致性比较: link_target 优先, 没 link 用 disk_path
        let key = |s: &&ScannedSkill| s.link_target.clone().unwrap_or_else(|| s.disk_path.clone());
        let first = key(&v[0]);
        v.iter().any(|s| key(s) != first)
    });
    by_name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SkillState;
    use chrono::Utc;

    fn mk(name: &str, platform: Platform, target: Option<&str>) -> ScannedSkill {
        ScannedSkill {
            name: name.into(),
            platform,
            disk_path: PathBuf::from(format!("/tmp/{platform:?}/skills/{name}")),
            is_link: target.is_some(),
            link_target: target.map(PathBuf::from),
            status: SkillStatus::External,
        }
    }

    fn empty_state() -> State {
        let tf = tempfile::NamedTempFile::new().unwrap();
        State::load(tf.path().to_path_buf()).unwrap()
    }

    fn state_with(name: &str, source_path: &str, enabled: bool) -> State {
        let mut s = empty_state();
        s.put(SkillState {
            name: name.into(),
            source_ref: "ref".into(),
            source_path: PathBuf::from(source_path),
            platforms: vec![Platform::Claude],
            installed_at: Utc::now(),
            enabled,
            visibility: None,
        });
        s
    }

    #[test]
    fn scan_platform_never_panics_on_real_dirs() {
        // 不能跨 home dir 隔离, 这里只验证不 panic — 真集成走 cargo run
        let state = empty_state();
        for &p in ALL_PLATFORMS {
            let _ = scan_platform(p, &state);
        }
    }

    #[test]
    fn classify_covers_all_four_states() {
        let none = empty_state();
        assert_eq!(classify("foo", false, None, &none), SkillStatus::External);

        let enabled = state_with("x", "/tmp/cache/a", true);
        let src = PathBuf::from("/tmp/cache/a");
        assert_eq!(
            classify("x", true, Some(&src), &enabled),
            SkillStatus::ManagedEnabled
        );
        // drift: link 指向别处
        let other = PathBuf::from("/elsewhere");
        assert_eq!(
            classify("x", true, Some(&other), &enabled),
            SkillStatus::ManagedMissing
        );
        // 无 link
        assert_eq!(
            classify("x", false, None, &enabled),
            SkillStatus::ManagedMissing
        );

        let disabled = state_with("x", "/tmp/cache/a", false);
        assert_eq!(
            classify("x", false, None, &disabled),
            SkillStatus::ManagedDisabled
        );
    }

    #[test]
    fn find_duplicates_ignores_consistent_targets() {
        let v = vec![
            mk("foo", Platform::Claude, Some("/tmp/same")),
            mk("foo", Platform::Codex, Some("/tmp/same")),
        ];
        assert!(find_duplicates(&v).is_empty());
    }

    #[test]
    fn find_duplicates_flags_divergent_targets() {
        let v = vec![
            mk("foo", Platform::Claude, Some("/tmp/a")),
            mk("foo", Platform::Codex, Some("/tmp/b")),
        ];
        let dups = find_duplicates(&v);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups["foo"].len(), 2);
    }

    #[test]
    fn find_duplicates_singleton_is_not_dup() {
        let v = vec![mk("foo", Platform::Claude, Some("/tmp/a"))];
        assert!(find_duplicates(&v).is_empty());
    }
}
