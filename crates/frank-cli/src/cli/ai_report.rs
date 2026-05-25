//! `frank ai ask` 的 CLI 输出解析 — claude/codex JSON 提取 token+cost+session, 构造 `CallReport`。
//!
//! 输入: claude `--output-format json` 单行 JSON; codex `--json` JSONL 流。
//! 输出: `(reply_text, Option<CallReport>)` — 解析失败时 `report = None`, reply 走 raw,
//! **永不阻塞用户拿到回答** (CLI 版本 skew / 字段变化 / JSON 损坏都静默 fallback)。
//!
//! TODO v0.11+: gemini/opencode token parsing — 当前用户主流是 claude/codex, 留空。

use frank_cred::pricing::{compute_cost, PricingTable};
use frank_cred::report::{CallReport, CallSource, Confidence};
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
///   "total_cost_usd": 0.0001,
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

    // claude 自家给了 total_cost_usd 优先用 (跟 anthropic 后台对得上)
    let cost_usd = v
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .or_else(|| pricing_lookup_cost(&model, input_tokens, output_tokens));

    let latency_ms = v
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(fallback_latency_ms);

    let report = CallReport {
        provider: PROVIDER_CLAUDE.to_string(),
        model,
        input_tokens,
        output_tokens,
        cost_usd,
        latency_ms,
        session_id,
        source: CallSource::SpawnedCli {
            bin: "claude".to_string(),
        },
        timestamp: chrono::Utc::now(),
        confidence: if cost_usd.is_some() {
            Confidence::High
        } else {
            Confidence::Low
        },
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
                    let is_msg =
                        item.get("type").and_then(Value::as_str) == Some("agent_message");
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

    let cost_usd = pricing_lookup_cost(&model, input_tokens, output_tokens);
    let confidence = if cost_usd.is_some() {
        Confidence::Med // codex 无官方 cost, pricing 表的 gpt-5.5 标 med
    } else {
        Confidence::Low
    };

    let report = CallReport {
        provider: PROVIDER_CODEX.to_string(),
        model,
        input_tokens,
        output_tokens,
        cost_usd,
        latency_ms: fallback_latency_ms,
        session_id: thread_id,
        source: CallSource::SpawnedCli {
            bin: "codex".to_string(),
        },
        timestamp: chrono::Utc::now(),
        confidence,
    };
    (reply, Some(report))
}

/// pricing 表查 model → cost。未知模型返 `None` (调用方决定 fallback)。
fn pricing_lookup_cost(model: &str, input: u64, output: u64) -> Option<f64> {
    let table = PricingTable::load_with_override();
    table
        .lookup(model)
        .map(|rates| compute_cost(input, output, rates))
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
            "total_cost_usd": 0.0001,
            "duration_ms": 1234
        }"#;
        let (reply, report) = parse_claude_json(raw, 9999);
        assert_eq!(reply, "hello world");
        let r = report.expect("must have report");
        assert_eq!(r.provider, "claude");
        assert_eq!(r.model, "claude-sonnet-4-6");
        assert_eq!(r.input_tokens, 8);
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.cost_usd, Some(0.0001));
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
    fn parse_claude_json_fallback_when_no_total_cost() {
        // total_cost_usd 缺 → 走 pricing 表算
        let raw = r#"{
            "result": "hi",
            "model": "claude-sonnet-4-6",
            "session_id": "s",
            "usage": {"input_tokens": 1000000, "output_tokens": 0}
        }"#;
        let (_, report) = parse_claude_json(raw, 200);
        let r = report.unwrap();
        // 1M input @ $3/M = $3.00 (sonnet-4-6)
        assert!((r.cost_usd.unwrap() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn parse_claude_json_unknown_model_no_cost() {
        // model 不在 pricing 表 → cost = None, confidence = Low
        let raw = r#"{
            "result": "hi",
            "model": "claude-future-9",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let (_, report) = parse_claude_json(raw, 100);
        let r = report.unwrap();
        assert!(r.cost_usd.is_none());
        assert_eq!(r.confidence, Confidence::Low);
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
        // --model gpt-5.5-pro 时 hint 应进 model
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
        // 嵌套 `msg.{...}` 兼容
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
        // 末尾 agent_message 完整, 应替换 delta 累积
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
        // 只有 turn.completed 没 delta/message → reply 空, 应 fallback raw
        let raw = r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":0}}"#;
        let (reply, report) = parse_codex_jsonl(raw, 100, "");
        assert_eq!(reply, raw); // fallback
        let r = report.unwrap();
        assert_eq!(r.input_tokens, 5);
    }
}
