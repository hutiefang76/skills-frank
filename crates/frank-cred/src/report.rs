//! `CallReport` — 一次 AI 调用的可观测性单行汇总。
//!
//! # 设计目的
//!
//! frank 跨进程调 claude / codex / openai-embedding 等后, 需要给用户**一行**
//! 简洁汇报: 哪个 provider 哪个 model 用了多少 token / 花了多少钱 / 多少毫秒。
//!
//! # 用法
//!
//! ```
//! use frank_cred::report::{CallReport, CallSource, Confidence};
//! use chrono::Utc;
//!
//! let r = CallReport {
//!     provider: "claude".to_string(),
//!     model: "claude-sonnet-4-6".to_string(),
//!     input_tokens: 8,
//!     output_tokens: 5,
//!     cost_usd: Some(0.0001),
//!     latency_ms: 120,
//!     session_id: Some("abc12345-def".to_string()),
//!     source: CallSource::SpawnedCli { bin: "claude".to_string() },
//!     timestamp: Utc::now(),
//!     confidence: Confidence::High,
//! };
//! eprintln!("{}", r.render_oneline());
//! // [frank] claude/claude-sonnet-4-6 in=8 out=5 $0.0001 120ms sid=abc12345
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
    /// USD 成本。`None` = 模型未在 pricing 表 (区别于 `Some(0.0)` 真免费)。
    pub cost_usd: Option<f64>,
    /// 调用耗时 (ms)。从 spawn 前到 wait 完成。
    pub latency_ms: u64,
    /// CLI / API 返回的会话 ID (claude 有 `session_id`, codex 有 `thread_id`)。
    pub session_id: Option<String>,
    /// 数据来源 (子进程 / 本地缓存 / 远端 Qdrant / embedding API)。
    pub source: CallSource,
    /// 调用结束时刻 (UTC)。
    pub timestamp: DateTime<Utc>,
    /// 该条 cost 估算的可信度。
    pub confidence: Confidence,
}

/// 调用源 — 帮用户区分钱是花在 spawn cli 还是远端 vector search。
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

/// cost 估算的可信度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// 直接拿到官方 cost 或 pricing 表 `confidence: high`。
    High,
    /// 价格在 pricing 表但 `confidence: med` (e.g. secondary 来源)。
    Med,
    /// pricing 表 `confidence: low` 或走 `unknown_model_policy`。
    Low,
}

impl Confidence {
    /// 从 pricing.json 的 `confidence` 字段 (`"high"`/`"med"`/`"low"`) 解析。未知 → `Low`。
    ///
    /// 名字故意不叫 `from_str` (避免和 `std::str::FromStr` trait 撞), 用 `parse_label` 区分。
    #[must_use]
    pub fn parse_label(s: &str) -> Self {
        match s {
            "high" => Self::High,
            "med" => Self::Med,
            _ => Self::Low,
        }
    }
}

impl CallReport {
    /// 单行人类可读输出。格式:
    ///
    /// `[frank] <provider>/<model> in=N out=N $X.XXXX YYms sid=ABCDEFGH`
    ///
    /// 规则:
    /// - `cost_usd >= 0.0001` → `$X.XXXX` (4 位小数, 永不科学计数)
    /// - `0 < cost_usd < 0.0001` → `<$0.0001` (兜底, 避免显示 `$0.0000`)
    /// - `cost_usd == 0` → `$0.0000`
    /// - `cost_usd == None` → `?` (未在 pricing 表)
    /// - `session_id` → 前 8 字符 (UUID 前缀, 调试够用)
    /// - `source` 不渲染 (字段冗余, 给 JSON / debug 用)
    /// - `confidence` 不渲染 (low 信心可走 `render_with_warning` 单独提示)
    #[must_use]
    pub fn render_oneline(&self) -> String {
        let cost_str = match self.cost_usd {
            None => "?".to_string(),
            Some(c) if c < 0.0001 && c > -0.0001 => {
                if c.abs() < f64::EPSILON {
                    "$0.0000".to_string()
                } else {
                    "<$0.0001".to_string()
                }
            }
            Some(c) => format!("${c:.4}"),
        };
        let sid_part = self
            .session_id
            .as_deref()
            .map(|s| format!(" sid={}", s.chars().take(8).collect::<String>()))
            .unwrap_or_default();
        format!(
            "[frank] {}/{} in={} out={} {} {}ms{}",
            self.provider,
            self.model,
            self.input_tokens,
            self.output_tokens,
            cost_str,
            self.latency_ms,
            sid_part,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_report(cost: Option<f64>, sid: Option<&str>) -> CallReport {
        CallReport {
            provider: "claude".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            input_tokens: 8,
            output_tokens: 5,
            cost_usd: cost,
            latency_ms: 120,
            session_id: sid.map(String::from),
            source: CallSource::SpawnedCli {
                bin: "claude".to_string(),
            },
            timestamp: Utc::now(),
            confidence: Confidence::High,
        }
    }

    #[test]
    fn render_normal_cost_4_decimals() {
        let r = mk_report(Some(0.0123), Some("abcdef0123456"));
        let s = r.render_oneline();
        assert!(s.contains("$0.0123"), "got: {s}");
        assert!(s.contains("sid=abcdef01"), "sid truncate to 8, got: {s}");
    }

    #[test]
    fn render_sub_cent_falls_back_to_floor() {
        let r = mk_report(Some(0.00001), None);
        let s = r.render_oneline();
        assert!(s.contains("<$0.0001"), "got: {s}");
        assert!(!s.contains("sid="), "no session — no sid part, got: {s}");
    }

    #[test]
    fn render_exact_zero_cost() {
        let r = mk_report(Some(0.0), None);
        let s = r.render_oneline();
        assert!(s.contains("$0.0000"), "got: {s}");
    }

    #[test]
    fn render_no_cost_shows_question_mark() {
        let r = mk_report(None, None);
        let s = r.render_oneline();
        assert!(s.contains(" ? "), "got: {s}");
    }

    #[test]
    fn render_large_cost_no_scientific() {
        let r = mk_report(Some(1234.5678), Some("zzz"));
        let s = r.render_oneline();
        assert!(s.contains("$1234.5678"), "got: {s}");
        // session_id 短于 8 也别炸
        assert!(s.contains("sid=zzz"), "got: {s}");
    }

    #[test]
    fn render_includes_provider_model_tokens_latency() {
        let r = mk_report(Some(0.0001), None);
        let s = r.render_oneline();
        assert!(s.starts_with("[frank] "), "got: {s}");
        assert!(s.contains("claude/claude-sonnet-4-6"), "got: {s}");
        assert!(s.contains("in=8"), "got: {s}");
        assert!(s.contains("out=5"), "got: {s}");
        assert!(s.contains("120ms"), "got: {s}");
    }

    #[test]
    fn confidence_parse_label_handles_all_three() {
        assert_eq!(Confidence::parse_label("high"), Confidence::High);
        assert_eq!(Confidence::parse_label("med"), Confidence::Med);
        assert_eq!(Confidence::parse_label("low"), Confidence::Low);
        // 未知 → Low (safe default)
        assert_eq!(Confidence::parse_label("garbage"), Confidence::Low);
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

    #[test]
    fn render_unknown_model_with_zero_tokens() {
        let mut r = mk_report(None, None);
        r.input_tokens = 0;
        r.output_tokens = 0;
        r.model = "totally-unknown".to_string();
        let s = r.render_oneline();
        // 0 token + 未知 cost, 仍然能渲染不炸
        assert!(s.contains("in=0"));
        assert!(s.contains("out=0"));
        assert!(s.contains(" ? "));
    }
}
