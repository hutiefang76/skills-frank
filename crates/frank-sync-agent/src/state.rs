//! Axum 共享 [`AppState`] 容器。
//!
//! 持有所有"长寿命依赖" (Qdrant 客户端 / Memory 门面 / 后续 orchestrator 调度器)。
//! `Clone` 后所有副本共享同一份 `Arc` 内部, 安全无成本。

use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use frank_memory::embed::openai::OpenAIEmbedder;
use frank_memory::extract::claude::ClaudeExtractor;
use frank_memory::store::qdrant::QdrantStore;
use frank_memory::{Embedder, FactExtractor, Memory, MemoryConfig, MemoryStore};

/// 服务级共享状态。
#[derive(Clone)]
pub struct AppState {
    /// 高层记忆门面。
    pub memory: Arc<Memory>,
}

impl AppState {
    /// 从环境变量构造。失败时报清晰错误 (启动期, 早 fail 比晚 panic 强)。
    ///
    /// # 环境变量
    /// - `FRANK_QDRANT_URL` (默认 `http://qdrant:6334`)
    /// - `FRANK_COLLECTION` (默认 `frank_memories_v1`)
    /// - `FRANK_MEMORY_MOCK` (= `"1"` 时启用 mock embedder/extractor, 不需要外部 API key)
    /// - `OPENAI_API_KEY` (mock 关闭时必填) — `OpenAIEmbedder`
    /// - `ANTHROPIC_API_KEY` (mock 关闭时必填) — `ClaudeExtractor`
    pub async fn from_env() -> Result<Self> {
        let qdrant_url =
            env::var("FRANK_QDRANT_URL").unwrap_or_else(|_| "http://qdrant:6334".to_string());
        let collection =
            env::var("FRANK_COLLECTION").unwrap_or_else(|_| "frank_memories_v1".to_string());

        let use_mock = matches!(env::var("FRANK_MEMORY_MOCK").as_deref(), Ok("1"));

        let store: Box<dyn MemoryStore> = Box::new(
            QdrantStore::from_url(&qdrant_url, collection)
                .with_context(|| format!("connect Qdrant at {qdrant_url}"))?,
        );

        let (embedder, extractor): (Box<dyn Embedder>, Box<dyn FactExtractor>) = if use_mock {
            tracing::warn!(
                "FRANK_MEMORY_MOCK=1: using zero-vector embedder + line-split extractor; \
                 NOT for production"
            );
            (
                Box::new(crate::mock::MockEmbedder),
                Box::new(crate::mock::MockExtractor),
            )
        } else {
            let openai_key = env::var("OPENAI_API_KEY").context("env OPENAI_API_KEY required")?;
            let anthropic_key =
                env::var("ANTHROPIC_API_KEY").context("env ANTHROPIC_API_KEY required")?;
            (
                Box::new(OpenAIEmbedder::small(openai_key)),
                Box::new(ClaudeExtractor::haiku(anthropic_key)),
            )
        };

        let cfg = MemoryConfig::new(store, embedder, extractor);
        let memory = Memory::new(cfg).await.context("init Memory")?;

        Ok(Self {
            memory: Arc::new(memory),
        })
    }
}
