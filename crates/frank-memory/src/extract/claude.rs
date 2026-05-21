//! Anthropic Claude 实现的 fact extractor。
//!
//! 默认用 `claude-haiku-4-5-20251001` (便宜 + 抽取质量足够), 可自定义 model id。
//!
//! API key 从构造参数 / 环境变量 `ANTHROPIC_API_KEY` 读取。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::extract::FactExtractor;

/// 默认抽取 prompt。强 schema: JSON array of strings, 每条短句。
const SYSTEM_PROMPT: &str = "\
You extract factual statements from text. Output ONLY a JSON array of short \
declarative sentences. Each sentence should be self-contained (subject + verb + object), \
present tense when possible, and capture one fact. No commentary. No nesting. \
Example output: [\"user prefers vim over emacs\", \"user's project uses Rust 1.75\"]";

/// Claude (Anthropic) 实现。
#[derive(Debug, Clone)]
pub struct ClaudeExtractor {
    api_key: String,
    base_url: String,
    model: String,
    http: Client,
    max_tokens: u32,
}

impl ClaudeExtractor {
    /// 默认 `claude-haiku-4-5-20251001` 模型构造。
    pub fn haiku(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "claude-haiku-4-5-20251001")
    }

    /// 指定模型构造。
    pub fn with_model(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            model: model.into(),
            http: Client::new(),
            max_tokens: 1024,
        }
    }

    /// 自定义 base URL (代理 / 兼容端)。
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// 调整 `max_tokens` (默认 1024)。
    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

#[async_trait]
impl FactExtractor for ClaudeExtractor {
    async fn extract(&self, content: &str) -> Result<Vec<String>> {
        let url = format!("{}/messages", self.base_url);
        let body = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: SYSTEM_PROMPT,
            messages: vec![Message {
                role: "user",
                content,
            }],
        };
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("POST /v1/messages")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic API {status}: {text}"));
        }
        let parsed: MessagesResponse = resp
            .json()
            .await
            .context("parse Anthropic /messages response")?;
        let raw_text = parsed
            .content
            .into_iter()
            .next()
            .map(|ContentBlock::Text { text }| text)
            .ok_or_else(|| anyhow!("no text block in Anthropic response"))?;

        parse_facts_json(&raw_text)
    }
}

/// 解析 LLM 返回的 JSON array (容错: 允许前后多余空白 / 代码块包裹)。
fn parse_facts_json(text: &str) -> Result<Vec<String>> {
    // 去 markdown code fence 围栏
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<Vec<String>>(cleaned)
        .with_context(|| format!("LLM did not return valid JSON array: {text}"))
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'static str,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_array() {
        let r = parse_facts_json(r#"["a", "b"]"#).unwrap();
        assert_eq!(r, vec!["a", "b"]);
    }

    #[test]
    fn parses_markdown_fenced_json() {
        let r = parse_facts_json("```json\n[\"a\"]\n```").unwrap();
        assert_eq!(r, vec!["a"]);
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_facts_json("not json").is_err());
    }

    #[test]
    fn builder_defaults_to_haiku() {
        let e = ClaudeExtractor::haiku("dummy");
        assert_eq!(e.model, "claude-haiku-4-5-20251001");
        assert_eq!(e.max_tokens, 1024);
    }
}
