//! `CallReport` — 一次 AI 调用的可观测性单行汇总。
//!
//! # 设计目的
//!
//! frank 跨进程调 claude / codex / openai-embedding 等后, 给用户**一行**简洁汇报:
//! 哪个 provider 哪个 model 用了多少 token / 多少毫秒。
//!
//! # 为什么不算 cost?
//!
//! v0.10.5 实施中用户反馈: 中转站 (proxy / 共享账号) 的价格跟官方不一样, 2026 年内
//! 官方定价也会动好几次。frank 不知道用户走哪个 endpoint, **算成本反而是误导**。
//! 用户拿到 token 数, 自己按所用 endpoint 的实际单价换算才对。简单 > 全面。
//!
//! # 用法
//!
//! ```
//! use frank_cred::report::{CallReport, CallSource};
//! use chrono::Utc;
//!
//! let r = CallReport {
//!     provider: "claude".to_string(),
//!     model: "claude-sonnet-4-6".to_string(),
//!     input_tokens: 8,
//!     output_tokens: 5,
//!     latency_ms: 120,
//!     session_id: Some("abc12345-def".to_string()),
//!     source: CallSource::SpawnedCli { bin: "claude".to_string() },
//!     timestamp: Utc::now(),
//! };
//! eprintln!("{}", r.render_oneline());
//! // [frank] claude/claude-sonnet-4-6 in=8 out=5 120ms sid=abc12345
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 单次 AI 调用的全要素观测记录。
///
/// 字段全 public — 这是 data-bag, 用户用 `render_oneline()` 一行输出, 或自行序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallReport {
    /// provider 名 (`"claude"` / `"codex"` / `"gemini"` / `"opencode"` / `"openai-embedding"`)。
    pub provider: String,
    /// model 全名 (`"claude-sonnet-4-6"` / `"gpt-5.5"` / `"text-embedding-3-small"`)。
    pub model: String,
    /// 输入 token 数。`0` = 未知/未上报。
    pub input_tokens: u64,
    /// 输出 token 数。`0` = 未知/embedding 无输出。
    pub output_tokens: u64,
    /// 调用耗时 (ms)。从 spawn 前到 wait 完成。
    pub latency_ms: u64,
    /// CLI / API 返回的会话 ID (claude 有 `session_id`, codex 有 `thread_id`)。
    pub session_id: Option<String>,
    /// 数据来源 (子进程 / 本地缓存 / 远端 Qdrant / embedding API)。
    pub source: CallSource,
    /// 调用结束时刻 (UTC)。
    pub timestamp: DateTime<Utc>,
}

/// 调用源 — 帮用户区分是 spawn cli 还是远端 vector search。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallSource {
    /// frank 自己 spawn 的本地 CLI subprocess (`claude` / `codex` / ...)。
    SpawnedCli {
        /// binary 名 (PATH 解析前的)。
        bin: String,
    },
    /// 本地缓存命中, 未走任何外部调用。
    LocalCache,
    /// 远端 Qdrant 向量库 (HTTP)。
    RemoteQdrant {
        /// 服务端 URL (e.g. `https://frank.hutiefang.com`)。
        endpoint: String,
    },
    /// embedding REST API (OpenAI 等)。
    EmbeddingApi {
        /// API endpoint (e.g. `https://api.openai.com/v1`)。
        endpoint: String,
    },
}

impl CallReport {
    /// 单行人类可读输出。格式:
    ///
    /// `[frank] <provider>/<model> in=N out=N YYms sid=ABCDEFGH`
    ///
    /// 规则:
    /// - `session_id` 取前 8 字符 (UUID 前缀, 调试够用)
    /// - `source` / `timestamp` 不渲染 (字段冗余, 给 JSON / debug 用)
    /// - **不算 cost**: 中转站价格与官方不同 + 官方定价年内会动, token 数才是用户能控的真信号
    #[must_use]
    pub fn render_oneline(&self) -> String {
        let sid_part = self
            .session_id
            .as_deref()
            .map(|s| format!(" sid={}", s.chars().take(8).collect::<String>()))
            .unwrap_or_default();
        format!(
            "[frank] {}/{} in={} out={} {}ms{}",
            self.provider,
            self.model,
            self.input_tokens,
            self.output_tokens,
            self.latency_ms,
            sid_part,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_report(sid: Option<&str>) -> CallReport {
        CallReport {
            provider: "claude".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            input_tokens: 8,
            output_tokens: 5,
            latency_ms: 120,
            session_id: sid.map(String::from),
            source: CallSource::SpawnedCli {
                bin: "claude".to_string(),
            },
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn render_with_session_id_truncated_to_8() {
        let r = mk_report(Some("abcdef0123456"));
        let s = r.render_oneline();
        assert!(s.contains("sid=abcdef01"), "sid truncate to 8, got: {s}");
        // 不含 $ — 不再渲染 cost (V2 用户反馈: 中转站价不同)
        assert!(!s.contains('$'), "cost 已删, 不该出现 $, got: {s}");
    }

    #[test]
    fn render_no_session_id_omits_sid_part() {
        let r = mk_report(None);
        let s = r.render_oneline();
        assert!(!s.contains("sid="), "no session → no sid part, got: {s}");
    }

    #[test]
    fn render_short_session_id_does_not_panic() {
        let r = mk_report(Some("zzz"));
        let s = r.render_oneline();
        assert!(s.contains("sid=zzz"), "got: {s}");
    }

    #[test]
    fn render_includes_provider_model_tokens_latency() {
        let r = mk_report(None);
        let s = r.render_oneline();
        assert!(s.starts_with("[frank] "), "got: {s}");
        assert!(s.contains("claude/claude-sonnet-4-6"), "got: {s}");
        assert!(s.contains("in=8"), "got: {s}");
        assert!(s.contains("out=5"), "got: {s}");
        assert!(s.contains("120ms"), "got: {s}");
    }

    #[test]
    fn render_zero_tokens_handles_gracefully() {
        let mut r = mk_report(None);
        r.input_tokens = 0;
        r.output_tokens = 0;
        let s = r.render_oneline();
        assert!(s.contains("in=0"));
        assert!(s.contains("out=0"));
        assert!(!s.contains('$'));
    }

    #[test]
    fn call_source_serializes_with_tag() {
        let s = CallSource::SpawnedCli {
            bin: "claude".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""type":"spawned_cli""#));
        assert!(json.contains(r#""bin":"claude""#));
    }
}
