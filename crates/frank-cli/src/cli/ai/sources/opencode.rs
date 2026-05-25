//! 读 `~/.config/opencode/opencode.json` 的 `provider.<name>.models.<id>` 嵌套.
//!
//! opencode 配置例子 (实测用户机器):
//! ```json
//! {
//!   "provider": {
//!     "xiaomi": {
//!       "name": "Xiaomi MiMo",
//!       "models": {
//!         "mimo-v2-pro": { "name": "MiMo V2 Pro" }
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! 一个用户可能配 N 个 provider, 每个 provider 又有 M 个 model. frank 全列, 名字按
//! `<provider>/<model_id>` 拼 (例 `xiaomi/mimo-v2-pro`) — 跟 opencode 自家约定一致.
//!
//! # 为什么不 spawn `opencode models` (v0.10.7 的旧方案)
//!
//! v0.10.7 跑 `opencode models` 子进程时, opencode 初始化会扫照片/音乐库/网络宗卷,
//! macOS TCC 弹一堆 "frank 想访问 ..." 权限对话框 + daemon 卡死. 用户原话:
//! "你干嘛了又在要访问权限?"
//!
//! v0.10.8 改读配置文件 (zero-IO 路径), 完全规避 TCC.

use std::path::PathBuf;

use super::{ModelEntry, ModelSource};

/// 拿 `~/.config/opencode/opencode.json` 路径.
///
/// 注意: opencode **不在** `~/.opencode/`, 而是 XDG 风格 `~/.config/opencode/`.
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join(".config")
            .join("opencode")
            .join("opencode.json")
    })
}

/// 读 opencode 配置里所有 provider 的 models.
///
/// 输出 `provider_name/model_id` 列表 (例 `xiaomi/mimo-v2-pro`).
/// provider 顺序按 JSON 出现顺序保留 (serde_json::Value::as_object 是 BTreeMap, 字典序).
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

    let mut out = Vec::new();
    let Some(providers) = v.get("provider").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };

    for (provider_name, provider_cfg) in providers {
        let Some(models) = provider_cfg.get("models").and_then(serde_json::Value::as_object) else {
            continue;
        };
        for model_id in models.keys() {
            if model_id.is_empty() {
                continue;
            }
            out.push(ModelEntry {
                name: format!("{provider_name}/{model_id}"),
                source: ModelSource::ConfigFile(path.clone()),
            });
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
    fn reads_single_provider_single_model() {
        with_temp_home(|home| {
            let dir = home.join(".config").join("opencode");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("opencode.json"),
                r#"{
  "provider": {
    "xiaomi": {
      "name": "Xiaomi MiMo",
      "models": {
        "mimo-v2-pro": { "name": "MiMo V2 Pro" }
      }
    }
  }
}"#,
            )
            .unwrap();
            let models = read_models();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].name, "xiaomi/mimo-v2-pro");
        });
    }

    #[test]
    fn reads_multi_provider_multi_model() {
        with_temp_home(|home| {
            let dir = home.join(".config").join("opencode");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("opencode.json"),
                r#"{
  "provider": {
    "xiaomi": {
      "models": {
        "mimo-v2-pro": {},
        "mimo-v2": {}
      }
    },
    "alibaba": {
      "models": {
        "qwen3-coder": {}
      }
    }
  }
}"#,
            )
            .unwrap();
            let names: Vec<String> = read_models().into_iter().map(|m| m.name).collect();
            assert!(names.contains(&"xiaomi/mimo-v2-pro".to_string()));
            assert!(names.contains(&"xiaomi/mimo-v2".to_string()));
            assert!(names.contains(&"alibaba/qwen3-coder".to_string()));
            assert_eq!(names.len(), 3);
        });
    }

    #[test]
    fn empty_when_no_provider_field() {
        with_temp_home(|home| {
            let dir = home.join(".config").join("opencode");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("opencode.json"),
                r#"{"mcp": {}, "plugin": []}"#,
            )
            .unwrap();
            assert!(read_models().is_empty());
        });
    }

    #[test]
    fn empty_when_provider_has_no_models_field() {
        with_temp_home(|home| {
            let dir = home.join(".config").join("opencode");
            std::fs::create_dir_all(&dir).unwrap();
            // provider 配了 baseURL 但没 models 段 (不算配过, 跳过)
            std::fs::write(
                dir.join("opencode.json"),
                r#"{"provider": {"x": {"baseURL": "x"}}}"#,
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
            let dir = home.join(".config").join("opencode");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("opencode.json"), "broken").unwrap();
            assert!(read_models().is_empty());
        });
    }
}
