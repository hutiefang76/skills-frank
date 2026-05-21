//! OpenAI embedding 实现 (`text-embedding-3-small`)。
//!
//! 模型: `text-embedding-3-small`
//! - 维度: 1536
//! - 价格: 0.02 USD / 1M tokens (2026-05 时价)
//! - 中英文均可
//!
//! API key 从构造参数 / 环境变量 `OPENAI_API_KEY` 读取。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::embed::{Embedder, Embedding};

/// OpenAI embedding 客户端。
#[derive(Debug, Clone)]
pub struct OpenAIEmbedder {
    api_key: String,
    base_url: String,
    model: String,
    dim: u64,
    http: Client,
}

impl OpenAIEmbedder {
    /// 默认模型 `text-embedding-3-small` 构造 (1536 维)。
    pub fn small(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "text-embedding-3-small", 1536)
    }

    /// 指定模型 + 维度构造。`base_url` 走默认 `https://api.openai.com/v1`。
    pub fn with_model(api_key: impl Into<String>, model: impl Into<String>, dim: u64) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: model.into(),
            dim,
            http: Client::new(),
        }
    }

    /// 自定义 base URL (例如走代理 / Azure OpenAI 兼容端)。
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    async fn call(&self, inputs: Vec<String>) -> Result<EmbeddingResponse> {
        let url = format!("{}/embeddings", self.base_url);
        let body = EmbeddingRequest {
            model: &self.model,
            input: inputs,
        };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("POST /embeddings")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI embeddings API {status}: {text}"));
        }
        resp.json::<EmbeddingResponse>()
            .await
            .context("parse OpenAI embeddings response")
    }
}

#[async_trait]
impl Embedder for OpenAIEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        let mut resp = self.call(vec![text.to_string()]).await?;
        let item = resp
            .data
            .pop()
            .ok_or_else(|| anyhow!("empty embedding response"))?;
        Ok(Embedding {
            vector: item.embedding,
            model: self.model.clone(),
        })
    }

    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.call(texts).await?;
        Ok(resp
            .data
            .into_iter()
            .map(|d| Embedding {
                vector: d.embedding,
                model: self.model.clone(),
            })
            .collect())
    }

    fn dim(&self) -> u64 {
        self.dim
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults_to_text_embedding_3_small() {
        let e = OpenAIEmbedder::small("dummy");
        assert_eq!(e.model(), "text-embedding-3-small");
        assert_eq!(e.dim(), 1536);
    }

    #[test]
    fn with_base_url_overrides() {
        let e = OpenAIEmbedder::small("dummy").with_base_url("https://proxy.example/v1");
        assert_eq!(e.base_url, "https://proxy.example/v1");
    }
}
