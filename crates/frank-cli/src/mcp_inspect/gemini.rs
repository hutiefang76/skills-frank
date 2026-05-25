//! Gemini CLI (`~/.gemini/settings.json`) MCP memory reader.
//!
//! Gemini 配置是纯 JSON, 形如:
//! ```json
//! {
//!   "apiKey": "...",
//!   "mcpServers": {
//!     "memory": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-memory"]
//!     }
//!   }
//! }
//! ```
//!
//! 与 Claude 的区别: 顶层无 `projects.*` 嵌套层, 单层即可。

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::{is_official, OfficialMcp};

/// 读 `~/.gemini/settings.json` 探测 official memory MCP。
pub fn read() -> (Option<PathBuf>, Option<OfficialMcp>) {
    let Some(home) = dirs::home_dir() else {
        return (None, None);
    };
    let path = home.join(".gemini").join("settings.json");
    if !path.exists() {
        return (Some(path), None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(?path, error = %e, "gemini mcp_inspect: read failed");
            return (Some(path), None);
        }
    };
    if raw.trim().is_empty() {
        return (Some(path), None);
    }
    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?path, error = %e, "gemini mcp_inspect: parse failed");
            return (Some(path), None);
        }
    };
    (Some(path), detect(&root))
}

fn detect(root: &Value) -> Option<OfficialMcp> {
    let servers = root.get("mcpServers")?.as_object()?;
    for (entry_name, cfg) in servers {
        let command = cfg.get("command").and_then(Value::as_str).unwrap_or("");
        let args: Vec<String> = cfg
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if is_official(command, &args) {
            return Some(OfficialMcp {
                entry_name: entry_name.clone(),
                disabled: false, // gemini JSON 无 disabled 字段
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<OfficialMcp> {
        let root: Value = serde_json::from_str(json).expect("test json valid");
        detect(&root)
    }

    #[test]
    fn detect_memory_entry() {
        let json = r#"{
          "apiKey": "fake",
          "mcpServers": {
            "memory": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-memory"]
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert_eq!(res.entry_name, "memory");
    }

    #[test]
    fn detect_uvx_form() {
        let json = r#"{
          "mcpServers": {
            "mem": {
              "command": "uvx",
              "args": ["mcp-server-memory"]
            }
          }
        }"#;
        assert_eq!(parse(json).unwrap().entry_name, "mem");
    }

    #[test]
    fn ignore_time_mcp() {
        let json = r#"{
          "mcpServers": {
            "time": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-time"]
            }
          }
        }"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn empty_no_mcpservers_key() {
        let json = r#"{"apiKey": "fake"}"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn read_missing_file_returns_none_no_panic() {
        // 直接调 read() 在没有 ~/.gemini/settings.json 的临时环境 → 应静默返回 None.
        // 这里仅验 read() 调用不 panic; 真实路径依赖用户 HOME, 不强制 None.
        let _ = read();
    }
}
