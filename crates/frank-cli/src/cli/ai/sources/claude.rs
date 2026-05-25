//! 读 `~/.claude/settings.json` 的 `"model"` 字段.
//!
//! claude CLI 的配置文件长这样 (官方 anthropic-quickstart 例):
//! ```json
//! {
//!   "model": "haiku",
//!   "theme": "light",
//!   ...
//! }
//! ```
//!
//! 用户切 provider 时 cc-switch 等工具会改这个 `"model"` 字段值.
//! frank 只读, 拿到啥认啥.
//!
//! # 不存在的字段
//!
//! 字段没配 (老用户从来没改过) = 返回空 Vec, 调用方走兜底 alias.

use std::path::PathBuf;

use super::{ModelEntry, ModelSource};

/// 拿 `~/.claude/settings.json` 路径. 跟其他 frank 模块一致, 走 `dirs::home_dir`.
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// 读 claude 配置里的 model 字段.
///
/// 全 read-only, 静默 fallback:
/// - 文件不存在 (用户没装 claude 或刚装没用过) → 空 Vec
/// - JSON 解析失败 (配置坏了) → 空 Vec + tracing::warn
/// - 没 `"model"` 字段 → 空 Vec (用户从没改过, 走兜底)
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

    // 当前 claude 配置只用 "model" 字段 (单值, 不是数组).
    // 未来若 anthropic 加多 profile 支持, 这里扩展即可.
    let mut out = Vec::new();
    if let Some(name) = v.get("model").and_then(|m| m.as_str()) {
        if !name.is_empty() {
            out.push(ModelEntry {
                name: name.to_string(),
                source: ModelSource::ConfigFile(path),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 设临时 HOME → 写 settings.json → 验证读取.
    ///
    /// 拿 `history_store::test_home_lock()` 全局锁 — `cli::ai::*` 测试共用一把锁,
    /// 避免 4 个 reader 的测试并跑时 HOME 互相覆盖.
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
            let dir = home.join(".claude");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("settings.json"),
                r#"{"model": "haiku", "theme": "light"}"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "haiku");
            assert!(matches!(models[0].source, ModelSource::ConfigFile(_)));
        });
    }

    #[test]
    fn returns_empty_when_no_model_field() {
        with_temp_home(|home| {
            let dir = home.join(".claude");
            std::fs::create_dir_all(&dir).unwrap();
            // 老用户配置 — 只配了 theme, 没 model
            std::fs::write(dir.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn returns_empty_when_file_missing() {
        with_temp_home(|_home| {
            // 不创建文件 → 返回空, 不 panic
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn returns_empty_when_json_corrupt() {
        with_temp_home(|home| {
            let dir = home.join(".claude");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), "{ not json !").unwrap();
            // 不 panic, 返回空 + 只打 warn
            assert!(read_models().is_empty());
        });
    }
}
