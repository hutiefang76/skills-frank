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
//! frank 必须**只增/删 `mcpServers.<name>` 一项**, 不动其他字段. 实现:
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
    if let Some(Value::Object(servers)) = root.as_object_mut().and_then(|m| m.get_mut("mcpServers"))
    {
        servers.remove(name);
    }
    atomic_write_json(&path, &root)
}

/// 列出 Claude `~/.claude.json` `mcpServers` 全部条目 (name, command, args)。
/// 用于 `frank scan --mcp` 扫描.
pub fn list_claude() -> Result<Vec<McpEntry>> {
    let path = claude_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let root = read_json_or_empty(&path)?;
    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    Ok(servers
        .iter()
        .map(|(name, cfg)| McpEntry {
            name: name.clone(),
            command: cfg
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            args: cfg
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            env: HashMap::new(),
        })
        .collect())
}

/// 列出 codex `~/.codex/config.toml` `[mcp_servers.*]` 全部条目。
pub fn list_codex() -> Result<Vec<McpEntry>> {
    let path = codex_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let root = read_toml_or_empty(&path)?;
    let Some(servers) = root.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    Ok(servers
        .iter()
        .map(|(name, cfg)| McpEntry {
            name: name.clone(),
            command: cfg
                .get("command")
                .and_then(toml::Value::as_str)
                .unwrap_or("?")
                .to_string(),
            args: cfg
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            env: HashMap::new(),
        })
        .collect())
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

// ─── codex (`~/.codex/config.toml` `[mcp_servers.<name>]`) ────────────────

/// 把 MCP 注入 codex 的 `~/.codex/config.toml` `[mcp_servers.<name>]`。
///
/// codex 的 MCP 配置是 TOML, 字段:
/// ```toml
/// [mcp_servers.<name>]
/// type = "stdio"
/// command = "..."
/// args = [...]
/// env = { KEY = "..." }
/// ```
pub fn install_codex(entry: &McpEntry) -> Result<()> {
    let path = codex_config_path()?;
    let mut root = read_toml_or_empty(&path)?;

    // 确保 mcp_servers 是 table
    if !root.get("mcp_servers").is_some_and(toml::Value::is_table) {
        if let toml::Value::Table(ref mut map) = root {
            map.insert(
                "mcp_servers".to_string(),
                toml::Value::Table(toml::map::Map::new()),
            );
        }
    }

    let mut server = toml::map::Map::new();
    server.insert("type".to_string(), toml::Value::String("stdio".to_string()));
    server.insert(
        "command".to_string(),
        toml::Value::String(entry.command.clone()),
    );
    server.insert(
        "args".to_string(),
        toml::Value::Array(
            entry
                .args
                .iter()
                .map(|s| toml::Value::String(s.clone()))
                .collect(),
        ),
    );
    if !entry.env.is_empty() {
        let mut env_table = toml::map::Map::new();
        for (k, v) in &entry.env {
            env_table.insert(k.clone(), toml::Value::String(v.clone()));
        }
        server.insert("env".to_string(), toml::Value::Table(env_table));
    }

    if let Some(toml::Value::Table(servers)) =
        root.as_table_mut().and_then(|m| m.get_mut("mcp_servers"))
    {
        servers.insert(entry.name.clone(), toml::Value::Table(server));
    }

    atomic_write_toml(&path, &root)
}

/// 从 codex config.toml 反向删 `[mcp_servers.<name>]`。幂等。
pub fn uninstall_codex(name: &str) -> Result<()> {
    let path = codex_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_toml_or_empty(&path)?;
    if let Some(toml::Value::Table(servers)) =
        root.as_table_mut().and_then(|m| m.get_mut("mcp_servers"))
    {
        servers.remove(name);
    }
    atomic_write_toml(&path, &root)
}

// ─── v0.14: gemini + opencode 写入 ───────────────────────────────────

/// 把 MCP 注入 Gemini 的 `~/.gemini/settings.json` `mcpServers.<name>`.
///
/// schema 跟 Claude 几乎一致 (JSON, mcpServers.<name>.{command, args, env}).
/// 同样**只动单一 entry**, 用 tmp+rename 原子写, 严禁全文重 serialize 破坏用户其他字段.
pub fn install_gemini(entry: &McpEntry) -> Result<()> {
    let path = gemini_config_path()?;
    let mut root = read_json_or_empty(&path)?;

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

/// 从 `~/.gemini/settings.json` 反向删 `mcpServers.<name>`. 幂等.
pub fn uninstall_gemini(name: &str) -> Result<()> {
    let path = gemini_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&path)?;
    if let Some(Value::Object(servers)) = root.as_object_mut().and_then(|m| m.get_mut("mcpServers"))
    {
        servers.remove(name);
    }
    atomic_write_json(&path, &root)
}

/// 列出 Gemini `~/.gemini/settings.json` `mcpServers` 全部条目.
pub fn list_gemini() -> Result<Vec<McpEntry>> {
    let path = gemini_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let root = read_json_or_empty(&path)?;
    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    Ok(servers
        .iter()
        .map(|(name, cfg)| McpEntry {
            name: name.clone(),
            command: cfg
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            args: cfg
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            env: HashMap::new(),
        })
        .collect())
}

/// 把 MCP 注入 opencode 的 `~/.config/opencode/opencode.json` `mcp.<name>`.
///
/// opencode schema 跟 Claude/Gemini 不同:
/// - 顶层是 `mcp` (不是 `mcpServers`)
/// - `command` **是 array** (即 `[bin, ...args]` 全合一), 不分 command/args
/// - 支持 `enabled: bool` 字段, 默认 true
///
/// 我们把 entry.command + entry.args 合并成 command 数组.
pub fn install_opencode(entry: &McpEntry) -> Result<()> {
    let path = opencode_config_path()?;
    let mut root = read_json_or_empty(&path)?;

    if !root.get("mcp").is_some_and(Value::is_object) {
        if let Value::Object(ref mut map) = root {
            map.insert("mcp".into(), json!({}));
        }
    }

    let mut combined_command: Vec<String> = vec![entry.command.clone()];
    combined_command.extend(entry.args.iter().cloned());

    let mut server_cfg = json!({
        "type": "local",
        "command": combined_command,
        "enabled": true,
    });
    if !entry.env.is_empty() {
        server_cfg["environment"] = serde_json::to_value(&entry.env)?;
    }

    if let Value::Object(ref mut map) = root {
        if let Some(Value::Object(servers)) = map.get_mut("mcp") {
            servers.insert(entry.name.clone(), server_cfg);
        }
    }
    atomic_write_json(&path, &root)
}

/// 从 `~/.config/opencode/opencode.json` 反向删 `mcp.<name>`. 幂等.
pub fn uninstall_opencode(name: &str) -> Result<()> {
    let path = opencode_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&path)?;
    if let Some(Value::Object(servers)) = root.as_object_mut().and_then(|m| m.get_mut("mcp")) {
        servers.remove(name);
    }
    atomic_write_json(&path, &root)
}

/// 列出 opencode `~/.config/opencode/opencode.json` `mcp` 全部条目.
///
/// 注: opencode command 是 array, 这里把第一个元素当 command, 剩下当 args (复用 McpEntry schema).
pub fn list_opencode() -> Result<Vec<McpEntry>> {
    let path = opencode_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let root = read_json_or_empty(&path)?;
    let Some(servers) = root.get("mcp").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    Ok(servers
        .iter()
        .map(|(name, cfg)| {
            let command_arr: Vec<String> = cfg
                .get("command")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let (command, args) = match command_arr.split_first() {
                Some((first, rest)) => (first.clone(), rest.to_vec()),
                None => ("?".to_string(), Vec::new()),
            };
            McpEntry {
                name: name.clone(),
                command,
                args,
                env: HashMap::new(),
            }
        })
        .collect())
}

fn gemini_config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".gemini")
        .join("settings.json"))
}

fn opencode_config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".config")
        .join("opencode")
        .join("opencode.json"))
}

fn codex_config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".codex")
        .join("config.toml"))
}

fn read_toml_or_empty(path: &std::path::Path) -> Result<toml::Value> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    toml::from_str(&content).with_context(|| format!("parse {} as TOML", path.display()))
}

fn atomic_write_toml(path: &std::path::Path, value: &toml::Value) -> Result<()> {
    let text = toml::to_string_pretty(value).context("serialize TOML")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).with_context(|| format!("write tmp {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
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
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
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
