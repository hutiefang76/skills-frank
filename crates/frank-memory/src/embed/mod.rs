//! Embedding 抽象层。
//!
//! 给上层提供"文本 → dense vector"的统一接口, 解耦具体 embedding provider。
//!
//! 实现:
//! - [`openai::OpenAIEmbedder`] — OpenAI `text-embedding-3-small` (1536 dim, 便宜)
//! - 预留: 本地 fastembed / Cohere 等

use async_trait::async_trait;

pub mod openai;

/// 一次 embedding 输出: dense float vector + 模型/维度元数据。
#[derive(Debug, Clone)]
pub struct Embedding {
    /// 向量数据 (维度由 [`Embedder::dim`] 报告)。
    pub vector: Vec<f32>,
    /// 产生此向量的模型标识 (例如 `text-embedding-3-small`)。
    pub model: String,
}

/// 文本 → 向量的统一接口。
#[async_trait]
pub trait Embedder: Send + Sync {
    /// 单条文本 embed。
    async fn embed(&self, text: &str) -> anyhow::Result<Embedding>;

    /// 批量 embed (实现可以走 provider 的批接口, 不会比循环慢)。
    async fn embed_batch(&self, texts: Vec<String>) -> anyhow::Result<Vec<Embedding>> {
        // 默认实现: 串行调 embed。Provider 应该覆盖以走真正的批接口。
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(&t).await?);
        }
        Ok(out)
    }

    /// 向量维度 (与 [`crate::store::MemoryStore::ensure_initialized`] 必须匹配)。
    fn dim(&self) -> u64;

    /// 模型标识 (便于审计 / collection 命名带版本)。
    fn model(&self) -> &str;
}
