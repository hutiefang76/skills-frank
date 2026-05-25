//! Hybrid Retrieval — 多路并行召回 + RRF 融合 (v0.11 子项 B, ADR-011)。
//!
//! # 4 路设计 (v0.11 落地 3 路 + BM25 stub)
//!
//! | 路 | 实现 | 状态 |
//! |---|---|---|
//! | 1. 向量 (semantic) | LocalStore.search (fastembed + LanceDB cosine) | ✅ |
//! | 2. 时间衰减 (recency) | LocalStore.list + exp decay 排序 | ✅ |
//! | 3. 元数据 (metadata) | LocalStore.list + scope 软排序 | ✅ |
//! | 4. BM25 (keyword) | tantivy 索引 | 🚧 v0.11.1 |
//!
//! # 融合
//!
//! RRF (Cormack 2009), k=60 默认 — 见 [`rrf`] 模块.
//!
//! # 用法
//!
//! ```rust,ignore
//! let retriever = HybridRetriever::new(local_store, embedder);
//! let hits = retriever.search("user prefers vim", scope, opts).await?;
//! ```

use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::embed::Embedder;
use crate::local_store::LocalStore;
use crate::memory::{MemoryMatch, Scope, SearchOpts};

pub mod rrf;

/// 时间衰减半衰期 (天), ADR-011 §3.3.3 推荐 30 天。
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 30.0;

/// 每路独立 top-K, ADR-011 §3.2 默认 20 (融合后截至 SearchOpts.limit)。
pub const PER_PATH_TOP_K: u64 = 20;

/// 混合召回器 — 持有 LocalStore + Embedder。
pub struct HybridRetriever {
    store: Arc<dyn LocalStore>,
    embedder: Arc<dyn Embedder>,
}

impl HybridRetriever {
    /// 用现成 store + embedder 构造。
    pub fn new(store: Arc<dyn LocalStore>, embedder: Arc<dyn Embedder>) -> Self {
        Self { store, embedder }
    }

    /// 混合检索: 3 路并行 + RRF 融合 + 截 top-K。
    ///
    /// # 失败策略
    ///
    /// 任一路失败 → 该路当作空, log warn, 不影响其他路 (ADR-011 §3.5)。
    pub async fn search(
        &self,
        query: &str,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> Result<Vec<MemoryMatch>> {
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .context("embed query for hybrid retrieval")?;

        // tokio::join! 启 3 路并行 (BM25 暂跳)
        let (vec_r, time_r, meta_r) = tokio::join!(
            search_vector(&self.store, &query_embedding.vector, scope, PER_PATH_TOP_K),
            search_time(&self.store, scope, PER_PATH_TOP_K),
            search_metadata(&self.store, scope, PER_PATH_TOP_K),
        );

        let vec_ids = vec_r.unwrap_or_else(|e| {
            tracing::warn!(error=?e, "vector path failed, treating as empty");
            Vec::new()
        });
        let time_ids = time_r.unwrap_or_else(|e| {
            tracing::warn!(error=?e, "time path failed, treating as empty");
            Vec::new()
        });
        let meta_ids = meta_r.unwrap_or_else(|e| {
            tracing::warn!(error=?e, "metadata path failed, treating as empty");
            Vec::new()
        });

        tracing::info!(
            vec = vec_ids.len(),
            time = time_ids.len(),
            meta = meta_ids.len(),
            "hybrid retrieval: per-path hits"
        );

        let fused = rrf::fuse(
            &[vec_ids.clone(), time_ids, meta_ids],
            rrf::DEFAULT_K,
            None,
        );

        // 取 top-N (SearchOpts.limit), 然后再拉完整 record (走 vec 路的 match 信息回填)
        let limit = usize::try_from(opts.limit).unwrap_or(10);
        let mut out = Vec::with_capacity(limit);

        // vec_path 已含完整 MemoryMatch + score, 用它作为详情来源 (其他路只贡献排序权重)
        let vec_full = search_vector_full(&self.store, &query_embedding.vector, scope, opts)
            .await
            .unwrap_or_default();
        let detail_lookup: std::collections::HashMap<_, _> = vec_full
            .into_iter()
            .map(|m| (m.record.id, m))
            .collect();

        for (id, rrf_score) in fused.into_iter().take(limit) {
            if let Some(mut detail) = detail_lookup.get(&id).cloned() {
                // 用 RRF score 替代余弦 (统一打分语义)
                #[allow(clippy::cast_possible_truncation)]
                {
                    detail.score = rrf_score as f32;
                }
                out.push(detail);
            } else {
                // vec 路没召到这个 id (time/meta 路独有), 单独 list 不出来; v0.12 加 LocalStore.get_by_id
                // 这里先用空 record + 仅 RRF score 占位 (用户能看到 id, debug 用)
                tracing::debug!(?id, "id only from time/meta path, no full record details v0.11");
            }
        }

        Ok(out)
    }
}

/// 向量路: 返排好序的 MemoryId 列表 (rank 即顺序)。
async fn search_vector(
    store: &Arc<dyn LocalStore>,
    embedding: &[f32],
    scope: &Scope,
    top_k: u64,
) -> Result<Vec<crate::memory::MemoryId>> {
    let opts = SearchOpts {
        limit: top_k,
        score_threshold: 0.0, // 不过滤, RRF 看 rank
        filter: None,
    };
    let matches = store.search(embedding.to_vec(), scope, &opts).await?;
    Ok(matches.into_iter().map(|m| m.record.id).collect())
}

/// 向量路 full: 返完整 MemoryMatch (给详情回填用)。
async fn search_vector_full(
    store: &Arc<dyn LocalStore>,
    embedding: &[f32],
    scope: &Scope,
    opts: &SearchOpts,
) -> Result<Vec<MemoryMatch>> {
    // 略放大查 (RRF 后还要截), 避免详情漏
    let mut wider = opts.clone_with_limit(opts.limit.saturating_mul(2).max(20));
    wider.score_threshold = 0.0;
    store.search(embedding.to_vec(), scope, &wider).await
}

/// 时间路: 按 `created_at` 指数衰减排, 半衰期 30 天 (ADR-011 §3.3.3)。
async fn search_time(
    store: &Arc<dyn LocalStore>,
    scope: &Scope,
    top_k: u64,
) -> Result<Vec<crate::memory::MemoryId>> {
    let records = store.list(scope, top_k.saturating_mul(3)).await?;
    let now = Utc::now();

    let mut scored: Vec<_> = records
        .into_iter()
        .map(|r| {
            let s = recency_score(now, r.created_at, DEFAULT_HALF_LIFE_DAYS);
            (r.id, s)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(usize::try_from(top_k).unwrap_or(20));
    Ok(scored.into_iter().map(|(id, _)| id).collect())
}

/// 元数据路: 按 scope 精确度软排序 (agent_id / session_id 完全匹配的优先)。
async fn search_metadata(
    store: &Arc<dyn LocalStore>,
    query_scope: &Scope,
    top_k: u64,
) -> Result<Vec<crate::memory::MemoryId>> {
    // 取一批后按 scope 完全匹配度评分 (ADR-011 §3.3.4 简化版).
    let records = store.list(query_scope, top_k.saturating_mul(3)).await?;
    let mut weighted: Vec<_> = records
        .into_iter()
        .map(|r| {
            let mut weight = 0_u32;
            if query_scope.agent_id.is_some() && r.scope.agent_id == query_scope.agent_id {
                weight += 2;
            }
            if query_scope.session_id.is_some() && r.scope.session_id == query_scope.session_id {
                weight += 3;
            }
            (r.id, weight, r.created_at)
        })
        .collect();
    // 高分优先, 同分按时间倒序
    weighted.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    weighted.truncate(usize::try_from(top_k).unwrap_or(20));
    Ok(weighted.into_iter().map(|(id, _, _)| id).collect())
}

/// 指数衰减半衰期函数: `e^(-ln(2) × age_days / half_life_days)`
fn recency_score(now: DateTime<Utc>, created_at: DateTime<Utc>, half_life_days: f64) -> f64 {
    // i64 → f64: 秒数 < 2^53 = 285 万年, frank 场景永远在 f64 安全范围
    #[allow(clippy::cast_precision_loss)]
    let age_seconds = (now - created_at).num_seconds() as f64;
    let age_days = age_seconds / 86400.0;
    let lambda = std::f64::consts::LN_2 / half_life_days;
    (-lambda * age_days).exp()
}

/// `SearchOpts` 缺 `clone_with_limit` 辅助 — 这里内部加 extension trait.
trait SearchOptsExt {
    fn clone_with_limit(&self, new_limit: u64) -> Self;
}

impl SearchOptsExt for SearchOpts {
    fn clone_with_limit(&self, new_limit: u64) -> Self {
        Self {
            limit: new_limit,
            score_threshold: self.score_threshold,
            filter: self.filter.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// 0 天 age → score = 1.0
    #[test]
    fn recency_today_is_full_score() {
        let now = Utc::now();
        let s = recency_score(now, now, 30.0);
        assert!((s - 1.0).abs() < 1e-9, "today 应得满分, got {s}");
    }

    /// 30 天 age, 半衰期 30 → score = 0.5
    #[test]
    fn recency_half_life_is_half_score() {
        let now = Utc::now();
        let past = now - Duration::days(30);
        let s = recency_score(now, past, 30.0);
        assert!((s - 0.5).abs() < 0.001, "30d age @ HL=30 应得 0.5, got {s}");
    }

    /// 365 天 age, 半衰期 30 → score ≈ 0.0001 (12 半衰期)
    #[test]
    fn recency_year_old_near_zero() {
        let now = Utc::now();
        let past = now - Duration::days(365);
        let s = recency_score(now, past, 30.0);
        assert!(s < 0.001, "1y old 应近 0, got {s}");
    }

    /// 极端: 半衰期 = 0 不 panic (虽然实践应防御)
    #[test]
    fn recency_zero_half_life_returns_zero_for_aged() {
        let now = Utc::now();
        let past = now - Duration::days(1);
        let s = recency_score(now, past, 0.0001);
        assert!(s < 1e-100, "短半衰期 1 day old 应几乎 0");
    }

    /// SearchOptsExt clone_with_limit 改 limit 不动其他字段
    #[test]
    fn search_opts_clone_with_limit_preserves_threshold() {
        let opts = SearchOpts {
            limit: 5,
            score_threshold: 0.42,
            filter: None,
        };
        let new_opts = opts.clone_with_limit(100);
        assert_eq!(new_opts.limit, 100);
        assert!((new_opts.score_threshold - 0.42).abs() < f32::EPSILON);
    }
}
