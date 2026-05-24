//! Claude Code (`~/.claude.json`) MCP memory reader.
//!
//! Claude Code 的 MCP 配置有 2 个位置:
//! - 顶层 `mcpServers.<name>` (user scope)
//! - 嵌套 `projects.<path>.mcpServers.<name>` (project scope)
//!
//! 任一处出现 official memory MCP 都算 "装了"。

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::{is_official, OfficialMcp};

/// 读 `~/.claude.json` 探测 official memory MCP。
///
/// 返回 `(config_path, official_mcp)`. 任何 IO / parse 错误降级为
/// `(Some(path), None)` + log warn (config 在但坏了) 或 `(None, None)`。
pub fn read() -> (Option<PathBuf>, Option<OfficialMcp>) {
    let Some(home) = dirs::home_dir() else {
        return (None, None);
    };
    let path = home.join(".claude.json");
    if !path.exists() {
        return (Some(path), None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(?path, error = %e, "claude mcp_inspect: read failed");
            return (Some(path), None);
        }
    };
    if raw.trim().is_empty() {
        return (Some(path), None);
    }
    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?path, error = %e, "claude mcp_inspect: parse failed");
            return (Some(path), None);
        }
    };

    // 1) 顶层 mcpServers
    if let Some(found) = detect_in_servers(&root, "mcpServers") {
        return (Some(path), Some(found));
    }

    // 2) projects.<path>.mcpServers
    if let Some(projects) = root.get("projects").and_then(Value::as_object) {
        for (_proj_path, proj_cfg) in projects {
            if let Some(found) = detect_in_servers(proj_cfg, "mcpServers") {
                return (Some(path), Some(found));
            }
        }
    }

    (Some(path), None)
}

/// 在某个 JSON object 的 `<key>.{entry → {command, args}}` 中找 official memory。
fn detect_in_servers(parent: &Value, key: &str) -> Option<OfficialMcp> {
    let servers = parent.get(key)?.as_object()?;
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
            // claude JSON 没有 disabled 字段, 永远 false
            return Some(OfficialMcp {
                entry_name: entry_name.clone(),
                disabled: false,
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
        detect_in_servers(&root, "mcpServers")
            .or_else(|| {
                root.get("projects")
                    .and_then(Value::as_object)
                    .and_then(|projects| {
                        projects
                            .values()
                            .find_map(|p| detect_in_servers(p, "mcpServers"))
                    })
            })
    }

    #[test]
    fn detect_top_level_npx_memory() {
        let json = r#"{
          "mcpServers": {
            "memory": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-memory"]
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert_eq!(res.entry_name, "memory");
        assert!(!res.disabled);
    }

    #[test]
    fn detect_nested_project_scope_memory() {
        let json = r#"{
          "mcpServers": {},
          "projects": {
            "/Users/x/proj": {
              "mcpServers": {
                "mem": {
                  "command": "uvx",
                  "args": ["mcp-server-memory"]
                }
              }
            }
          }
        }"#;
        let res = parse(json).expect("should detect");
        assert_eq!(res.entry_name, "mem");
    }

    #[test]
    fn ignore_non_memory_mcp() {
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
    fn empty_config_no_detection() {
        let json = r"{}";
        assert!(parse(json).is_none());
    }
}
