//! 高层门面 [`Memory`]: 把 store / embedder / extractor 拼成单一 API。
//!
//! 典型用法:
//! ```rust,ignore
//! use frank_memory::*;
//!
//! # async fn demo() -> anyhow::Result<()> {
//! let cfg = MemoryConfig::new(
//!     Box::new(QdrantStore::from_url("http://localhost:6334", "frank_memories")?),
//!     Box::new(OpenAIEmbedder::small(std::env::var("OPENAI_API_KEY")?)),
//!     Box::new(ClaudeExtractor::haiku(std::env::var("ANTHROPIC_API_KEY")?)),
//! );
//! let mem = Memory::new(cfg).await?;
//!
//! mem.add("user prefers vim over emacs", Scope::user("alice"), None).await?;
//! let hits = mem.search("what editor does alice use?", Scope::user("alice"), SearchOpts::default()).await?;
//! for hit in hits { println!("{} (score {})", hit.record.content, hit.score); }
//! # Ok(()) }
//! ```

use anyhow::Result;
use chrono::Utc;

use crate::embed::Embedder;
use crate::extract::FactExtractor;
use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
use crate::store::{EmbeddedRecord, MemoryStore};

/// 构造 [`Memory`] 用的依赖打包。
///
/// 三个 trait object 必须同时给; 测试可换成 mock 实现, 生产是 Qdrant + OpenAI + Claude。
pub struct MemoryConfig {
    /// 存储后端 (实际生产 = Qdrant)。
    pub store: Box<dyn MemoryStore>,
    /// 文本 → 向量。
    pub embedder: Box<dyn Embedder>,
    /// 文本 → fact 列表 (用于 `add` 拆条)。
    pub extractor: Box<dyn FactExtractor>,
}

impl MemoryConfig {
    /// 简单构造 (三个组件按顺序)。
    pub fn new(
        store: Box<dyn MemoryStore>,
        embedder: Box<dyn Embedder>,
        extractor: Box<dyn FactExtractor>,
    ) -> Self {
        Self {
            store,
            embedder,
            extractor,
        }
    }
}

/// 高层记忆 API。线程安全 (内部 trait object 都是 `Send + Sync`)。
pub struct Memory {
    config: MemoryConfig,
}

impl Memory {
    /// 初始化: 触发 store `ensure_initialized` (建 collection 等)。
    pub async fn new(config: MemoryConfig) -> Result<Self> {
        config
            .store
            .ensure_initialized(config.embedder.dim())
            .await?;
        Ok(Self { config })
    }

    /// 把 `content` 抽取成多条事实, 每条 embed 后存入 store。
    ///
    /// 返回每条 fact 的 ID (按 extractor 输出顺序)。
    pub async fn add(
        &self,
        content: &str,
        scope: Scope,
        metadata: Option<serde_json::Value>,
    ) -> Result<Vec<MemoryId>> {
        let facts = self.config.extractor.extract(content).await?;
        if facts.is_empty() {
            tracing::debug!(content, "extractor returned no facts");
            return Ok(Vec::new());
        }

        let embeddings = self.config.embedder.embed_batch(facts.clone()).await?;
        debug_assert_eq!(embeddings.len(), facts.len());

        let now = Utc::now();
        let mut ids = Vec::with_capacity(facts.len());
        let mut items = Vec::with_capacity(facts.len());
        for (fact, embedding) in facts.into_iter().zip(embeddings) {
            let id = MemoryId::new();
            ids.push(id);
            items.push(EmbeddedRecord {
                record: MemoryRecord {
                    id,
                    content: fact,
                    scope: scope.clone(),
                    metadata: metadata.clone().unwrap_or(serde_json::Value::Null),
                    created_at: now,
                    updated_at: now,
                },
                embedding: embedding.vector,
            });
        }

        self.config.store.upsert_batch(items).await?;
        Ok(ids)
    }

    /// 直接存一条已成型的 fact (跳过 extractor)。常用于"我就知道这是一条事实, 别让 LLM 改"。
    pub async fn add_raw(
        &self,
        fact: &str,
        scope: Scope,
        metadata: Option<serde_json::Value>,
    ) -> Result<MemoryId> {
        let embedding = self.config.embedder.embed(fact).await?;
        let now = Utc::now();
        let id = MemoryId::new();
        let record = MemoryRecord {
            id,
            content: fact.to_string(),
            scope,
            metadata: metadata.unwrap_or(serde_json::Value::Null),
            created_at: now,
            updated_at: now,
        };
        self.config
            .store
            .upsert(EmbeddedRecord {
                record,
                embedding: embedding.vector,
            })
            .await?;
        Ok(id)
    }

    /// 按 query 检索相关记忆。
    pub async fn search(
        &self,
        query: &str,
        scope: Scope,
        opts: SearchOpts,
    ) -> Result<Vec<MemoryMatch>> {
        let embedding = self.config.embedder.embed(query).await?;
        self.config
            .store
            .search(embedding.vector, &scope, &opts)
            .await
    }

    /// 按 ID 取一条 (不存在返回 None)。
    pub async fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>> {
        self.config.store.get(id).await
    }

    /// 按 ID 删除。
    pub async fn delete(&self, id: &MemoryId) -> Result<()> {
        self.config.store.delete(id).await
    }

    /// 按 scope 列出 (按 store 自然顺序; v1 不强制排序)。
    pub async fn list(&self, scope: Scope, limit: u64) -> Result<Vec<MemoryRecord>> {
        self.config.store.list(&scope, limit).await
    }
}
