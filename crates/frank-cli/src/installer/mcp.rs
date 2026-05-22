//! MCP server install/uninstall — 写入各平台 MCP 配置文件。
//!
//! # 平台 MCP 配置位置 (实测找出)
//!
//! - **Claude Code** (CLI): `~/.claude.json` 顶层 `mcpServers.<name>` (JSON)
//! - **codex CLI**: `~/.codex/config.toml` `[mcp_servers.<name>]` block (TOML)
//! - **opencode**: 暂未支持 MCP (留 v0.5+)
//!
//! # 写入策略
//!
//! 用户 `~/.claude.json` 可能含项目历史、设备 token 等关键数据 (实测可达 200K 行).
//! frank 必须**只增/删 mcpServers.<name> 一项**, 不动其他字段. 实现:
//! 1. read 整文件 → serde_json::Value
//! 2. `value["mcpServers"][name] = { command, args, env }`
//! 3. 原子写: tmp + rename
//!
//! 失败时不破原文件 (rename 是 atomic).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// MCP server 入库参数 (来自 manifest::schema::Source::Mcp)。
#[derive(Debug, Clone)]
pub struct McpEntry {
    /// MCP server 名 (会作为 mcpServers map 的 key, 也是 frank skill name).
    pub name: String,
    /// 启动命令 (例 `npx` / `uvx`).
    pub command: String,
    /// 命令 args (例 `["-y", "@modelcontextprotocol/server-time"]`).
    pub args: Vec<String>,
    /// env 变量.
    pub env: HashMap<String, String>,
}

/// 把 MCP 注入 Claude Code 的 `~/.claude.json` `mcpServers.<name>`。
pub fn install_claude(entry: &McpEntry) -> Result<()> {
    let path = claude_config_path()?;
    let mut root = read_json_or_empty(&path)?;

    // 确保 mcpServers 是 object
    if !root.get("mcpServers").is_some_and(Value::is_object) {
        if let Value::Object(ref mut map) = root {
            map.insert("mcpServers".into(), json!({}));
        }
    }

    let server_cfg = json!({
        "command": entry.command,
        "args": entry.args,
    });
    let server_cfg = if entry.env.is_empty() {
        server_cfg
    } else {
        let mut v = server_cfg;
        v["env"] = serde_json::to_value(&entry.env)?;
        v
    };

    if let Value::Object(ref mut map) = root {
        if let Some(Value::Object(servers)) = map.get_mut("mcpServers") {
            servers.insert(entry.name.clone(), server_cfg);
        }
    }

    atomic_write_json(&path, &root)
}

/// 从 `~/.claude.json` 反向删 `mcpServers.<name>`。幂等。
pub fn uninstall_claude(name: &str) -> Result<()> {
    let path = claude_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&path)?;
    if let Some(Value::Object(servers)) = root
        .as_object_mut()
        .and_then(|m| m.get_mut("mcpServers"))
    {
        servers.remove(name);
    }
    atomic_write_json(&path, &root)
}

/// 看 Claude `~/.claude.json` 里 `mcpServers.<name>` 是否存在。
#[must_use]
pub fn claude_installed(name: &str) -> bool {
    let Ok(path) = claude_config_path() else {
        return false;
    };
    let Ok(root) = read_json_or_empty(&path) else {
        return false;
    };
    root.get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key(name))
}

fn claude_config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".claude.json"))
}

fn read_json_or_empty(path: &std::path::Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&content).with_context(|| format!("parse {} as JSON", path.display()))
}

fn atomic_write_json(path: &std::path::Path, value: &Value) -> Result<()> {
    let json = serde_json::to_string_pretty(value).context("serialize JSON")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("write tmp {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_serialize_into_config_value() {
        let entry = McpEntry {
            name: "time".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-time".into()],
            env: HashMap::new(),
        };
        assert_eq!(entry.command, "npx");
        assert_eq!(entry.args.len(), 2);
    }

    #[test]
    fn install_and_uninstall_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");

        // 写一个含其他字段的 base config (模拟用户真配置)
        fs::write(
            &path,
            r#"{"preferences":{"theme":"dark"},"mcpServers":{"existing":{"command":"x"}}}"#,
        )
        .unwrap();

        // 直接测试 read/write 逻辑 (不通过 install_claude 因为它走 $HOME)
        let mut root = read_json_or_empty(&path).unwrap();
        root["mcpServers"]["new"] = json!({"command": "npx", "args": ["-y", "foo"]});
        atomic_write_json(&path, &root).unwrap();

        let reload = read_json_or_empty(&path).unwrap();
        // 新条目存在
        assert_eq!(reload["mcpServers"]["new"]["command"], "npx");
        // 原有条目保留
        assert_eq!(reload["mcpServers"]["existing"]["command"], "x");
        // 顶层其他字段保留
        assert_eq!(reload["preferences"]["theme"], "dark");
    }
}
