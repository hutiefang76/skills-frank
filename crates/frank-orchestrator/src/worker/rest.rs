//! 通用 REST AI provider worker。
//!
//! P0 默认实现 Anthropic Messages API (`POST /v1/messages`); 同代码框架适合 OpenAI
//! Chat Completions / 任意 Anthropic-compatible 端点 (改 `with_base_url` + `with_model`)。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::job::{Step, StepOutput};
use crate::worker::{LogLine, Worker, WorkerId};

/// 通用 REST AI provider worker。
///
/// 默认走 Anthropic `/v1/messages`。要切到 OpenAI / 兼容端,
/// 用 [`RestWorker::with_base_url`] / [`RestWorker::with_model`] 调整。
#[derive(Debug, Clone)]
pub struct RestWorker {
    id: WorkerId,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
    http: Client,
}

impl RestWorker {
    /// 用默认配置构造一个 Claude (Anthropic) worker。
    ///
    /// `id` 就是 step.provider 里要写的注册名 (例如 "claude")。
    pub fn anthropic(id: impl Into<WorkerId>, api_key: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 4096,
            http: Client::new(),
        }
    }

    /// 改 base URL (例如代理 / 自建网关)。
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// 改模型 id。
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 改 `max_tokens` (默认 4096)。
    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

#[async_trait]
impl Worker for RestWorker {
    fn id(&self) -> &WorkerId {
        &self.id
    }

    async fn health(&self) -> bool {
        // P0 简化: 只要 api_key 非空就当 healthy; 后期可改成真打一次 ping。
        !self.api_key.is_empty()
    }

    async fn run(&self, step: &Step, log_tx: mpsc::Sender<LogLine>) -> Result<StepOutput> {
        let _ = log_tx
            .send(LogLine::info(format!(
                "RestWorker[{}] -> model={} step={}",
                self.id, self.model, step.id
            )))
            .await;

        let url = format!("{}/messages", self.base_url);
        let body = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            messages: vec![Message {
                role: "user",
                content: &step.prompt,
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
            let _ = log_tx
                .send(LogLine::error(format!("Anthropic API {status}: {text}")))
                .await;
            return Err(anyhow!("Anthropic API {status}: {text}"));
        }
        let parsed: MessagesResponse = resp
            .json()
            .await
            .context("parse Anthropic /messages response")?;
        let text = parsed
            .content
            .into_iter()
            .map(|b| match b {
                ContentBlock::Text { text } => text,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let _ = log_tx
            .send(LogLine::info(format!(
                "RestWorker[{}] produced {} chars",
                self.id,
                text.len()
            )))
            .await;
        Ok(StepOutput {
            stdout: text,
            structured: serde_json::Value::Null,
        })
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
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
    fn anthropic_builder_defaults() {
        let w = RestWorker::anthropic("claude", "sk-test");
        assert_eq!(w.id.as_str(), "claude");
        assert_eq!(w.model, "claude-sonnet-4-5-20250929");
        assert_eq!(w.max_tokens, 4096);
        assert_eq!(w.base_url, "https://api.anthropic.com/v1");
    }

    #[test]
    fn builders_chain() {
        let w = RestWorker::anthropic("openai", "sk-x")
            .with_base_url("https://example.com/v1")
            .with_model("gpt-4o")
            .with_max_tokens(2048);
        assert_eq!(w.base_url, "https://example.com/v1");
        assert_eq!(w.model, "gpt-4o");
        assert_eq!(w.max_tokens, 2048);
    }

    #[tokio::test]
    async fn health_false_on_empty_key() {
        let w = RestWorker::anthropic("claude", "");
        assert!(!w.health().await);
    }

    #[tokio::test]
    async fn health_true_on_present_key() {
        let w = RestWorker::anthropic("claude", "sk-test");
        assert!(w.health().await);
    }
}
