//! `frank memory <op>` 的 CallReport 构造 — 客户端近似 token 估算。
//!
//! # 限制 (与 frank ai ask 的根本区别)
//!
//! `frank-cli` 通过 HTTP 调 `frank-sync-agent`, embedding HTTP 实调发生在**服务端**,
//! 客户端拿不到 OpenAI 真实的 token 计数。两个选项:
//!
//! 1. 改 wire protocol 让 sync-agent 在响应里带 token usage — 跨 crate 大改, 留 Phase 3.
//! 2. **本文件**: 客户端按 char/4 估算 input_tokens (mem0 / tiktoken 经验比例),
//!    `Confidence::Low` 显式标"估算", `cost_usd = None` 不误导用户。
//!
//! 选 2 — 用户立刻看到 latency + 大致 token, 真要精确等 Phase 3 服务端透出。

use std::time::Instant;

use frank_cred::report::{CallReport, CallSource, Confidence};

/// 近似 token 估算: 1 token ≈ 4 chars (mem0 / OpenAI 经验比例)。
///
/// 仅对英文 / ASCII 比较准, 中文偏低估; 用户会看到 `Confidence::Low` 提示。
#[must_use]
pub fn approx_tokens_from_chars(text: &str) -> u64 {
    // chars() 而非 bytes(): 中文 1 字符 ≈ 3 byte; 用字符数除 4 整体偏低估但更稳。
    let chars = text.chars().count() as u64;
    chars.div_ceil(4).max(1)
}

/// 构造一条 frank-memory 调用的 CallReport (输入文本 + 远端 sync-agent endpoint)。
///
/// - `op_name`: `"search"` / `"add"` / `"list"` ... — 当前仅用于内部 trace, 不渲染
/// - `input_text`: 用户查询 / 添加的文本 (用来估 token)
/// - `endpoint`: sync-agent base URL (会进 `CallSource::RemoteQdrant`)
/// - `latency_ms`: 整个 HTTP 调用耗时
/// - `output_tokens`: 通常 0 (memory 查/写没"输出"; 服务端可能 internal embed 有, 但
///   客户端看不到)
#[must_use]
pub fn build_memory_report(
    op_name: &str,
    input_text: &str,
    endpoint: &str,
    latency_ms: u64,
    output_tokens: u64,
) -> CallReport {
    tracing::debug!(op = op_name, "build_memory_report");
    let input_tokens = approx_tokens_from_chars(input_text);
    CallReport {
        provider: "frank-memory".to_string(),
        model: "text-embedding-3-small".to_string(),
        input_tokens,
        output_tokens,
        cost_usd: None, // 客户端无服务端真实 usage, 不算 cost 避免误导
        latency_ms,
        session_id: None,
        source: CallSource::RemoteQdrant {
            endpoint: endpoint.to_string(),
        },
        timestamp: chrono::Utc::now(),
        confidence: Confidence::Low, // chars/4 估算 + 无 cost
    }
}

/// 一行 stderr 打印 helper — 跟 `[frank]` 行格式一致。
pub fn eprint_memory_report(report: &CallReport) {
    eprintln!("{}", report.render_oneline());
}

/// 计时器: 在调 client 前调 `start()`, 调完后 `elapsed_ms()` 取耗时。
pub struct Stopwatch {
    started: Instant,
}

impl Stopwatch {
    /// 启动计时。
    #[must_use]
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    /// 当前耗时 (毫秒)。
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_tokens_minimum_one() {
        // 极短文本至少 1 token
        assert_eq!(approx_tokens_from_chars("a"), 1);
        assert_eq!(approx_tokens_from_chars(""), 1);
    }

    #[test]
    fn approx_tokens_ascii_div_four() {
        // 16 chars → 4 tokens
        assert_eq!(approx_tokens_from_chars("0123456789abcdef"), 4);
        // 17 chars → 5 tokens (ceil)
        assert_eq!(approx_tokens_from_chars("0123456789abcdefg"), 5);
    }

    #[test]
    fn approx_tokens_chinese_counts_chars() {
        // 4 中文字符 → 1 token (chars count, 不是 bytes)
        assert_eq!(approx_tokens_from_chars("你好世界"), 1);
        // 5 中文字符 → 2 tokens (ceil 5/4)
        assert_eq!(approx_tokens_from_chars("你好世界呢"), 2);
    }

    #[test]
    fn build_report_uses_low_confidence_no_cost() {
        let r = build_memory_report("search", "test query", "https://x.test", 100, 0);
        assert_eq!(r.provider, "frank-memory");
        assert_eq!(r.model, "text-embedding-3-small");
        assert_eq!(r.confidence, Confidence::Low);
        assert!(r.cost_usd.is_none());
        assert_eq!(r.latency_ms, 100);
        // chars=10 → 3 tokens (ceil)
        assert_eq!(r.input_tokens, 3);
    }

    #[test]
    fn build_report_render_contains_endpoint_source() {
        let r = build_memory_report("add", "hi", "https://frank.example.com", 50, 0);
        let serialized = serde_json::to_string(&r.source).unwrap();
        assert!(serialized.contains("frank.example.com"));
    }

    #[test]
    fn stopwatch_elapsed_is_nonnegative() {
        let sw = Stopwatch::start();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(sw.elapsed_ms() >= 2);
    }
}
