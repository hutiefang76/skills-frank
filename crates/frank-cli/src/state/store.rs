//! `state.json` 持久化: 已安装 skills 的本机状态记录。
//!
//! # 文件位置
//!
//! 默认 `~/.frank/state.json`; 测试期可传任意路径。
//!
//! # 原子写
//!
//! 任何 `save()` 都先写 `state.json.tmp` 再 `rename` 到目标 — 即使写入中途崩溃,
//! 原文件也不会半截。注意: 跨进程并发仍可能丢更新 (P1 加 `fs2` 文件锁解决)。
//! 单进程 CLI 场景下原子 rename 已经够用。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::manifest::schema::Platform;

/// `state.json` 顶层结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateData {
    /// schema 版本; 当前 1。
    pub schema_version: u32,

    /// 当前 active profile (例如 `personal` / `company`)。
    #[serde(default = "default_profile")]
    pub profile: String,

    /// 已安装的 skill 记录, key 为 skill name。
    /// 用 `BTreeMap` 保证序列化顺序稳定 (diff 友好)。
    #[serde(default)]
    pub skills: BTreeMap<String, SkillState>,
}

impl Default for StateData {
    fn default() -> Self {
        Self {
            schema_version: 1,
            profile: default_profile(),
            skills: BTreeMap::new(),
        }
    }
}

/// 单个 skill 在本机的安装状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillState {
    /// skill 名称 (与 manifest `name` 一致, 可含 `kdwl:` 前缀)。
    pub name: String,

    /// 已安装的 git 引用 (commit SHA, 40 字符全长)。
    pub source_ref: String,

    /// 实际作为 adapter 链接源的本地路径 (含 manifest subpath 拼接)。
    pub source_path: PathBuf,

    /// 已渲染到的目标平台。
    pub platforms: Vec<Platform>,

    /// 安装时间 (UTC)。
    pub installed_at: DateTime<Utc>,

    /// 当前是否启用 (true = adapter 链接存在; false = 仅保留 cache)。
    pub enabled: bool,

    /// 装时记的 visibility (v0.7.3 起新增, 老 state 无字段时反序列化为 None).
    /// 用于 `frank uninstall` 区分 frank 官方装的 vs 用户自己 --url 装的:
    /// 无参数 uninstall 只清 frank-official + frank-recommended (frank 自己负责的),
    /// community/team/private 不动 (用户自己装的, 用户自己卸).
    #[serde(default)]
    pub visibility: Option<crate::manifest::schema::Visibility>,
}

/// State 持久化句柄: 持有数据 + 文件路径, 提供 CRUD + save。
#[derive(Debug)]
pub struct State {
    path: PathBuf,
    data: StateData,
}

impl State {
    /// 从指定路径加载; 文件不存在或为空时返回空 `default` state。
    ///
    /// 空文件特殊处理: 用户手动 `touch state.json` 不应让 frank 崩溃。
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("read state file {}", path.display()))?;
            if content.trim().is_empty() {
                tracing::debug!(path = %path.display(), "state file empty, returning default");
                StateData::default()
            } else {
                serde_json::from_str(&content)
                    .with_context(|| format!("parse state file {}", path.display()))?
            }
        } else {
            tracing::debug!(path = %path.display(), "state file not found, returning default");
            StateData::default()
        };
        Ok(Self { path, data })
    }

    /// 用默认路径 (`~/.frank/state.json`) 加载。
    pub fn load_default() -> Result<Self> {
        Self::load(default_path()?)
    }

    /// 原子持久化到磁盘。父目录不存在时自动创建。
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&self.data).context("serialize state.json")?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("write tmp {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        tracing::debug!(path = %self.path.display(), "state saved");
        Ok(())
    }

    /// 按 name 取 skill 状态。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&SkillState> {
        self.data.skills.get(name)
    }

    /// 按 name 取可变引用 (用于 enable/disable 切换 `enabled` 字段)。
    pub fn get_mut(&mut self, name: &str) -> Option<&mut SkillState> {
        self.data.skills.get_mut(name)
    }

    /// 插入或覆盖一条记录。
    pub fn put(&mut self, skill: SkillState) {
        self.data.skills.insert(skill.name.clone(), skill);
    }

    /// 移除一条记录, 返回被移除值。
    pub fn remove(&mut self, name: &str) -> Option<SkillState> {
        self.data.skills.remove(name)
    }

    /// 已安装 skill 数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.skills.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.skills.is_empty()
    }

    /// 遍历所有 skill state。
    pub fn iter(&self) -> impl Iterator<Item = &SkillState> {
        self.data.skills.values()
    }

    /// 文件路径 (调试/日志用)。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 默认 state.json 路径: `~/.frank/state.json`。
pub fn default_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate user home dir")?
        .join(".frank")
        .join("state.json"))
}

fn default_profile() -> String {
    "personal".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_skill(name: &str) -> SkillState {
        SkillState {
            name: name.to_string(),
            source_ref: "abc1234".to_string(),
            source_path: PathBuf::from("/tmp/cache/x"),
            platforms: vec![Platform::Claude],
            installed_at: Utc.with_ymd_and_hms(2026, 5, 21, 10, 0, 0).unwrap(),
            enabled: true,
            visibility: None,
        }
    }

    #[test]
    fn load_missing_file_returns_default() {
        let tf = tempfile::NamedTempFile::new().unwrap();
        let path = tf.path().to_path_buf();
        drop(tf); // 文件被删
        let state = State::load(path).unwrap();
        assert!(state.is_empty());
        assert_eq!(state.data.schema_version, 1);
    }

    #[test]
    fn put_get_remove_roundtrip() {
        let tf = tempfile::NamedTempFile::new().unwrap();
        let mut state = State::load(tf.path().to_path_buf()).unwrap();
        state.put(sample_skill("doris-ops"));
        assert!(state.get("doris-ops").is_some());
        assert_eq!(state.len(), 1);

        let removed = state.remove("doris-ops").unwrap();
        assert_eq!(removed.name, "doris-ops");
        assert!(state.is_empty());
    }

    #[test]
    fn save_load_roundtrip_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut state = State::load(path.clone()).unwrap();
        state.put(sample_skill("a"));
        state.put(sample_skill("b"));
        state.save().unwrap();
        assert!(path.exists());

        let reloaded = State::load(path).unwrap();
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded.get("a").unwrap().source_ref, "abc1234");
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("a").join("state.json");
        let mut state = State::load(path.clone()).unwrap();
        state.put(sample_skill("x"));
        state.save().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn get_mut_allows_toggling_enabled() {
        let tf = tempfile::NamedTempFile::new().unwrap();
        let mut state = State::load(tf.path().to_path_buf()).unwrap();
        state.put(sample_skill("d"));
        state.get_mut("d").unwrap().enabled = false;
        assert!(!state.get("d").unwrap().enabled);
    }
}
