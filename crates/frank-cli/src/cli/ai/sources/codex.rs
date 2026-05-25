//! 读 `~/.codex/config.toml` 的 `model` 字段 + `[profiles.*]` 段.
//!
//! codex 配置例子 (实测用户机器):
//! ```toml
//! disable_response_storage = true
//! model = "gpt-5.5"
//! model_reasoning_effort = "high"
//!
//! [profiles.fast]
//! model = "gpt-5.4-mini"
//! ```
//!
//! 顶层 `model` 是当前默认, `[profiles.*]` 段每个 profile 一个 model (用户用
//! `codex --profile fast` 切到对应模型). frank 把两者都拉出来.
//!
//! # 兼容性
//!
//! 当前用户机器**没有** `[profiles.*]` 段 (只有顶层 model). frank 解析时 profiles
//! 部分用 `.unwrap_or_default()` 兜底, 没就跳过, 不会 panic.

use std::path::PathBuf;

use super::{ModelEntry, ModelSource};

/// 拿 `~/.codex/config.toml` 路径.
fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("config.toml"))
}

/// 读 codex 配置里的所有 model (顶层 + profiles).
///
/// 返回顺序: 顶层 model 在前, profiles 段在后 (用户最常用的当前默认排第一).
#[must_use]
pub fn read_models() -> Vec<ModelEntry> {
    let Some(path) = config_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let v: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{} 解析失败 ({e}); 跳过", path.display());
            return Vec::new();
        }
    };

    let mut out = Vec::new();

    // 顶层 model
    if let Some(name) = v.get("model").and_then(|m| m.as_str()) {
        if !name.is_empty() {
            out.push(ModelEntry {
                name: name.to_string(),
                source: ModelSource::ConfigFile(path.clone()),
            });
        }
    }

    // [profiles.*] 每段一个 model (可能多个 profile, 各自配不同模型)
    if let Some(profiles) = v.get("profiles").and_then(toml::Value::as_table) {
        for (_profile_name, profile_cfg) in profiles {
            if let Some(name) = profile_cfg.get("model").and_then(|m| m.as_str()) {
                if !name.is_empty() {
                    out.push(ModelEntry {
                        name: name.to_string(),
                        source: ModelSource::ConfigFile(path.clone()),
                    });
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let td = tempfile::tempdir().expect("tempdir");
        let old = std::env::var_os("HOME");
        std::env::set_var("HOME", td.path());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(td.path())));
        if let Some(o) = old {
            std::env::set_var("HOME", o);
        } else {
            std::env::remove_var("HOME");
        }
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }

    #[test]
    fn reads_top_level_model_only() {
        with_temp_home(|home| {
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir).unwrap();
            // 实测用户机器格式: 只有顶层 model, 没 profiles
            std::fs::write(
                dir.join("config.toml"),
                r#"
disable_response_storage = true
model = "gpt-5.5"
model_reasoning_effort = "high"
"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "gpt-5.5");
        });
    }

    #[test]
    fn reads_top_plus_profiles() {
        with_temp_home(|home| {
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("config.toml"),
                r#"
model = "gpt-5.5"

[profiles.fast]
model = "gpt-5.4-mini"

[profiles.experimental]
model = "o3"
"#,
            )
            .unwrap();
            let models = read_models();
            let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
            // 顶层在前
            assert_eq!(names[0], "gpt-5.5");
            assert!(names.contains(&"gpt-5.4-mini"));
            assert!(names.contains(&"o3"));
            assert_eq!(names.len(), 3);
        });
    }

    #[test]
    fn returns_empty_when_no_model() {
        with_temp_home(|home| {
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.toml"), "disable_response_storage = true").unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn returns_empty_when_file_missing() {
        with_temp_home(|_| {
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn returns_empty_when_toml_corrupt() {
        with_temp_home(|home| {
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.toml"), "not valid = = toml").unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn ignores_profile_without_model_field() {
        with_temp_home(|home| {
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir).unwrap();
            // profile 段有但没 model 字段 — 跳过别 panic
            std::fs::write(
                dir.join("config.toml"),
                r#"
model = "gpt-5.5"

[profiles.fast]
approval_policy = "never"
"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "gpt-5.5");
        });
    }
}
