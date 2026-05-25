//! 高层门面 [`Memory`]: 把 store / embedder / extractor 拼成单一 API。
//!
//! # v0.11 起 (ADR-010)
//!
//! 新加 `local` 字段后:
//!
//! - **写**: 本地优先 (强一致), 远程异步 spawn (最终一致)
//! - **读**: 本地优先, 本地空 fallback 远程
//! - **配置**: `local` / `store` 各自 Optional, 至少一个必须设
//!   - 仅本地: 离线 / 单机隐私模式
//!   - 仅远程: 服务端 (sync-agent) 自身, 不需要本地
//!   - 同时: 客户端 (frank-cli memory) 默认模式

use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::embed::Embedder;
use crate::extract::FactExtractor;
use crate::local_store::{LocalRecord, LocalStore, SyncStatus};
use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
use crate::store::{EmbeddedRecord, MemoryStore};

/// 构造 [`Memory`] 用的依赖打包。
///
/// `store` / `local` 至少一个必须 Some, 否则 [`Memory::new`] 报错。
pub struct MemoryConfig {
    /// 远程存储后端 (Qdrant via sync-agent), 仅本地模式可空。
    pub store: Option<Arc<dyn MemoryStore>>,
    /// 本地主存 (LanceDB), 仅远程模式可空。v0.11 加。
    pub local: Option<Arc<dyn LocalStore>>,
    /// 文本 → 向量。
    pub embedder: Box<dyn Embedder>,
    /// 文本 → fact 列表 (用于 `add` 拆条)。
    pub extractor: Box<dyn FactExtractor>,
}

impl MemoryConfig {
    /// 仅远程模式 — sync-agent 服务端用; frank-cli 走 [`Self::with_local`] 加本地。
    ///
    /// 向后兼容: v0.10 之前的调用方 (sync-agent) 不用改, 自动是仅远程模式。
    pub fn new(
        store: Box<dyn MemoryStore>,
        embedder: Box<dyn Embedder>,
        extractor: Box<dyn FactExtractor>,
    ) -> Self {
        Self {
            store: Some(Arc::from(store)),
            local: None,
            embedder,
            extractor,
        }
    }

    /// 仅本地模式 (离线 / 单机隐私) — 不连任何远程。
    pub fn local_only(
        local: Arc<dyn LocalStore>,
        embedder: Box<dyn Embedder>,
        extractor: Box<dyn FactExtractor>,
    ) -> Self {
        Self {
            store: None,
            local: Some(local),
            embedder,
            extractor,
        }
    }

    /// Builder: 给 [`Self::new`] 加上本地主存, 进入双写模式。
    #[must_use]
    pub fn with_local(mut self, local: Arc<dyn LocalStore>) -> Self {
        self.local = Some(local);
        self
    }
}

/// 高层记忆 API。线程安全 (内部 trait object 都是 `Send + Sync`)。
pub struct Memory {
    config: MemoryConfig,
}

impl Memory {
    /// 初始化: 触发 store / local `ensure_initialized` (建 collection / table 等)。
    ///
    /// 若 `store` 与 `local` 都为 None, 报错 (至少要有一个)。
    pub async fn new(config: MemoryConfig) -> Result<Self> {
        if config.store.is_none() && config.local.is_none() {
            anyhow::bail!("MemoryConfig: 必须至少配 store 或 local 中一个");
        }
        let dim = config.embedder.dim();
        if let Some(store) = &config.store {
            store
                .ensure_initialized(dim)
                .await
                .context("init remote store")?;
        }
        if let Some(local) = &config.local {
            let dim_usize = usize::try_from(dim).context("embedder dim too large for usize")?;
            local
                .ensure_initialized(dim_usize)
                .await
                .context("init local store")?;
        }
        Ok(Self { config })
    }

    /// 把 `content` 抽取成多条事实, 每条 embed 后存入 store。
    ///
    /// 返回每条 fact 的 ID (按 extractor 输出顺序)。
    ///
    /// # 写策略 (v0.11 起)
    ///
    /// 1. 本地先写 (sync_status=Pending), 阻塞失败直接 Err
    /// 2. 若配了远程, `tokio::spawn` 异步推送, 成功后标 Synced
    /// 3. 若没配本地仅远程 (sync-agent 模式), 走老 upsert_batch
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
        let mut prepared = Vec::with_capacity(facts.len());
        for (fact, embedding) in facts.into_iter().zip(embeddings) {
            let id = MemoryId::new();
            ids.push(id);
            prepared.push((
                MemoryRecord {
                    id,
                    content: fact,
                    scope: scope.clone(),
                    metadata: metadata.clone().unwrap_or(serde_json::Value::Null),
                    created_at: now,
                    updated_at: now,
                },
                embedding.vector,
            ));
        }

        // 本地先写 (阻塞)
        if let Some(local) = &self.config.local {
            for (record, embedding) in &prepared {
                local
                    .add(LocalRecord {
                        record: record.clone(),
                        embedding: embedding.clone(),
                        sync_status: SyncStatus::Pending,
                    })
                    .await
                    .context("local add failed")?;
            }
        }

        // 远程: 有本地 → 异步推送; 无本地 → 同步等待 (老 sync-agent 模式)
        let items: Vec<EmbeddedRecord> = prepared
            .into_iter()
            .map(|(record, embedding)| EmbeddedRecord { record, embedding })
            .collect();
        if let Some(remote) = &self.config.store {
            if self.config.local.is_some() {
                // 双写模式: 远程 spawn 异步推送
                let remote = remote.clone();
                let local = self.config.local.clone();
                let ids_snapshot = ids.clone();
                tokio::spawn(async move {
                    match remote.upsert_batch(items).await {
                        Ok(()) => {
                            if let Some(local) = local {
                                if let Err(e) = local.mark_synced(&ids_snapshot).await {
                                    tracing::warn!(error=?e, "local mark_synced failed");
                                }
                            }
                        }
                        Err(e) => tracing::warn!(error=?e, "remote upsert_batch failed (留 v0.12 重试队列)"),
                    }
                });
            } else {
                // 仅远程 (sync-agent): 同步阻塞等
                remote.upsert_batch(items).await?;
            }
        }

        Ok(ids)
    }

    /// 直接存一条已成型的 fact (跳过 extractor)。
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

        // 本地先 (有的话)
        if let Some(local) = &self.config.local {
            local
                .add(LocalRecord {
                    record: record.clone(),
                    embedding: embedding.vector.clone(),
                    sync_status: SyncStatus::Pending,
                })
                .await
                .context("local add_raw failed")?;
        }

        // 远程同步 / 异步根据是否双写
        if let Some(remote) = &self.config.store {
            let item = EmbeddedRecord {
                record: record.clone(),
                embedding: embedding.vector,
            };
            if self.config.local.is_some() {
                let remote = remote.clone();
                let local = self.config.local.clone();
                tokio::spawn(async move {
                    if let Err(e) = remote.upsert(item).await {
                        tracing::warn!(error=?e, "remote upsert failed");
                    } else if let Some(local) = local {
                        if let Err(e) = local.mark_synced(&[id]).await {
                            tracing::warn!(error=?e, "local mark_synced failed");
                        }
                    }
                });
            } else {
                remote.upsert(item).await?;
            }
        }

        Ok(id)
    }

    /// 按 query 检索相关记忆。
    ///
    /// # 读策略 (v0.11 起)
    ///
    /// 1. 本地先查 (若配了)
    /// 2. 本地命中不空 → 直接返
    /// 3. 本地空 + 远程配了 → fallback 远程
    pub async fn search(
        &self,
        query: &str,
        scope: Scope,
        opts: SearchOpts,
    ) -> Result<Vec<MemoryMatch>> {
        let embedding = self.config.embedder.embed(query).await?;

        if let Some(local) = &self.config.local {
            let local_hits = local
                .search(embedding.vector.clone(), &scope, &opts)
                .await
                .context("local search")?;
            if !local_hits.is_empty() || self.config.store.is_none() {
                return Ok(local_hits);
            }
            tracing::info!("local empty, falling back to remote");
        }

        if let Some(remote) = &self.config.store {
            return remote.search(embedding.vector, &scope, &opts).await;
        }

        // 既无 local 也无 remote — Memory::new 已挡, 这里是不可达分支但保险
        Ok(Vec::new())
    }

    /// 按 ID 取一条 (不存在返回 None)。优先本地; 本地无 fallback 远程。
    pub async fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>> {
        // 本地 LocalStore 暂无 get(id), v0.12 加; 直接走远程
        if let Some(remote) = &self.config.store {
            return remote.get(id).await;
        }
        Ok(None)
    }

    /// 按 ID 删除 — 本地 + 远程都删 (尽力 best-effort)。
    pub async fn delete(&self, id: &MemoryId) -> Result<()> {
        if let Some(local) = &self.config.local {
            if let Err(e) = local.delete(id).await {
                tracing::warn!(error=?e, "local delete failed");
            }
        }
        if let Some(remote) = &self.config.store {
            remote.delete(id).await?;
        }
        Ok(())
    }

    /// 按 scope 列出 — 优先本地。
    pub async fn list(&self, scope: Scope, limit: u64) -> Result<Vec<MemoryRecord>> {
        if let Some(local) = &self.config.local {
            let local_records = local.list(&scope, limit).await?;
            if !local_records.is_empty() || self.config.store.is_none() {
                return Ok(local_records);
            }
        }
        if let Some(remote) = &self.config.store {
            return remote.list(&scope, limit).await;
        }
        Ok(Vec::new())
    }
}
