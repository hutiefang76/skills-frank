//! Qdrant 后端实现。
//!
//! 一个 collection `frank_memories`, 所有 scope 的记忆共用 (用 payload filter 隔离)。
//! 维度由 [`MemoryStore::ensure_initialized`] 传入, 与 embedder 维度匹配。

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, GetPointsBuilder,
    PointStruct, PointsIdsList, ScrollPointsBuilder, SearchPointsBuilder, UpsertPointsBuilder,
    Value as QdrantValue, VectorParamsBuilder,
};
use qdrant_client::{Payload, Qdrant};

use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
use crate::store::{EmbeddedRecord, MemoryStore};

/// Qdrant 后端实现。线程安全 (内部 `Qdrant` 持 `Arc`)。
///
/// `Qdrant` 自身不实现 `Debug`, 因此本结构也不能 `derive(Debug)`; 调试输出走 `tracing`。
pub struct QdrantStore {
    client: Qdrant,
    collection: String,
}

impl QdrantStore {
    /// 从 URL (例如 `http://localhost:6334`) 构造客户端。
    pub fn from_url(url: &str, collection: impl Into<String>) -> Result<Self> {
        let client = Qdrant::from_url(url)
            .build()
            .with_context(|| format!("connect Qdrant at {url}"))?;
        Ok(Self {
            client,
            collection: collection.into(),
        })
    }
}

#[async_trait]
impl MemoryStore for QdrantStore {
    async fn ensure_initialized(&self, vector_dim: u64) -> Result<()> {
        let exists = self
            .client
            .collection_exists(&self.collection)
            .await
            .with_context(|| format!("check collection {} exists", self.collection))?;
        if !exists {
            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection)
                        .vectors_config(VectorParamsBuilder::new(vector_dim, Distance::Cosine)),
                )
                .await
                .with_context(|| format!("create collection {}", self.collection))?;
            tracing::info!(collection = %self.collection, vector_dim, "collection created");
        }
        Ok(())
    }

    async fn upsert(&self, item: EmbeddedRecord) -> Result<()> {
        self.upsert_batch(vec![item]).await
    }

    async fn upsert_batch(&self, items: Vec<EmbeddedRecord>) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut points: Vec<PointStruct> = Vec::with_capacity(items.len());
        for item in items {
            let payload = record_to_payload(&item.record)?;
            points.push(PointStruct::new(
                item.record.id.to_string(),
                item.embedding,
                payload,
            ));
        }
        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection, points).wait(true))
            .await
            .context("upsert points")?;
        Ok(())
    }

    async fn search(
        &self,
        query_vector: Vec<f32>,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> Result<Vec<MemoryMatch>> {
        let mut builder = SearchPointsBuilder::new(&self.collection, query_vector, opts.limit)
            .score_threshold(opts.score_threshold)
            .with_payload(true);
        if let Some(filter) = scope_to_filter(scope) {
            builder = builder.filter(filter);
        }
        let response = self
            .client
            .search_points(builder)
            .await
            .context("search points")?;

        let mut matches = Vec::with_capacity(response.result.len());
        for hit in response.result {
            let record = payload_to_record(&hit.payload)?;
            matches.push(MemoryMatch {
                record,
                score: hit.score,
            });
        }
        Ok(matches)
    }

    async fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>> {
        let response = self
            .client
            .get_points(
                GetPointsBuilder::new(&self.collection, vec![id.to_string().into()])
                    .with_payload(true)
                    .with_vectors(false),
            )
            .await
            .context("get points by id")?;
        Ok(response
            .result
            .into_iter()
            .next()
            .map(|p| payload_to_record(&p.payload))
            .transpose()?)
    }

    async fn delete(&self, id: &MemoryId) -> Result<()> {
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection)
                    .points(PointsIdsList {
                        ids: vec![id.to_string().into()],
                    })
                    .wait(true),
            )
            .await
            .context("delete point")?;
        Ok(())
    }

    async fn list(&self, scope: &Scope, limit: u64) -> Result<Vec<MemoryRecord>> {
        let mut builder = ScrollPointsBuilder::new(&self.collection)
            .limit(u32::try_from(limit).unwrap_or(u32::MAX))
            .with_payload(true);
        if let Some(filter) = scope_to_filter(scope) {
            builder = builder.filter(filter);
        }
        let response = self.client.scroll(builder).await.context("scroll points")?;
        let mut records = Vec::with_capacity(response.result.len());
        for point in response.result {
            records.push(payload_to_record(&point.payload)?);
        }
        Ok(records)
    }
}

// ---- 内部辅助 ----

/// 把 `Scope` 三字段转成 Qdrant 必须条件 (must = AND)。
/// 全 `None` 时返回 `None` (无过滤)。
///
/// **Payload key 用 dot-notation** — `MemoryRecord` serde 后 `scope` 是嵌套对象,
/// Qdrant 走 `scope.user_id` / `scope.agent_id` / `scope.session_id` 取值。早期版本
/// 用了顶层 `user_id` 等导致 list/search filter 全部漏命中 (codex review + 真测确认).
fn scope_to_filter(scope: &Scope) -> Option<Filter> {
    let mut conditions: Vec<Condition> = Vec::new();
    if let Some(u) = &scope.user_id {
        conditions.push(Condition::matches("scope.user_id", u.clone()));
    }
    if let Some(a) = &scope.agent_id {
        conditions.push(Condition::matches("scope.agent_id", a.clone()));
    }
    if let Some(s) = &scope.session_id {
        conditions.push(Condition::matches("scope.session_id", s.clone()));
    }
    if conditions.is_empty() {
        None
    } else {
        Some(Filter::must(conditions))
    }
}

/// 把 `MemoryRecord` 通过 serde_json 转成 Qdrant Payload。
fn record_to_payload(record: &MemoryRecord) -> Result<Payload> {
    let val = serde_json::to_value(record).context("serialize MemoryRecord")?;
    Payload::try_from(val).context("convert serde_json::Value to qdrant Payload")
}

/// 把 Qdrant payload (`HashMap<String, Value>`) 转回 `MemoryRecord`。
fn payload_to_record(payload: &HashMap<String, QdrantValue>) -> Result<MemoryRecord> {
    let mut map = serde_json::Map::with_capacity(payload.len());
    for (k, v) in payload {
        map.insert(k.clone(), qdrant_value_to_json(v));
    }
    let json = serde_json::Value::Object(map);
    serde_json::from_value(json).context("deserialize Qdrant payload to MemoryRecord")
}

/// Qdrant gRPC Value → serde_json Value。
fn qdrant_value_to_json(v: &QdrantValue) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;
    match &v.kind {
        Some(Kind::NullValue(_)) | None => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Kind::IntegerValue(i)) => serde_json::Value::Number((*i).into()),
        Some(Kind::DoubleValue(d)) => serde_json::Number::from_f64(*d)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Some(Kind::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Kind::ListValue(l)) => {
            serde_json::Value::Array(l.values.iter().map(qdrant_value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => {
            let map: serde_json::Map<String, serde_json::Value> = s
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), qdrant_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryId, Scope};
    use chrono::Utc;

    #[test]
    fn scope_to_filter_empty_when_scope_empty() {
        assert!(scope_to_filter(&Scope::default()).is_none());
    }

    #[test]
    fn scope_to_filter_has_conditions_when_user_set() {
        let s = Scope::user("alice");
        let filter = scope_to_filter(&s).unwrap();
        // Filter::must 内部 should 列表; 至少有一个 must 条件
        assert!(!filter.must.is_empty());
    }

    #[test]
    fn record_payload_roundtrip_via_json() {
        let rec = MemoryRecord {
            id: MemoryId::new(),
            content: "test fact".to_string(),
            scope: Scope::user("alice"),
            metadata: serde_json::json!({ "src": "test" }),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        // 模拟: serialize → 假装走完 qdrant → 反序列化
        // 这里只验 serde 层的等价性 (qdrant 真测要 docker, 走集成测)
        let val = serde_json::to_value(&rec).unwrap();
        let back: MemoryRecord = serde_json::from_value(val).unwrap();
        assert_eq!(back.id, rec.id);
        assert_eq!(back.content, rec.content);
    }
}
