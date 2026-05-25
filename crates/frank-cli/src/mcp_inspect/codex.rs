//! codex (`~/.codex/config.toml`) MCP memory reader.
//!
//! codex 配置是 TOML, 形如:
//! ```toml
//! [mcp_servers.memory]
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-memory"]
//! enabled = false  # 可选, 用户禁用
//! ```

use std::fs;
use std::path::PathBuf;

use super::{is_official, OfficialMcp};

/// 读 `~/.codex/config.toml` 探测 official memory MCP。
pub fn read() -> (Option<PathBuf>, Option<OfficialMcp>) {
    let Some(home) = dirs::home_dir() else {
        return (None, None);
    };
    let path = home.join(".codex").join("config.toml");
    if !path.exists() {
        return (Some(path), None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(?path, error = %e, "codex mcp_inspect: read failed");
            return (Some(path), None);
        }
    };
    if raw.trim().is_empty() {
        return (Some(path), None);
    }
    let root: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?path, error = %e, "codex mcp_inspect: parse failed");
            return (Some(path), None);
        }
    };
    (Some(path), detect(&root))
}

fn detect(root: &toml::Value) -> Option<OfficialMcp> {
    let servers = root.get("mcp_servers").and_then(toml::Value::as_table)?;
    for (entry_name, cfg) in servers {
        let command = cfg
            .get("command")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        let args: Vec<String> = cfg
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if is_official(command, &args) {
            let disabled = !cfg
                .get("enabled")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
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

    fn parse(toml_str: &str) -> Option<OfficialMcp> {
        let root: toml::Value = toml::from_str(toml_str).expect("test toml valid");
        detect(&root)
    }

    #[test]
    fn detect_npx_memory_block() {
        let toml = r#"
            [mcp_servers.memory]
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-memory"]
        "#;
        let res = parse(toml).expect("should detect");
        assert_eq!(res.entry_name, "memory");
        assert!(!res.disabled);
    }

    #[test]
    fn detect_enabled_false_flag() {
        let toml = r#"
            [mcp_servers.memory]
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-memory"]
            enabled = false
        "#;
        let res = parse(toml).expect("should detect");
        assert!(res.disabled);
    }

    #[test]
    fn ignore_non_memory_block() {
        let toml = r#"
            [mcp_servers.context7]
            command = "npx"
            args = ["-y", "@upstash/context7-mcp"]
        "#;
        assert!(parse(toml).is_none());
    }

    #[test]
    fn enabled_true_no_disable_flag() {
        let toml = r#"
            [mcp_servers.memory]
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-memory"]
            enabled = true
        "#;
        let res = parse(toml).expect("should detect");
        assert!(!res.disabled);
    }

    #[test]
    fn no_mcp_servers_section_returns_none() {
        let toml = r#"
            [other_section]
            foo = "bar"
        "#;
        assert!(parse(toml).is_none());
    }
}
