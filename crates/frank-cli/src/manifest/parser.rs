//! Manifest 加载与合并。
//!
//! # 加载顺序 (后加载覆盖前加载)
//!
//! 1. 项目内置: `<repo>/manifest/public.yaml` (公开 skills)
//! 2. 用户私有: `~/.frank/manifests/*.yaml` (含公司 skills)
//! 3. 环境变量: `FRANK_EXTRA_MANIFEST` 指向额外文件
//!
//! 详见 docs/DESIGN.md §6.2.2。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::manifest::schema::{Manifest, Skill};

/// 从单个 YAML 文件加载 manifest。
///
/// # 错误
/// - 文件读取失败 (权限/不存在)
/// - YAML 语法错误
/// - 字段类型不匹配 schema
pub fn load_file(path: &Path) -> Result<Manifest> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read manifest file {}", path.display()))?;
    let m: Manifest = serde_yml::from_str(&content)
        .with_context(|| format!("parse manifest {}", path.display()))?;
    tracing::debug!(path = %path.display(), skills = m.skills.len(), "manifest loaded");
    Ok(m)
}

/// 发现并按优先级加载全部 manifest 文件。
///
/// 返回顺序保证: 先 public → 再 user → 最后 env extra。
/// 调用方可直接传给 [`merge`] 做后覆盖前。
pub fn discover() -> Result<Vec<Manifest>> {
    let mut manifests = Vec::new();

    // 1. 项目内置 public manifest
    if let Some(p) = built_in_public_path() {
        if p.exists() {
            manifests.push(load_file(&p)?);
        }
    }

    // 2. 用户私有 manifests (~/.frank/manifests/*.yaml)
    if let Some(dir) = user_manifest_dir() {
        if dir.exists() {
            for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                    manifests.push(load_file(&path)?);
                }
            }
        }
    }

    // 3. 环境变量额外文件
    if let Ok(extra) = std::env::var("FRANK_EXTRA_MANIFEST") {
        let p = PathBuf::from(extra);
        if p.exists() {
            manifests.push(load_file(&p)?);
        }
    }

    tracing::debug!(count = manifests.len(), "manifest discover completed");
    Ok(manifests)
}

/// 合并多份 manifest 为单一 skill 列表。
///
/// 同名 skill 后覆盖前 (用户私有 > 内置 public)。结果按 name 字典序排序,
/// 方便 `frank list` 输出稳定。
#[must_use]
pub fn merge(manifests: Vec<Manifest>) -> Vec<Skill> {
    let mut by_name: HashMap<String, Skill> = HashMap::new();
    for m in manifests {
        for s in m.skills {
            by_name.insert(s.name.clone(), s);
        }
    }
    let mut v: Vec<Skill> = by_name.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// 内置 public manifest 路径。
///
/// 开发模式 (`cargo run`): `<repo>/manifest/public.yaml`
/// 安装后: `<exe-dir>/../manifest/public.yaml`
fn built_in_public_path() -> Option<PathBuf> {
    // 开发时 (cargo run 上下文)
    let cargo_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("manifest")
        .join("public.yaml");
    if cargo_path.exists() {
        return Some(cargo_path);
    }
    // 安装后
    std::env::current_exe().ok().and_then(|p| {
        p.parent()
            .map(|d| d.join("..").join("manifest").join("public.yaml"))
    })
}

/// 用户私有 manifest 目录: `~/.frank/manifests/`。
fn user_manifest_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".frank").join("manifests"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn merge_overrides_by_name() {
        // 同名 skill, 第二个版本 (private profile) 应覆盖
        let yaml1 = r"
schema_version: 1
skills:
  - name: foo
    source: { type: git, url: 'https://example.com/v1.git' }
    visibility: public
";
        let yaml2 = r"
schema_version: 1
skills:
  - name: foo
    source: { type: git, url: 'https://example.com/v2.git' }
    visibility: private
";
        let m1: Manifest = serde_yml::from_str(yaml1).unwrap();
        let m2: Manifest = serde_yml::from_str(yaml2).unwrap();
        let merged = merge(vec![m1, m2]);
        assert_eq!(merged.len(), 1);
        assert!(matches!(
            merged[0].visibility,
            crate::manifest::schema::Visibility::Private
        ));
    }

    #[test]
    fn load_file_reads_yaml() {
        let mut tf = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tf,
            r"schema_version: 1
skills:
  - name: bar
    source: {{ type: git, url: 'https://example.com/bar.git' }}
    visibility: public"
        )
        .unwrap();
        let m = load_file(tf.path()).unwrap();
        assert_eq!(m.skills.len(), 1);
        assert_eq!(m.skills[0].name, "bar");
    }
}
