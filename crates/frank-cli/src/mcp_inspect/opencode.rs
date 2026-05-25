//! opencode (`~/.config/opencode/opencode.json`) MCP memory reader.
//!
//! opencode 配置是 JSONC (允许 `//` 注释 + 尾逗号), 顶层 key 是 `mcp` (不是
//! `mcpServers`), 形如:
//! ```jsonc
//! {
//!   "$schema": "https://opencode.ai/config.json",
//!   "mcp": {
//!     "memory": {
//!       "type": "local",
//!       "command": ["npx", "-y", "@modelcontextprotocol/server-memory"],
//!       "enabled": true
//!     }
//!   }
//! }
//! ```
//!
//! 关键差异 (vs claude/gemini):
//! - 顶层 key `mcp` (非 `mcpServers`)
//! - `command` 是单个数组 `["npx", "-y", "..."]` (含 command + args 一体)
//! - 有 `enabled: true/false` 字段
//! - 是 JSONC, 用 `json5` crate 解析容忍注释

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::{is_official_combined, OfficialMcp};

/// 读 `~/.config/opencode/opencode.json` 探测 official memory MCP。
pub fn read() -> (Option<PathBuf>, Option<OfficialMcp>) {
    let Some(home) = dirs::home_dir() else {
        return (None, None);
    };
    let path = home.join(".config").join("opencode").join("opencode.json");
    if !path.exists() {
        return (Some(path), None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(?path, error = %e, "opencode mcp_inspect: read failed");
            return (Some(path), None);
        }
    };
    if raw.trim().is_empty() {
        return (Some(path), None);
    }
    // 用 json5 容忍注释 / 尾逗号
    let root: Value = match json5::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?path, error = %e, "opencode mcp_inspect: parse failed");
            return (Some(path), None);
        }
    };
    (Some(path), detect(&root))
}

fn detect(root: &Value) -> Option<OfficialMcp> {
    let entries = root.get("mcp")?.as_object()?;
    for (entry_name, cfg) in entries {
        // command 是数组 ["npx", "-y", "..."]
        let command: Vec<String> = cfg
            .get("command")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if is_official_combined(&command) {
            let disabled = !cfg.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            return Some(OfficialMcp {
                entry_name: entry_name.clone(),
                disabled,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<OfficialMcp> {
        let root: Value = json5::from_str(json).expect("test json5 valid");
        detect(&root)
    }

    #[test]
    fn detect_combined_command_array_npx_memory() {
        let json = r#"{
          "mcp": {
            "memory": {
              "type": "local",
              "command": ["npx", "-y", "@modelcontextprotocol/server-memory"],
              "enabled": true
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert_eq!(res.entry_name, "memory");
        assert!(!res.disabled);
    }

    #[test]
    fn detect_enabled_false_marks_disabled() {
        let json = r#"{
          "mcp": {
            "memory": {
              "command": ["npx", "-y", "@modelcontextprotocol/server-memory"],
              "enabled": false
            }
          }
        }"#;
        assert!(parse(json).unwrap().disabled);
    }

    #[test]
    fn jsonc_with_comments_and_trailing_comma_ok() {
        // json5 容忍 // 注释 + 尾逗号
        let json = r#"{
          // 用户自定义注释
          "mcp": {
            "memory": {
              "command": ["uvx", "mcp-server-memory"],
              "enabled": true,
            },
          },
        }"#;
        let res = parse(json).expect("should detect");
        assert_eq!(res.entry_name, "memory");
    }

    #[test]
    fn ignore_non_memory_mcp() {
        let json = r#"{
          "mcp": {
            "time": {
              "command": ["npx", "-y", "@modelcontextprotocol/server-time"]
            }
          }
        }"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn enabled_true_explicit_not_disabled() {
        let json = r#"{
          "mcp": {
            "memory": {
              "command": ["npx", "-y", "@modelcontextprotocol/server-memory"],
              "enabled": true
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert!(!res.disabled);
    }

    #[test]
    fn missing_enabled_defaults_not_disabled() {
        // 没有 enabled 字段 → 默认视作 true (生效) → disabled = false
        let json = r#"{
          "mcp": {
            "memory": {
              "command": ["npx", "-y", "@modelcontextprotocol/server-memory"]
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert!(!res.disabled);
    }
}
