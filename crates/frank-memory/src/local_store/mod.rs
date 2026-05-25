//! 本地主存抽象 + LanceDB 实现 (v0.11 子项 A, ADR-010)。
//!
//! # 为什么独立于 `store::MemoryStore`
//!
//! `MemoryStore` 是远程后端 (Qdrant) 抽象, 返回 [`MemoryMatch`] / [`MemoryRecord`]
//! 不含 sync 状态信息。`LocalStore` 多了 [`SyncStatus`] 字段和 pending sync 队列接口,
//! 把两个 trait 混一起会让职责膨胀 (ADR-001 "单一职责" 要求)。
//!
//! # 数据流
//!
//! ```text
//!   frank memory add ──→ Memory::add (client.rs)
//!                          │
//!                          ├── LocalStore::add (本地主存, 阻塞写, status=Pending)
//!                          │
//!                          └── tokio::spawn ──→ MemoryStore::upsert (Qdrant remote)
//!                                                  │
//!                                                  └── 成功 → LocalStore::mark_synced
//! ```
//!
//! # 读优先本地
//!
//! `Memory::search` 先打 [`LocalStore::search`]; 本地空 (新机器首次启动)
//! 才 fallback 远程 [`MemoryStore::search`] (v0.12 加 sync pull-back 后才有意义)。

pub mod lance;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};

pub use lance::LanceLocalStore;

/// 本地记录的同步状态 (跟远程 Qdrant 同步进度)。
///
/// 状态机:
/// - 写入时默认 [`SyncStatus::Pending`]
/// - 后台 sync worker 推到远程成功 → [`SyncStatus::Synced`]
/// - 推送失败超过重试上限 → [`SyncStatus::Failed`] (留 v0.12 重试策略)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncStatus {
    /// 已同步到远程 sync-agent + Qdrant。
    Synced,
    /// 本地已写, 等待后台同步。
    Pending,
    /// 后台同步失败 (网络 / 远程错误等)。
    Failed,
}

impl SyncStatus {
    /// 序列化为短字符串 (LanceDB 列存格式紧凑些)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Pending => "pending",
            Self::Failed => "failed",
        }
    }

    /// 反序列化, 未知字符串当 Pending (保守)。
    #[must_use]
    pub fn from_str_lenient(s: &str) -> Self {
        match s {
            "synced" => Self::Synced,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// 本地存储的完整记录: 业务 record + embedding 向量 + 同步状态。
///
/// Embedding 必须跟随 record 一起持久化 (LanceDB 列存), 不能像远程那样仅传 ID。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRecord {
    /// 业务记录。
    pub record: MemoryRecord,
    /// 向量 (与 `Embedder` 输出维度一致, 默认 384)。
    pub embedding: Vec<f32>,
    /// 同步状态。
    pub sync_status: SyncStatus,
}

/// 本地主存抽象 — 嵌入式向量库 (LanceDB / sqlite-vec 等)。
///
/// 所有方法都是 async (LanceDB 内部 tokio runtime), 但实现可能内部 spawn_blocking
/// 把同步 IO 移出 main runtime。
///
/// # 并发模型
///
/// - 多 reader 同时读: 安全 (LanceDB MVCC 保证读一致快照)
/// - 写: 调用方应通过 [`crate::local_store::lance::with_write_lock`]
///   或类似 [`fs2`] 文件锁串行化, 避免多 frank cli 进程同时写撞 LanceDB commit 冲突
#[async_trait]
pub trait LocalStore: Send + Sync {
    /// 初始化存储 (建表 / 校验 schema), 幂等。
    ///
    /// `vector_dim` 应跟当前 [`crate::embed::Embedder`] 维度一致 (384 默认 BGE-small)。
    /// 若已存在表的维度不匹配, 返回 Err (避免维度污染)。
    async fn ensure_initialized(&self, vector_dim: usize) -> anyhow::Result<()>;

    /// 写入一条记录。`sync_status` 由调用方传入 (一般 Pending)。
    async fn add(&self, item: LocalRecord) -> anyhow::Result<()>;

    /// 向量检索 top-K, 应用 scope 过滤。
    ///
    /// 返回的 `MemoryMatch::score` 为余弦相似度 ∈ [0, 1]。
    async fn search(
        &self,
        query_vector: Vec<f32>,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> anyhow::Result<Vec<MemoryMatch>>;

    /// 按 scope 列出 (created_at desc), `limit` 限制条数。
    async fn list(&self, scope: &Scope, limit: u64) -> anyhow::Result<Vec<MemoryRecord>>;

    /// 按 id 删, 幂等 (不存在不报错)。
    async fn delete(&self, id: &MemoryId) -> anyhow::Result<()>;

    /// 列出 sync_status=Pending 的 record, 给后台同步 worker 用。
    ///
    /// 排序: created_at asc (先来先发, 避免老记录永远等待)。
    async fn pending_sync(&self, limit: u64) -> anyhow::Result<Vec<LocalRecord>>;

    /// 把指定 id 列表标 Synced (后台同步成功时调用)。
    async fn mark_synced(&self, ids: &[MemoryId]) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SyncStatus 字符串往返。
    #[test]
    fn sync_status_str_roundtrip() {
        for status in [SyncStatus::Synced, SyncStatus::Pending, SyncStatus::Failed] {
            assert_eq!(SyncStatus::from_str_lenient(status.as_str()), status);
        }
    }

    /// 未知字符串保守地映射成 Pending。
    #[test]
    fn sync_status_unknown_is_pending() {
        assert_eq!(SyncStatus::from_str_lenient("garbage"), SyncStatus::Pending);
        assert_eq!(SyncStatus::from_str_lenient(""), SyncStatus::Pending);
    }

    /// LocalRecord serde 往返不丢字段。
    #[test]
    fn local_record_serde_roundtrip() {
        use crate::memory::{MemoryRecord, Scope};
        use chrono::Utc;

        let item = LocalRecord {
            record: MemoryRecord {
                id: MemoryId::new(),
                content: "user prefers vim".to_string(),
                scope: Scope::user("alice"),
                metadata: serde_json::json!({"source": "chat-1"}),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            embedding: vec![0.1, 0.2, 0.3],
            sync_status: SyncStatus::Pending,
        };

        let s = serde_json::to_string(&item).expect("serialize");
        let back: LocalRecord = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.record.id, item.record.id);
        assert_eq!(back.embedding.len(), 3);
        assert_eq!(back.sync_status, SyncStatus::Pending);
    }
}
