//! `frank mcp-serve` — frank 作为 MCP server, 让 AI 主动调 frank 能力 (v0.14 P2-H, ADR-014 §3.6).
//!
//! # 设计
//!
//! 这是"第 5 通道" — AI 通过标准 MCP 协议主动调 frank.add_memory / frank.search_memory /
//! frank.list_skills 等. 完全跟 Claude/Codex/Gemini 原生 MCP 框架兼容.
//!
//! 用户在 ~/.claude.json 配:
//! ```json
//! "mcpServers": {
//!   "frank": {
//!     "command": "frank",
//!     "args": ["mcp-serve"]
//!   }
//! }
//! ```
//! Claude 会 spawn `frank mcp-serve` 子进程, 走 stdin/stdout JSON-RPC 协议.
//!
//! # 协议
//!
//! MCP 用 JSON-RPC 2.0 over stdio:
//! - Claude → frank: `{"jsonrpc": "2.0", "id": 1, "method": "initialize", ...}`
//! - frank → Claude: `{"jsonrpc": "2.0", "id": 1, "result": {...}}`
//!
//! 必须支持的 methods:
//! - `initialize` — 协议握手, 返 `serverInfo` + `capabilities`
//! - `tools/list` — 返工具清单 (我们暴露 4 个: add_memory / search_memory / list_skills / tenant_status)
//! - `tools/call` — 调具体工具, params: `{name, arguments}`
//!
//! # v0.14 范围
//!
//! v0.14.3 先实现 stdio + 4 tool 最小集. 凭 frank 已有的 SyncClient + Registry 直接 wire,
//! 不引入额外 MCP SDK (rust-mcp 还不稳, 自己写 ~200 行 stdio loop 即可).

use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{json, Value};

/// `frank mcp-serve` 参数.
#[derive(Parser, Debug)]
pub struct Args {
    /// 协议版本协商 (默认 `2024-11-05`, Claude Code 当前版).
    #[arg(long, default_value = "2024-11-05")]
    pub protocol_version: String,
}

/// 入口 — stdio JSON-RPC loop.
pub fn run(args: Args) -> Result<()> {
    tracing::info!(
        protocol = %args.protocol_version,
        "frank mcp-serve starting (stdio JSON-RPC loop)"
    );
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "stdin read error, exiting");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(line = %line, error = %e, "invalid JSON-RPC, skipping");
                continue;
            }
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        tracing::debug!(method, ?params, "rpc request");

        let result = handle_request(method, &params, &args.protocol_version);

        // 通知 (无 id) 不发响应
        if id.is_none() {
            continue;
        }

        let response = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32603, "message": format!("{e:#}")}
            }),
        };
        writeln!(out, "{response}").context("write response")?;
        out.flush().context("flush stdout")?;
    }
    Ok(())
}

/// 派发: initialize / tools/list / tools/call / 其他.
fn handle_request(method: &str, params: &Value, proto_version: &str) -> Result<Value> {
    match method {
        "initialize" => Ok(initialize(proto_version)),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(params),
        // notifications/initialized (通知) 没 id, 不该走到这里, safety pass
        "notifications/initialized" => Ok(Value::Null),
        _ => anyhow::bail!("method not supported: {method}"),
    }
}

fn initialize(proto_version: &str) -> Value {
    json!({
        "protocolVersion": proto_version,
        "serverInfo": {
            "name": "frank-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": {},
        }
    })
}

/// 暴露给 AI 的 tool 清单. 4 个核心能力 (memory + skills + tenant 状态).
fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "frank_add_memory",
                "description": "Store a fact into frank distributed memory \
                    (LanceDB local + Qdrant server, Hybrid RRF retrieval). Use for facts \
                    user wants AI to remember across sessions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The fact to remember"},
                        "tag": {"type": "string", "description": "Optional session/project tag"}
                    },
                    "required": ["content"]
                }
            },
            {
                "name": "frank_search_memory",
                "description": "Semantic search in frank memory via Hybrid RRF \
                    (3-route: dense vector + BM25 + tag filter). Returns top-K matches.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"},
                        "k": {"type": "integer", "description": "Top-K results (default 5)"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "frank_list_skills",
                "description": "List all skills frank knows about (frank-official / \
                    frank-recommended / user community / team / private). Shows install \
                    status across 4 platforms (claude/codex/gemini/opencode).",
                "inputSchema": {"type": "object", "properties": {}}
            },
            {
                "name": "frank_tenant_status",
                "description": "Get current frank tenant info: tenant_id, records_count / \
                    quota (10k), deletion_scheduled_at if any. Useful when AI wants to know \
                    storage limits before adding many memories.",
                "inputSchema": {"type": "object", "properties": {}}
            }
        ]
    })
}

/// 派发到具体 tool. tool 名 + arguments.
fn tools_call(params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tools/call: missing `name`"))?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let text_result = match name {
        "frank_add_memory" => tool_add_memory(&args)?,
        "frank_search_memory" => tool_search_memory(&args)?,
        "frank_list_skills" => tool_list_skills()?,
        "frank_tenant_status" => tool_tenant_status()?,
        _ => anyhow::bail!("tool not found: {name}"),
    };
    // MCP tools/call 返回 content blocks
    Ok(json!({
        "content": [{"type": "text", "text": text_result}],
        "isError": false,
    }))
}

fn tool_add_memory(args: &Value) -> Result<String> {
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `content`"))?;
    let tag = args.get("tag").and_then(Value::as_str);
    let client = crate::sync_client::SyncClient::from_env_or_config()
        .context("init sync-agent client")?;
    let scope = frank_memory::Scope {
        user_id: std::env::var("USER").ok(),
        agent_id: Some("frank-mcp".to_string()),
        session_id: tag.map(String::from),
    };
    let id = client
        .add_raw(content, &scope, None)
        .context("sync-agent add_raw failed")?;
    Ok(format!("Saved memory id={id} content={content:?}"))
}

fn tool_search_memory(args: &Value) -> Result<String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `query`"))?;
    let k = args.get("k").and_then(Value::as_u64).unwrap_or(5);
    let client = crate::sync_client::SyncClient::from_env_or_config()
        .context("init sync-agent client")?;
    let scope = frank_memory::Scope {
        user_id: std::env::var("USER").ok(),
        agent_id: Some("frank-mcp".to_string()),
        session_id: None,
    };
    let matches = client
        .search(query, &scope, Some(k), None)
        .context("sync-agent search failed")?;
    let lines: Vec<String> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| format!("{}. [{:.3}] {}", i + 1, m.score, m.record.content))
        .collect();
    if lines.is_empty() {
        Ok(format!("No matches for query: {query:?}"))
    } else {
        Ok(format!("Top {} matches for {query:?}:\n{}", lines.len(), lines.join("\n")))
    }
}

fn tool_list_skills() -> Result<String> {
    let manifests = crate::manifest::parser::discover()?;
    let registry =
        crate::manifest::resolver::Registry::new(crate::manifest::parser::merge(manifests));
    let state = crate::state::State::load_default().unwrap_or_else(|_| {
        crate::state::State::load(std::env::temp_dir().join("frank-empty-state.json"))
            .expect("empty state load")
    });
    let lines: Vec<String> = registry
        .all()
        .iter()
        .map(|s| {
            let installed = state.get(&s.name).is_some();
            let mark = if installed { "✓" } else { "·" };
            format!("{} {} ({}) — {}", mark, s.name, format!("{:?}", s.visibility).to_lowercase(), s.description.chars().take(60).collect::<String>())
        })
        .collect();
    Ok(format!("Skills ({} total):\n{}", lines.len(), lines.join("\n")))
}

fn tool_tenant_status() -> Result<String> {
    let client = crate::sync_client::SyncClient::from_env_or_config()
        .context("init sync-agent client")?;
    let s = client.tenant_status().context("tenant_status failed")?;
    let tid = s.get("tenant_id").and_then(Value::as_str).unwrap_or("?");
    let records = s
        .get("records_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let deletion = s
        .get("deletion_scheduled_at")
        .and_then(Value::as_i64);
    use std::fmt::Write as _;
    let mut out = format!("tenant_id: {tid}\nrecords: {records}/10000");
    if let Some(ts) = deletion {
        let _ = write!(out, "\ndeletion_scheduled_at: {ts} (epoch)");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_server_info() {
        let v = initialize("2024-11-05");
        assert_eq!(
            v.get("serverInfo").and_then(|s| s.get("name")).and_then(Value::as_str),
            Some("frank-mcp")
        );
        assert_eq!(
            v.get("protocolVersion").and_then(Value::as_str),
            Some("2024-11-05")
        );
    }

    #[test]
    fn tools_list_has_four_tools() {
        let v = tools_list();
        let tools = v.get("tools").and_then(Value::as_array).expect("tools array");
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"frank_add_memory"));
        assert!(names.contains(&"frank_search_memory"));
        assert!(names.contains(&"frank_list_skills"));
        assert!(names.contains(&"frank_tenant_status"));
    }

    #[test]
    fn handle_unknown_method_errors() {
        let r = handle_request("nonsense/foo", &Value::Null, "2024-11-05");
        assert!(r.is_err());
    }
}
