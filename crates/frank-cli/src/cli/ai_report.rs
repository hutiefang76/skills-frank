//! `frank ai ask` 的 CLI 输出解析 — claude/codex JSON 提取 token+session, 构造 `CallReport`。
//!
//! 输入: claude `--output-format json` 单行 JSON; codex `--json` JSONL 流。
//! 输出: `(reply_text, Option<CallReport>)` — 解析失败时 `report = None`, reply 走 raw,
//! **永不阻塞用户拿到回答** (CLI 版本 skew / 字段变化 / JSON 损坏都静默 fallback)。
//!
//! # 为什么不算 cost
//!
//! v0.10.5 实施后用户反馈: 中转站 (proxy / 共享账号) 价格跟官方不一样, 2026 年官方
//! 定价也会动好几次。frank 不知道用户走哪个 endpoint, 算成本反而是误导。**只输出
//! token 数**, 用户自己换算更准。
//!
//! TODO v0.11+: gemini/opencode token parsing — 当前用户主流是 claude/codex, 留空。

use frank_cred::report::{CallReport, CallSource};
use serde_json::Value;

/// 默认 claude provider 字段值。
const PROVIDER_CLAUDE: &str = "claude";
/// 默认 codex provider 字段值。
const PROVIDER_CODEX: &str = "codex";
/// 模型未知时填的占位符 (region: render 之后用户能一眼看出"模型上游没说")。
const UNKNOWN_MODEL: &str = "unknown";

/// 解析 `claude --print --output-format json` 的单行 JSON 输出。
///
/// Schema (claude code 2.x):
/// ```json
/// {
///   "result": "the reply text",
///   "model": "claude-sonnet-4-6",
///   "session_id": "uuid",
///   "usage": { "input_tokens": 8, "output_tokens": 5,
///              "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 },
///   "duration_ms": 1234
/// }
/// ```
///
/// 解析失败时返回 `(raw.to_string(), None)`, 不阻塞 reply 显示。
#[must_use]
pub fn parse_claude_json(raw: &str, fallback_latency_ms: u64) -> (String, Option<CallReport>) {
    let Ok(v) = serde_json::from_str::<Value>(raw.trim()) else {
        tracing::debug!("parse_claude_json: not JSON, fallback raw");
        return (raw.to_string(), None);
    };

    let reply = v
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or(raw)
        .to_string();
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(UNKNOWN_MODEL)
        .to_string();
    let session_id = v
        .get("session_id")
        .and_then(Value::as_str)
        .map(String::from);
    let (input_tokens, output_tokens) = v.get("usage").map_or((0, 0), |u| {
        (
            u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        )
    });

    let latency_ms = v
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(fallback_latency_ms);

    let report = CallReport {
        provider: PROVIDER_CLAUDE.to_string(),
        model,
        input_tokens,
        output_tokens,
        latency_ms,
        session_id,
        source: CallSource::SpawnedCli {
            bin: "claude".to_string(),
        },
        timestamp: chrono::Utc::now(),
    };
    (reply, Some(report))
}

/// 解析 `codex exec --json --skip-git-repo-check -` 的 JSONL 输出。
///
/// 实测 codex 1.x schema:
/// - `{type:"thread.started", thread_id}` (也兼容老 `thread.created`)
/// - `{type:"turn.started"}` (略过)
/// - `{type:"item.completed", item:{id, type:"agent_message", text:"..."}}` ← reply 在这
/// - `{type:"turn.completed", usage:{input_tokens, cached_input_tokens, output_tokens,
///    reasoning_output_tokens}}`
///
/// 兼容老 stream-delta schema: `agent_message_delta.delta` / `agent_message.message`。
///
/// 注意: codex 不输出 model 名 — 用 `model_hint` 参数传入 (CLI 取自用户 --model 或默认
/// `gpt-5.5`)。解析失败时返回 `(raw.to_string(), None)`。
#[must_use]
pub fn parse_codex_jsonl(
    raw: &str,
    fallback_latency_ms: u64,
    model_hint: &str,
) -> (String, Option<CallReport>) {
    let lines: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    if lines.is_empty() {
        tracing::debug!("parse_codex_jsonl: 0 valid json lines, fallback raw");
        return (raw.to_string(), None);
    }

    let mut reply = String::new();
    let mut thread_id: Option<String> = None;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    // codex 不上报 model — 用调用方传入的 hint (--model 优先, 否则 CLI 默认 gpt-5.5)
    let model = if model_hint.is_empty() {
        "gpt-5.5".to_string()
    } else {
        model_hint.to_string()
    };

    for ev in &lines {
        // codex 嵌套 `msg.type` 或顶层 `type`, 兼容两种
        let event_type = ev
            .get("msg")
            .and_then(|m| m.get("type"))
            .and_then(Value::as_str)
            .or_else(|| ev.get("type").and_then(Value::as_str));
        let msg = ev.get("msg").unwrap_or(ev);

        match event_type {
            Some("thread.created" | "thread.started" | "session_configured")
                if thread_id.is_none() =>
            {
                thread_id = msg
                    .get("thread_id")
                    .or_else(|| msg.get("session_id"))
                    .and_then(Value::as_str)
                    .map(String::from);
            }
            Some("item.completed") => {
                // 新版 codex 把 reply 文本放在 item.text 里 (item.type=agent_message)
                if let Some(item) = msg.get("item") {
                    let is_msg = item.get("type").and_then(Value::as_str) == Some("agent_message");
                    if is_msg {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            reply.push_str(text);
                        }
                    }
                }
            }
            Some("agent_message_delta") => {
                // 老 stream-delta schema 兼容
                if let Some(d) = msg.get("delta").and_then(Value::as_str) {
                    reply.push_str(d);
                }
            }
            Some("agent_message") => {
                // 终态 message: 替换 delta 累积值
                if let Some(m) = msg.get("message").and_then(Value::as_str) {
                    reply = m.to_string();
                }
            }
            Some("turn.completed") => {
                if let Some(usage) = msg.get("usage") {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(input_tokens);
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(output_tokens);
                }
            }
            _ => {}
        }
    }

    // 如果 reply 全空, fallback 用整段 raw (避免空 reply)
    if reply.is_empty() {
        reply = raw.to_string();
    }

    let report = CallReport {
        provider: PROVIDER_CODEX.to_string(),
        model,
        input_tokens,
        output_tokens,
        latency_ms: fallback_latency_ms,
        session_id: thread_id,
        source: CallSource::SpawnedCli {
            bin: "codex".to_string(),
        },
        timestamp: chrono::Utc::now(),
    };
    (reply, Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_json_full_schema() {
        let raw = r#"{
            "result": "hello world",
            "model": "claude-sonnet-4-6",
            "session_id": "abc12345-def6-7890",
            "usage": {"input_tokens": 8, "output_tokens": 5},
            "duration_ms": 1234
        }"#;
        let (reply, report) = parse_claude_json(raw, 9999);
        assert_eq!(reply, "hello world");
        let r = report.expect("must have report");
        assert_eq!(r.provider, "claude");
        assert_eq!(r.model, "claude-sonnet-4-6");
        assert_eq!(r.input_tokens, 8);
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.latency_ms, 1234);
        assert_eq!(r.session_id.as_deref(), Some("abc12345-def6-7890"));
    }

    #[test]
    fn parse_claude_json_fallback_when_garbage() {
        let raw = "not json at all just plain text";
        let (reply, report) = parse_claude_json(raw, 100);
        assert_eq!(reply, "not json at all just plain text");
        assert!(report.is_none());
    }

    #[test]
    fn parse_claude_json_unknown_model_still_reports_tokens() {
        // 即使 model 未知, token 数仍要上报 (用户照看)
        let raw = r#"{
            "result": "hi",
            "model": "claude-future-9",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let (_, report) = parse_claude_json(raw, 100);
        let r = report.unwrap();
        assert_eq!(r.model, "claude-future-9");
        assert_eq!(r.input_tokens, 10);
        assert_eq!(r.output_tokens, 5);
    }

    #[test]
    fn parse_codex_jsonl_item_completed_schema() {
        // codex 1.x 实测: thread.started + item.completed (agent_message text) + turn.completed
        let raw = r#"{"type":"thread.started","thread_id":"thd-abc"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}
{"type":"turn.completed","usage":{"input_tokens":18213,"cached_input_tokens":4992,"output_tokens":89,"reasoning_output_tokens":82}}"#;
        let (reply, report) = parse_codex_jsonl(raw, 8977, "");
        assert_eq!(reply, "OK");
        let r = report.unwrap();
        assert_eq!(r.provider, "codex");
        // model 走 hint default 因为传空
        assert_eq!(r.model, "gpt-5.5");
        assert_eq!(r.input_tokens, 18213);
        assert_eq!(r.output_tokens, 89);
        assert_eq!(r.session_id.as_deref(), Some("thd-abc"));
        assert_eq!(r.latency_ms, 8977);
    }

    #[test]
    fn parse_codex_jsonl_model_hint_overrides_default() {
        let raw = r#"{"type":"thread.started","thread_id":"x"}
{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}
{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":1}}"#;
        let (_, report) = parse_codex_jsonl(raw, 100, "gpt-5.5-pro");
        let r = report.unwrap();
        assert_eq!(r.model, "gpt-5.5-pro");
    }

    #[test]
    fn parse_codex_jsonl_old_delta_schema() {
        // 老 schema 仍兼容: thread.created + agent_message_delta 累积
        let raw = r#"{"type":"thread.created","thread_id":"abc"}
{"type":"agent_message_delta","delta":"hel"}
{"type":"agent_message_delta","delta":"lo"}
{"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":3}}"#;
        let (reply, report) = parse_codex_jsonl(raw, 500, "");
        assert_eq!(reply, "hello");
        let r = report.unwrap();
        assert_eq!(r.session_id.as_deref(), Some("abc"));
        assert_eq!(r.input_tokens, 12);
    }

    #[test]
    fn parse_codex_jsonl_nested_msg_format() {
        let raw = r#"{"id":"1","msg":{"type":"thread.started","thread_id":"abc"}}
{"id":"2","msg":{"type":"agent_message_delta","delta":"yo"}}
{"id":"3","msg":{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":1}}}"#;
        let (reply, report) = parse_codex_jsonl(raw, 200, "");
        assert_eq!(reply, "yo");
        let r = report.unwrap();
        assert_eq!(r.session_id.as_deref(), Some("abc"));
        assert_eq!(r.input_tokens, 4);
    }

    #[test]
    fn parse_codex_jsonl_agent_message_overrides_delta() {
        let raw = r#"{"type":"agent_message_delta","delta":"partial"}
{"type":"agent_message","message":"complete reply"}
{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}"#;
        let (reply, _) = parse_codex_jsonl(raw, 100, "");
        assert_eq!(reply, "complete reply");
    }

    #[test]
    fn parse_codex_jsonl_fallback_when_no_valid_lines() {
        let raw = "garbage\nmore garbage\n";
        let (reply, report) = parse_codex_jsonl(raw, 100, "");
        assert_eq!(reply, raw);
        assert!(report.is_none());
    }

    #[test]
    fn parse_codex_jsonl_empty_reply_falls_back_to_raw() {
        let raw = r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":0}}"#;
        let (reply, report) = parse_codex_jsonl(raw, 100, "");
        assert_eq!(reply, raw); // fallback
        let r = report.unwrap();
        assert_eq!(r.input_tokens, 5);
    }
}
