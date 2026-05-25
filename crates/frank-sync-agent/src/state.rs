//! Axum 共享 [`AppState`] 容器。
//!
//! 持有所有"长寿命依赖" (Qdrant 客户端 / Memory 门面 / 后续 orchestrator 调度器)。
//! `Clone` 后所有副本共享同一份 `Arc` 内部, 安全无成本。

use std::env;
use std::sync::Arc;

use crate::local_embedder::LocalEmbedder;
use crate::tenant::TenantStore;
use anyhow::{Context, Result};
use frank_memory::embed::openai::OpenAIEmbedder;
use frank_memory::extract::claude::ClaudeExtractor;
use frank_memory::store::qdrant::QdrantStore;
use frank_memory::{Embedder, FactExtractor, Memory, MemoryConfig, MemoryStore};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// 服务级共享状态。
#[derive(Clone)]
pub struct AppState {
    /// 高层记忆门面 (frank-memory)。
    pub memory: Arc<Memory>,
    /// 跨设备 skills state 同步: 每个 device_id 一份 state.json 透传 JSON。
    /// v0.4 内存版重启丢; v0.5 SQLite 持久化.
    pub skills_sync: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// v0.12.0: tenant registry + quota + deletion schedule (SQLite 持久化).
    pub tenants: Arc<TenantStore>,
    /// v0.12.0: per-tenant 配额上限 (default 10000 records). FRANK_QUOTA_PER_TENANT env.
    pub quota_per_tenant: i64,
    /// v0.12.0: 删除申请等待期 (default 14 day). FRANK_DELETION_WAIT_DAYS env.
    pub deletion_wait_days: i64,
    /// v0.12.0: qdrant 直 client (retention worker 真删 points 用).
    pub qdrant_url: String,
    /// v0.12.0: qdrant collection 名 (用于 retention delete by user_id filter).
    pub qdrant_collection: String,
}

impl AppState {
    /// 从环境变量构造。失败时报清晰错误 (启动期, 早 fail 比晚 panic 强)。
    ///
    /// # 环境变量
    /// - `FRANK_QDRANT_URL` (默认 `http://qdrant:6334`)
    /// - `FRANK_COLLECTION` (默认 `frank_memories_v1`)
    /// - `FRANK_MEMORY_MOCK` (= `"1"` 强制 zero-vector mock, 仅用于无网络冒烟)
    /// - `OPENAI_API_KEY` (可选) — `OpenAIEmbedder` (1536 维, 质量比 fastembed 好但要 token)
    /// - `ANTHROPIC_API_KEY` (可选) — `ClaudeExtractor` 服务端抽 fact
    ///
    /// # 默认: 零 token (LocalEmbedder + mock 行拆)
    ///
    /// 没 `OPENAI_API_KEY` 时**默认走 LocalEmbedder** (fastembed BAAI/bge-small, 384 维,
    /// 本地 ONNX, 0 token). 用户原问"分布式记忆需要 token 么"的答案: **不需要,默认就 0**.
    /// 服务端只 embed, 抽 fact 走客户端 (frank-cli 用 frank-bridge 调用户订阅) — M3 todo.
    pub async fn from_env() -> Result<Self> {
        let qdrant_url =
            env::var("FRANK_QDRANT_URL").unwrap_or_else(|_| "http://qdrant:6334".to_string());
        let collection =
            env::var("FRANK_COLLECTION").unwrap_or_else(|_| "frank_memories_v1".to_string());

        let force_mock = matches!(env::var("FRANK_MEMORY_MOCK").as_deref(), Ok("1"));

        let store: Box<dyn MemoryStore> = Box::new(
            QdrantStore::from_url(&qdrant_url, collection.clone())
                .with_context(|| format!("connect Qdrant at {qdrant_url}"))?,
        );

        let (embedder, extractor): (Box<dyn Embedder>, Box<dyn FactExtractor>) = if force_mock {
            tracing::warn!(
                "FRANK_MEMORY_MOCK=1: zero-vector embedder + line-split extractor; \
                 ONLY for offline smoke"
            );
            (
                Box::new(crate::mock::MockEmbedder),
                Box::new(crate::mock::MockExtractor),
            )
        } else {
            // 默认零 token 路径: LocalEmbedder (fastembed 384 维) + mock 行拆 extractor
            let embedder: Box<dyn Embedder> = match env::var("OPENAI_API_KEY") {
                Ok(key) if !key.trim().is_empty() => {
                    tracing::info!("OPENAI_API_KEY set: using OpenAIEmbedder (1536d)");
                    Box::new(OpenAIEmbedder::small(key))
                }
                _ => {
                    // v0.8.1: FRANK_LOCAL_MODEL_DIR=/path/to/snapshot 走 offline 模式
                    // (国内服务器连不上 huggingface.co, 走预下模型). 不设走默认 HF 下载.
                    if let Ok(dir) = env::var("FRANK_LOCAL_MODEL_DIR") {
                        if dir.trim().is_empty() {
                            tracing::info!(
                                "no OPENAI_API_KEY, no FRANK_LOCAL_MODEL_DIR: \
                                 using LocalEmbedder (fastembed BAAI/bge-small 384d, will download from HF)"
                            );
                            Box::new(LocalEmbedder::small().context("init LocalEmbedder")?)
                        } else {
                            tracing::info!(
                                "FRANK_LOCAL_MODEL_DIR={dir}: using offline LocalEmbedder \
                                 (BAAI/bge-small 384d, 0 token, no network)"
                            );
                            Box::new(
                                LocalEmbedder::from_files(&dir)
                                    .context("init offline LocalEmbedder from files")?,
                            )
                        }
                    } else {
                        tracing::info!(
                            "no OPENAI_API_KEY: using LocalEmbedder (fastembed BAAI/bge-small 384d, 0 token)"
                        );
                        Box::new(LocalEmbedder::small().context("init LocalEmbedder")?)
                    }
                }
            };
            let extractor: Box<dyn FactExtractor> = match env::var("ANTHROPIC_API_KEY") {
                Ok(key) if !key.trim().is_empty() => {
                    tracing::info!("ANTHROPIC_API_KEY set: using ClaudeExtractor (Haiku)");
                    Box::new(ClaudeExtractor::haiku(key))
                }
                _ => {
                    tracing::info!(
                        "no ANTHROPIC_API_KEY: using mock extractor (split by \\n); \
                         M3 todo: frank-cli 客户端 frank-bridge 抽 fact"
                    );
                    Box::new(crate::mock::MockExtractor)
                }
            };
            (embedder, extractor)
        };

        let cfg = MemoryConfig::new(store, embedder, extractor);
        let memory = Memory::new(cfg).await.context("init Memory")?;

        // v0.12.0: 初始化 tenant SQLite. 默认 /var/lib/frank/tenants.db (容器 volume).
        let tenant_db = env::var("FRANK_TENANT_DB")
            .unwrap_or_else(|_| "/var/lib/frank/tenants.db".to_string());
        tracing::info!(%tenant_db, "init tenant SQLite");
        let tenants = TenantStore::open(&tenant_db)
            .await
            .with_context(|| format!("open tenant store {tenant_db}"))?;

        let quota_per_tenant: i64 = env::var("FRANK_QUOTA_PER_TENANT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);
        let deletion_wait_days: i64 = env::var("FRANK_DELETION_WAIT_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14);
        tracing::info!(quota_per_tenant, deletion_wait_days, "tenant policy");

        Ok(Self {
            memory: Arc::new(memory),
            skills_sync: Arc::new(RwLock::new(HashMap::new())),
            tenants: Arc::new(tenants),
            quota_per_tenant,
            deletion_wait_days,
            qdrant_url,
            qdrant_collection: collection,
        })
    }
}
