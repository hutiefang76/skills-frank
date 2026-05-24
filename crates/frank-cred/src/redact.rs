//! Token redaction — 屏蔽 stdout/stderr/logs 中可能出现的 secret。
//!
//! V2 修 codex Plan Review dim_5: redaction 全链覆盖, 不只终端 writer。
//! 集成点 (调用方负责):
//! - frank-cli `log::ui::*` 输出包 [`RedactWriter`]
//! - tracing-subscriber fmt layer 替换 stderr writer 为 [`RedactWriter`]
//! - frank spawn child 时, child stdout/stderr 通过 [`redact_secrets`] 过滤
//! - `LocalCliWorker.StepOutput` (orchestrator 集成点, codex M2) — 写 StepOutput 前 redact

use std::io::{self, Write};
use std::sync::OnceLock;

use regex::Regex;

/// 编译一次共享的正则集合。匹配主流 token 前缀:
/// - `sk-ant-...` (Anthropic API key)
/// - `sk-proj-...` / `sk-...` (OpenAI / 兼容)
/// - `gho_...` (GitHub OAuth)
/// - `ghs_...` (GitHub server-to-server)
/// - `gemini_...` / 长 base64-like 串 (40+ 字符)
fn redactors() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        vec![
            // sk-ant-... (Anthropic), 至少 30 字符
            Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").expect("regex 编译"),
            // sk-proj-..., sk-... (OpenAI 及兼容)
            Regex::new(r"sk-(?:proj-)?[A-Za-z0-9_\-]{20,}").expect("regex 编译"),
            // gho_..., ghs_..., ghp_... (GitHub tokens)
            Regex::new(r"gh[opus]_[A-Za-z0-9]{36,}").expect("regex 编译"),
            // 通用: 40+ 字符 base64-like 长串 (兜底, 可能误报)
            Regex::new(r"\b[A-Za-z0-9_\-]{40,}\b").expect("regex 编译"),
        ]
    })
}

/// 屏蔽字符串中所有匹配 token 前缀的子串, 保留前 6 字符 + `***...{last4}`。
///
/// 例: `sk-ant-api03-abcdefghijklmnopqrstuvwxyz` → `sk-ant***...wxyz`
///
/// 调用频率不高 (主要在错误路径 / doctor 输出), 编译过的正则集合 lazy init。
#[must_use]
pub fn redact_secrets(s: &str) -> String {
    let mut out = s.to_string();
    for re in redactors() {
        out = re
            .replace_all(&out, |caps: &regex::Captures<'_>| {
                let matched = &caps[0];
                if matched.len() <= 10 {
                    "***".to_string()
                } else {
                    let prefix: String = matched.chars().take(6).collect();
                    let suffix: String = matched
                        .chars()
                        .rev()
                        .take(4)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    format!("{prefix}***...{suffix}")
                }
            })
            .into_owned();
    }
    out
}

/// 包装任意 [`Write`] 的 writer, 写入前自动 [`redact_secrets`]。
///
/// 用于 tracing fmt layer / stderr / StepOutput pipe / frank-cli ui 输出。
pub struct RedactWriter<W: Write> {
    inner: W,
}

impl<W: Write> RedactWriter<W> {
    /// 包装内部 writer。
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// 取回内部 writer (consume self)。
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for RedactWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 仅当 buf 是有效 UTF-8 时 redact, 否则直透 (binary 不改)。
        if let Ok(s) = std::str::from_utf8(buf) {
            let redacted = redact_secrets(s);
            self.inner.write_all(redacted.as_bytes())?;
            Ok(buf.len())
        } else {
            self.inner.write(buf)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_anthropic_key() {
        let s = "got token sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890 here";
        let r = redact_secrets(s);
        assert!(r.starts_with("got token sk-ant***"));
        assert!(r.ends_with("here"));
        assert!(!r.contains("abcdefghi"), "原 token 不应出现: {r}");
    }

    #[test]
    fn redact_openai_proj_key() {
        let s = "key sk-proj-aaaaaaaaaaaaaaaaaaaaaaaaaaaa end";
        let r = redact_secrets(s);
        assert!(r.contains("sk-pro***"), "got: {r}");
        assert!(!r.contains("aaaaaaaaa"), "got: {r}");
    }

    #[test]
    fn redact_github_token() {
        let s = "gho_abcdefghijklmnopqrstuvwxyz0123456789ABCD next";
        let r = redact_secrets(s);
        assert!(!r.contains("0123456789ABCD"), "got: {r}");
    }

    #[test]
    fn redact_writer_passes_through_plain_text() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = RedactWriter::new(&mut buf);
            w.write_all(b"hello world").unwrap();
        }
        assert_eq!(buf, b"hello world");
    }

    #[test]
    fn redact_writer_masks_secret() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = RedactWriter::new(&mut buf);
            w.write_all(b"token=sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890")
                .unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("sk-ant***"), "got: {s}");
        assert!(!s.contains("abcdefghij"));
    }

    #[test]
    fn short_string_not_falsely_redacted() {
        let s = "hello";
        assert_eq!(redact_secrets(s), "hello");
    }
}
