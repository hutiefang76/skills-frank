//! 读 `~/.gemini/settings.json` 的 `model` / `defaultModel` 字段.
//!
//! gemini CLI 的配置文件 schema 不太一致 (Google 内部不同版本叫法不同):
//! - 早期: `"model": "gemini-2.5-pro"`
//! - 近期: `"defaultModel": "gemini-2.5-pro"` 或 `"selectedModel": "..."`
//!
//! 实测用户机器的 `~/.gemini/settings.json` **没有任何 model 字段** (用户走中转, 模型
//! 在 server 端决定). 所以本读取实现:
//! - 尽量多字段名 fallback (model / defaultModel / selectedModel)
//! - 一个都没有 → 空 Vec, 走兜底 alias
//! - 文件不存在 → 空 Vec
//!
//! frank 不预判字段会有, 实测过就放进 fallback 链.

use std::path::PathBuf;

use super::{ModelEntry, ModelSource};

/// 拿 `~/.gemini/settings.json` 路径.
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gemini").join("settings.json"))
}

/// 尝试的 model 字段名 (按 gemini CLI 历史叫法, 第一个找到就用).
const MODEL_FIELD_NAMES: &[&str] = &["model", "defaultModel", "selectedModel"];

/// 读 gemini 配置里的 model 字段.
///
/// 字段 fallback 链: `model` → `defaultModel` → `selectedModel` → 空.
#[must_use]
pub fn read_models() -> Vec<ModelEntry> {
    let Some(path) = settings_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{} 解析失败 ({e}); 跳过", path.display());
            return Vec::new();
        }
    };

    // 顺序尝试每个字段名, 拿到第一个就停 (不重复加同 model)
    for field in MODEL_FIELD_NAMES {
        if let Some(name) = v.get(*field).and_then(|m| m.as_str()) {
            if !name.is_empty() {
                return vec![ModelEntry {
                    name: name.to_string(),
                    source: ModelSource::ConfigFile(path),
                }];
            }
        }
    }
    Vec::new()
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
    fn reads_model_field() {
        with_temp_home(|home| {
            let dir = home.join(".gemini");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), r#"{"model": "gemini-3.1-pro"}"#).unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "gemini-3.1-pro");
        });
    }

    #[test]
    fn falls_back_to_default_model_field() {
        with_temp_home(|home| {
            let dir = home.join(".gemini");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("settings.json"),
                r#"{"defaultModel": "gemini-2.5-flash"}"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "gemini-2.5-flash");
        });
    }

    #[test]
    fn empty_when_no_model_field() {
        with_temp_home(|home| {
            // 实测用户机器情况: 配了 apiKey 等但没 model 字段
            let dir = home.join(".gemini");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("settings.json"),
                r#"{"apiKey": "xxx", "baseUrl": "https://example.com"}"#,
            )
            .unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn empty_when_file_missing() {
        with_temp_home(|_| {
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn empty_when_corrupt_json() {
        with_temp_home(|home| {
            let dir = home.join(".gemini");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), "broken json").unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn model_field_wins_over_default_model() {
        with_temp_home(|home| {
            let dir = home.join(".gemini");
            std::fs::create_dir_all(&dir).unwrap();
            // 两个都有 — model 在前优先
            std::fs::write(
                dir.join("settings.json"),
                r#"{"model": "A", "defaultModel": "B"}"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "A");
        });
    }
}
