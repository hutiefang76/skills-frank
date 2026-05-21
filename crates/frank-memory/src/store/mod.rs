//! 记忆存储抽象层: vector DB 后端 (Qdrant) 通过 trait 解耦。
//!
//! # 为什么有 trait
//!
//! - 测试 (用 `InMemoryStore` mock, 不强依赖 docker)
//! - 后续可换其他 vector DB (Pinecone / Chroma / pgvector) 而不动上层
//!
//! 实际生产用 Qdrant — 见 [`qdrant`] 模块。

use async_trait::async_trait;

use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};

pub mod qdrant;

/// 一条用于写入的"向量化记录": 记录本身 + 已算好的 embedding。
#[derive(Debug, Clone)]
pub struct EmbeddedRecord {
    /// 完整 record 数据。
    pub record: MemoryRecord,

    /// 与 record.content 对应的 dense vector。
    pub embedding: Vec<f32>,
}

/// vector DB 后端统一接口。
///
/// 所有方法 `async`, 实现里走 Qdrant gRPC / HTTP。
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 初始化 (建 collection, 校验维度等)。幂等。
    async fn ensure_initialized(&self, vector_dim: u64) -> anyhow::Result<()>;

    /// 插入一条已 embed 的记录。覆盖同 ID。
    async fn upsert(&self, item: EmbeddedRecord) -> anyhow::Result<()>;

    /// 批量插入 (性能更好)。
    async fn upsert_batch(&self, items: Vec<EmbeddedRecord>) -> anyhow::Result<()>;

    /// 按向量检索 top-k, 过滤 scope + metadata。
    async fn search(
        &self,
        query_vector: Vec<f32>,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> anyhow::Result<Vec<MemoryMatch>>;

    /// 按 ID 取一条; 不存在返回 `None`。
    async fn get(&self, id: &MemoryId) -> anyhow::Result<Option<MemoryRecord>>;

    /// 按 ID 删除; 幂等 (不存在不报错)。
    async fn delete(&self, id: &MemoryId) -> anyhow::Result<()>;

    /// 按 scope 列出 (最多 limit 条; 按 created_at desc 排)。
    async fn list(&self, scope: &Scope, limit: u64) -> anyhow::Result<Vec<MemoryRecord>>;
}
