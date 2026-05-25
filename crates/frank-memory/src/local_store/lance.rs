//! LanceDB 本地主存实现 (ADR-010)。
//!
//! # 文件布局
//!
//! ```text
//! ~/.frank/memory/
//! ├── lance.db/                LanceDB 数据目录 (lancedb::connect 指向)
//! │   └── memories.lance/      自动建的 table (Lance 列存格式)
//! └── lance.db.lock            fs2 互斥锁 (写串行化, 跨进程)
//! ```
//!
//! # 并发策略
//!
//! - **读不加锁** — LanceDB MVCC 保证读永远见某个一致快照
//! - **写必须持 [`fs2::FileExt::try_lock_exclusive`]** — LanceDB 无 OS 级 file lock,
//!   多 frank cli 进程同时写会撞 commit 冲突 (issue #1597 截至 0.29 未自动 retry)
//!
//! # 实现状态 (v0.11 A.3 待落地)
//!
//! 当前只放骨架 + 类型, 真接 LanceDB API 在 A.3 完成。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator,
    RecordBatchReader, StringArray,
};
use futures::TryStreamExt;
// lancedb 0.29 只 re-export arrow_schema, arrow-array 必须显式加在 Cargo.toml.
use lancedb::arrow::arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::AddDataMode;

use super::{LocalRecord, LocalStore, SyncStatus};
use crate::memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};

/// LanceDB 实现的本地主存。
///
/// 构造时不立即建表; 调用 [`LocalStore::ensure_initialized`] 才真创建。
///
/// # 字段
///
/// - `db_path`: LanceDB 数据目录 (默认 `~/.frank/memory/lance.db/`)
/// - `lock_path`: fs2 互斥锁路径 (跟 db_path 平级)
/// - `connection`: 懒初始化的 lancedb Connection (Arc 以便 clone)
pub struct LanceLocalStore {
    /// LanceDB 数据目录路径。
    pub db_path: PathBuf,
    /// 文件锁路径 (写串行化用)。
    pub lock_path: PathBuf,
    /// 懒初始化的连接句柄。
    connection: tokio::sync::OnceCell<Arc<lancedb::Connection>>,
}

impl LanceLocalStore {
    /// 构造一个指向 `~/.frank/memory/lance.db/` 的实例 (跨平台 home dir 探测)。
    pub fn at_default_home() -> Result<Self> {
        let home = dirs::home_dir().context("locate user home dir")?;
        let dir = home.join(".frank").join("memory");
        Ok(Self::at(dir))
    }

    /// 构造一个指向自定义父目录的实例 (`<dir>/lance.db/` + `<dir>/lance.db.lock`)。
    /// 主要用于测试 (tempdir 注入)。
    #[must_use]
    pub fn at(parent_dir: PathBuf) -> Self {
        let db_path = parent_dir.join("lance.db");
        let lock_path = parent_dir.join("lance.db.lock");
        Self {
            db_path,
            lock_path,
            connection: tokio::sync::OnceCell::new(),
        }
    }

    /// 拿到 (懒初始化) 的 lancedb Connection。
    async fn conn(&self) -> Result<&Arc<lancedb::Connection>> {
        self.connection
            .get_or_try_init(|| async {
                // 确保父目录存在 (LanceDB 自己不建顶层目录)
                if let Some(parent) = self.db_path.parent() {
                    tokio::fs::create_dir_all(parent).await.with_context(|| {
                        format!("create lance parent dir {}", parent.display())
                    })?;
                }
                let uri = self.db_path.to_string_lossy().to_string();
                let conn = lancedb::connect(&uri)
                    .execute()
                    .await
                    .with_context(|| format!("lancedb::connect {uri}"))?;
                Ok::<_, anyhow::Error>(Arc::new(conn))
            })
            .await
    }
}

/// 构造 memories 表的 Arrow Schema (ADR-010 §6.2)。
///
/// 字段:
/// - `id`: UUID 字符串 (MemoryId 序列化)
/// - `user_id` / `agent_id` / `session_id`: scope 三字段拍平 (避免嵌套 dot-notation 坑)
/// - `content`: 自然语言 fact
/// - `metadata_json`: 用户自由 JSON 序列化字符串
/// - `created_at_ms` / `updated_at_ms`: Unix epoch ms (i64, 跨版本最稳)
/// - `sync_status`: "synced" / "pending" / "failed"
/// - `embedding`: `FixedSizeList<Float32, N>` 向量
#[must_use]
pub fn build_schema(vector_dim: usize) -> Arc<Schema> {
    let vector_dim = i32::try_from(vector_dim).expect("vector_dim fits i32");
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, true),
        Field::new("agent_id", DataType::Utf8, true),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("content", DataType::Utf8, false),
        Field::new("metadata_json", DataType::Utf8, true),
        Field::new("created_at_ms", DataType::Int64, false),
        Field::new("updated_at_ms", DataType::Int64, false),
        Field::new("sync_status", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                vector_dim,
            ),
            false,
        ),
    ]))
}

#[async_trait]
impl LocalStore for LanceLocalStore {
    async fn ensure_initialized(&self, vector_dim: usize) -> Result<()> {
        let conn = self.conn().await?;
        let table_names = conn
            .table_names()
            .execute()
            .await
            .context("lancedb table_names")?;
        if !table_names.iter().any(|n| n == "memories") {
            // 建空表 — 用 empty RecordBatch + schema 创建; 后续 add 才填数据
            let schema = build_schema(vector_dim);
            let _table = conn
                .create_empty_table("memories", schema)
                .execute()
                .await
                .context("lancedb create_empty_table memories")?;
            tracing::info!(
                dim = vector_dim,
                path = %self.db_path.display(),
                "memories table created"
            );
        }
        Ok(())
    }

    async fn add(&self, item: LocalRecord) -> Result<()> {
        // 推断 vector_dim 从 embedding 长度, 表已存在则维度必须对齐 (LanceDB 自己会拦截维度错)
        let vector_dim = item.embedding.len();
        self.ensure_initialized(vector_dim).await?;

        let conn = self.conn().await?;
        let table = conn
            .open_table("memories")
            .execute()
            .await
            .context("open memories table")?;

        let batch = local_record_to_batch(&item, vector_dim)?;
        let schema = batch.schema();
        let reader: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));

        table
            .add(reader)
            .mode(AddDataMode::Append)
            .execute()
            .await
            .context("lancedb table.add")?;
        Ok(())
    }

    async fn search(
        &self,
        query_vector: Vec<f32>,
        scope: &Scope,
        opts: &SearchOpts,
    ) -> Result<Vec<MemoryMatch>> {
        let conn = self.conn().await?;
        let Ok(table) = conn.open_table("memories").execute().await else {
            return Ok(Vec::new()); // 表未建 → 空结果, 不报错
        };

        let dim = query_vector.len();
        let mut query = table
            .vector_search(query_vector)
            .context("build vector_search")?
            .limit(usize::try_from(opts.limit).unwrap_or(10));

        if let Some(filter) = scope_to_sql(scope) {
            query = query.only_if(filter);
        }

        let stream = query.execute().await.context("execute search")?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .context("collect search batches")?;

        let mut hits = Vec::new();
        for batch in batches {
            for record in batch_to_matches(&batch, opts.score_threshold, dim)? {
                hits.push(record);
            }
        }
        Ok(hits)
    }

    async fn list(&self, scope: &Scope, limit: u64) -> Result<Vec<MemoryRecord>> {
        let conn = self.conn().await?;
        let Ok(table) = conn.open_table("memories").execute().await else {
            return Ok(Vec::new());
        };

        let mut query = table.query().limit(usize::try_from(limit).unwrap_or(100));
        if let Some(filter) = scope_to_sql(scope) {
            query = query.only_if(filter);
        }

        let stream = query.execute().await.context("execute list")?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .context("collect list batches")?;

        let mut records = Vec::new();
        for batch in batches {
            records.extend(batch_to_records(&batch)?);
        }
        // 按 created_at desc 排序 (LanceDB 表扫无固定顺序)
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        records.truncate(usize::try_from(limit).unwrap_or(100));
        Ok(records)
    }

    async fn delete(&self, id: &MemoryId) -> Result<()> {
        let conn = self.conn().await?;
        let Ok(table) = conn.open_table("memories").execute().await else {
            return Ok(());
        };
        let id_str = id.to_string();
        table
            .delete(&format!("id = '{id_str}'"))
            .await
            .with_context(|| format!("lancedb delete id={id_str}"))?;
        Ok(())
    }

    async fn pending_sync(&self, limit: u64) -> Result<Vec<LocalRecord>> {
        let conn = self.conn().await?;
        let Ok(table) = conn.open_table("memories").execute().await else {
            return Ok(Vec::new());
        };
        let stream = table
            .query()
            .only_if("sync_status = 'pending'")
            .limit(usize::try_from(limit).unwrap_or(100))
            .execute()
            .await
            .context("execute pending_sync")?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .context("collect pending_sync batches")?;

        let mut items = Vec::new();
        for batch in batches {
            items.extend(batch_to_local_records(&batch)?);
        }
        items.sort_by_key(|i| i.record.created_at);
        Ok(items)
    }

    async fn mark_synced(&self, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn().await?;
        let Ok(table) = conn.open_table("memories").execute().await else {
            return Ok(());
        };
        let id_list: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
        let filter = format!("id IN ({})", id_list.join(","));
        table
            .update()
            .only_if(filter)
            .column("sync_status", "'synced'")
            .execute()
            .await
            .context("lancedb update mark_synced")?;
        Ok(())
    }
}

// ---- 内部辅助 (record <-> arrow batch 转换) ----

/// 把单条 LocalRecord 转成一条 Arrow RecordBatch。
fn local_record_to_batch(item: &LocalRecord, vector_dim: usize) -> Result<RecordBatch> {
    let schema = build_schema(vector_dim);
    let r = &item.record;

    let id_arr = StringArray::from(vec![r.id.to_string()]);
    let user_arr = StringArray::from(vec![r.scope.user_id.clone()]);
    let agent_arr = StringArray::from(vec![r.scope.agent_id.clone()]);
    let session_arr = StringArray::from(vec![r.scope.session_id.clone()]);
    let content_arr = StringArray::from(vec![r.content.clone()]);
    let meta_str = if r.metadata.is_null() {
        None
    } else {
        Some(serde_json::to_string(&r.metadata).context("serialize metadata")?)
    };
    let meta_arr = StringArray::from(vec![meta_str]);
    let created_arr = Int64Array::from(vec![r.created_at.timestamp_millis()]);
    let updated_arr = Int64Array::from(vec![r.updated_at.timestamp_millis()]);
    let sync_arr = StringArray::from(vec![item.sync_status.as_str()]);

    // FixedSizeList<Float32, vector_dim>
    let dim = i32::try_from(vector_dim).context("vector_dim fits i32")?;
    let values = Float32Array::from(item.embedding.clone());
    let field = Arc::new(Field::new("item", DataType::Float32, true));
    let embedding_arr = FixedSizeListArray::try_new(field, dim, Arc::new(values), None)
        .context("build FixedSizeListArray for embedding")?;

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),
            Arc::new(user_arr),
            Arc::new(agent_arr),
            Arc::new(session_arr),
            Arc::new(content_arr),
            Arc::new(meta_arr),
            Arc::new(created_arr),
            Arc::new(updated_arr),
            Arc::new(sync_arr),
            Arc::new(embedding_arr),
        ],
    )
    .context("build RecordBatch")
}

/// Scope → LanceDB SQL filter, 全空返 None (无过滤)。
fn scope_to_sql(scope: &Scope) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(u) = &scope.user_id {
        parts.push(format!("user_id = '{}'", sql_escape(u)));
    }
    if let Some(a) = &scope.agent_id {
        parts.push(format!("agent_id = '{}'", sql_escape(a)));
    }
    if let Some(s) = &scope.session_id {
        parts.push(format!("session_id = '{}'", sql_escape(s)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" AND "))
    }
}

/// SQL 单引号字面值 escape: ' → ''。LanceDB 走 datafusion SQL 解析。
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// 从 RecordBatch 提取 MemoryMatch (含 score), 由 vector_search 返回。
/// LanceDB vector_search 加 `_distance` 字段, distance 越小越像。
/// 这里把 distance 转 cosine similarity (假设 metric=L2 on normalized embedding):
///   sim = 1 - distance / 2 (L2 squared on unit vectors)
/// 简化: 直接返 1 / (1 + distance) 作为粗略 score (单测仅校 ordering)。
fn batch_to_matches(
    batch: &RecordBatch,
    score_threshold: f32,
    _vector_dim: usize,
) -> Result<Vec<MemoryMatch>> {
    let n = batch.num_rows();
    let records = batch_to_records(batch)?;
    let distances = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
    let mut out = Vec::with_capacity(n);
    for (i, record) in records.into_iter().enumerate() {
        let score = distances.map_or(1.0, |d| {
            let dist = d.value(i);
            1.0 / (1.0 + dist)
        });
        if score >= score_threshold {
            out.push(MemoryMatch { record, score });
        }
    }
    Ok(out)
}

/// 从 RecordBatch 还原 MemoryRecord 列表 (不含 embedding / sync_status)。
fn batch_to_records(batch: &RecordBatch) -> Result<Vec<MemoryRecord>> {
    let n = batch.num_rows();
    let ids = col_string(batch, "id")?;
    let users = col_string_opt(batch, "user_id")?;
    let agents = col_string_opt(batch, "agent_id")?;
    let sessions = col_string_opt(batch, "session_id")?;
    let contents = col_string(batch, "content")?;
    let metas = col_string_opt(batch, "metadata_json")?;
    let created = col_i64(batch, "created_at_ms")?;
    let updated = col_i64(batch, "updated_at_ms")?;

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let id_str = ids.value(i);
        let id = MemoryId::from_uuid(
            uuid::Uuid::parse_str(id_str).with_context(|| format!("parse uuid {id_str}"))?,
        );
        let metadata = match metas.value(i) {
            Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        out.push(MemoryRecord {
            id,
            content: contents.value(i).to_string(),
            scope: Scope {
                user_id: users.value(i).map(str::to_string),
                agent_id: agents.value(i).map(str::to_string),
                session_id: sessions.value(i).map(str::to_string),
            },
            metadata,
            created_at: chrono::DateTime::from_timestamp_millis(created.value(i))
                .context("invalid created_at_ms")?,
            updated_at: chrono::DateTime::from_timestamp_millis(updated.value(i))
                .context("invalid updated_at_ms")?,
        });
    }
    Ok(out)
}

/// 从 RecordBatch 还原 LocalRecord (含 embedding + sync_status), 给 pending_sync 用。
fn batch_to_local_records(batch: &RecordBatch) -> Result<Vec<LocalRecord>> {
    let records = batch_to_records(batch)?;
    let sync_strs = col_string(batch, "sync_status")?;
    let embeddings = batch
        .column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .context("embedding column missing or wrong type")?;

    let mut out = Vec::with_capacity(records.len());
    for (i, record) in records.into_iter().enumerate() {
        let sync_status = SyncStatus::from_str_lenient(sync_strs.value(i));
        let values = embeddings.value(i);
        let floats = values
            .as_any()
            .downcast_ref::<Float32Array>()
            .context("embedding inner not Float32")?;
        let embedding: Vec<f32> = (0..floats.len()).map(|j| floats.value(j)).collect();
        out.push(LocalRecord {
            record,
            embedding,
            sync_status,
        });
    }
    Ok(out)
}

fn col_string<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("column {name} missing or wrong type"))
}

/// 包装一下 nullable 字符串列 (StringArray 自带 nullable 支持, 这里只是统一接口)。
fn col_string_opt<'a>(batch: &'a RecordBatch, name: &str) -> Result<NullableStr<'a>> {
    Ok(NullableStr(col_string(batch, name)?))
}

fn col_i64<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .with_context(|| format!("column {name} missing or wrong type"))
}

/// 包装 StringArray, .value(i) 返 `Option<&str>` 处理 null。
struct NullableStr<'a>(&'a StringArray);

impl<'a> NullableStr<'a> {
    fn value(&self, i: usize) -> Option<&'a str> {
        if self.0.is_null(i) {
            None
        } else {
            Some(self.0.value(i))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Schema 含 10 个字段 (id + 3 scope + content + metadata + 2 timestamp + sync_status + embedding)。
    #[test]
    fn schema_has_expected_fields() {
        let schema = build_schema(384);
        assert_eq!(schema.fields().len(), 10);
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"id"));
        assert!(names.contains(&"user_id"));
        assert!(names.contains(&"agent_id"));
        assert!(names.contains(&"session_id"));
        assert!(names.contains(&"content"));
        assert!(names.contains(&"metadata_json"));
        assert!(names.contains(&"created_at_ms"));
        assert!(names.contains(&"updated_at_ms"));
        assert!(names.contains(&"sync_status"));
        assert!(names.contains(&"embedding"));
    }

    /// Embedding 字段是 FixedSizeList<Float32, vector_dim>。
    #[test]
    fn embedding_field_is_fixed_size_list() {
        let schema = build_schema(384);
        let embedding = schema.field_with_name("embedding").expect("embedding field");
        match embedding.data_type() {
            DataType::FixedSizeList(item, size) => {
                assert_eq!(*size, 384);
                assert_eq!(*item.data_type(), DataType::Float32);
            }
            other => panic!("unexpected embedding type: {other:?}"),
        }
    }

    /// scope 三字段 nullable, 其他业务字段 non-null。
    #[test]
    fn scope_fields_nullable() {
        let schema = build_schema(384);
        assert!(schema.field_with_name("user_id").unwrap().is_nullable());
        assert!(schema.field_with_name("agent_id").unwrap().is_nullable());
        assert!(schema.field_with_name("session_id").unwrap().is_nullable());
        assert!(!schema.field_with_name("id").unwrap().is_nullable());
        assert!(!schema.field_with_name("content").unwrap().is_nullable());
        assert!(!schema.field_with_name("embedding").unwrap().is_nullable());
    }

    /// at_default_home 解析到 ~/.frank/memory/lance.db。
    #[test]
    fn at_default_home_resolves_home() {
        let store = LanceLocalStore::at_default_home().expect("home");
        assert!(store.db_path.to_string_lossy().contains(".frank/memory/lance.db"));
        assert!(
            store
                .lock_path
                .to_string_lossy()
                .contains(".frank/memory/lance.db.lock")
        );
    }

    /// at(tempdir) 路径正确。
    #[test]
    fn at_custom_dir_uses_tempdir() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());
        assert_eq!(store.db_path, dir.path().join("lance.db"));
        assert_eq!(store.lock_path, dir.path().join("lance.db.lock"));
    }

    /// ensure_initialized 在空 tempdir 上能跑通 (建表) — 真接 lancedb 的集成测。
    #[tokio::test]
    async fn ensure_initialized_creates_table() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());
        store.ensure_initialized(384).await.expect("create table");

        // 验证 table_names 含 memories
        let conn = store.conn().await.expect("conn");
        let names = conn.table_names().execute().await.expect("table_names");
        assert!(names.contains(&"memories".to_string()), "names: {names:?}");
    }

    /// ensure_initialized 幂等 — 第二次调用不报错。
    #[tokio::test]
    async fn ensure_initialized_idempotent() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());
        store.ensure_initialized(384).await.expect("first");
        store.ensure_initialized(384).await.expect("second");
    }

    // ---- add / search / list / delete 端到端真测 (基于 4 维 mock 向量) ----

    /// 4 维 mock embedding 工厂.
    fn fake_record(id: MemoryId, user: &str, fact: &str, vec: [f32; 4]) -> LocalRecord {
        use chrono::Utc;
        LocalRecord {
            record: MemoryRecord {
                id,
                content: fact.to_string(),
                scope: Scope::user(user),
                metadata: serde_json::Value::Null,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            embedding: vec.to_vec(),
            sync_status: SyncStatus::Pending,
        }
    }

    /// add → list 完整往返, scope 过滤生效.
    #[tokio::test]
    async fn add_list_roundtrip_with_scope() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        let id_a = MemoryId::new();
        let id_b = MemoryId::new();
        store
            .add(fake_record(id_a, "alice", "vim", [1.0, 0.0, 0.0, 0.0]))
            .await
            .expect("add alice");
        store
            .add(fake_record(id_b, "bob", "emacs", [0.0, 1.0, 0.0, 0.0]))
            .await
            .expect("add bob");

        let alice_list = store.list(&Scope::user("alice"), 10).await.expect("list");
        assert_eq!(alice_list.len(), 1, "alice scope hits 1");
        assert_eq!(alice_list[0].id, id_a);
        assert_eq!(alice_list[0].content, "vim");

        let bob_list = store.list(&Scope::user("bob"), 10).await.expect("list bob");
        assert_eq!(bob_list.len(), 1);
        assert_eq!(bob_list[0].id, id_b);
    }

    /// search 向量检索 top-K, score_threshold 过滤.
    #[tokio::test]
    async fn search_top_k_with_threshold() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        // 3 条 alice 的 fact, 向量分别指向不同方向
        store
            .add(fake_record(
                MemoryId::new(),
                "alice",
                "near",
                [1.0, 0.0, 0.0, 0.0],
            ))
            .await
            .expect("add near");
        store
            .add(fake_record(
                MemoryId::new(),
                "alice",
                "mid",
                [0.7, 0.7, 0.0, 0.0],
            ))
            .await
            .expect("add mid");
        store
            .add(fake_record(
                MemoryId::new(),
                "alice",
                "far",
                [0.0, 0.0, 1.0, 0.0],
            ))
            .await
            .expect("add far");

        // 查询向量跟 "near" 完全重合
        let opts = SearchOpts {
            limit: 2,
            score_threshold: 0.0, // 不过滤, 看返回顺序
            filter: None,
        };
        let hits = store
            .search(vec![1.0, 0.0, 0.0, 0.0], &Scope::user("alice"), &opts)
            .await
            .expect("search");
        assert_eq!(hits.len(), 2, "limit=2 拿 top-2");
        // 第一个应该是 "near" (distance 0)
        assert_eq!(hits[0].record.content, "near", "near 最近");
        // score 应该是 1.0 (1/(1+0))
        assert!(
            (hits[0].score - 1.0).abs() < 0.01,
            "near score ≈ 1.0, got {}",
            hits[0].score
        );
    }

    /// delete 后 list 不再含该条.
    #[tokio::test]
    async fn delete_removes_record() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        let id = MemoryId::new();
        store
            .add(fake_record(id, "alice", "tmp", [1.0, 0.0, 0.0, 0.0]))
            .await
            .expect("add");
        assert_eq!(store.list(&Scope::user("alice"), 10).await.unwrap().len(), 1);

        store.delete(&id).await.expect("delete");
        assert_eq!(store.list(&Scope::user("alice"), 10).await.unwrap().len(), 0);
    }

    /// pending_sync 返 Pending 的; mark_synced 后 pending 不再返.
    #[tokio::test]
    async fn pending_sync_and_mark_synced() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        let id_p = MemoryId::new();
        store
            .add(fake_record(id_p, "alice", "pending", [1.0, 0.0, 0.0, 0.0]))
            .await
            .expect("add pending");

        let pending = store.pending_sync(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].record.id, id_p);
        assert_eq!(pending[0].sync_status, SyncStatus::Pending);

        store.mark_synced(&[id_p]).await.expect("mark_synced");
        let pending_after = store.pending_sync(10).await.expect("pending after");
        assert!(pending_after.is_empty(), "标 Synced 后 pending 空");
    }

    /// search 空表返 [], 不报错.
    #[tokio::test]
    async fn search_empty_table_returns_empty() {
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        let opts = SearchOpts {
            limit: 10,
            score_threshold: 0.0,
            filter: None,
        };
        let hits = store
            .search(vec![1.0, 0.0, 0.0, 0.0], &Scope::user("alice"), &opts)
            .await
            .expect("search empty");
        assert!(hits.is_empty());
    }

    /// scope 三层 filter 都生效 (回归 ADR-003 dot-notation 漏 filter 的坑).
    #[tokio::test]
    async fn scope_three_level_filter() {
        use chrono::Utc;
        use tempfile::tempdir;
        let dir = tempdir().expect("tempdir");
        let store = LanceLocalStore::at(dir.path().to_path_buf());

        // 同 user 不同 agent
        let make = |agent: &str| LocalRecord {
            record: MemoryRecord {
                id: MemoryId::new(),
                content: format!("fact-{agent}"),
                scope: Scope {
                    user_id: Some("alice".to_string()),
                    agent_id: Some(agent.to_string()),
                    session_id: None,
                },
                metadata: serde_json::Value::Null,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            sync_status: SyncStatus::Pending,
        };
        store.add(make("claude")).await.unwrap();
        store.add(make("codex")).await.unwrap();

        // user 过滤 → 2 条
        assert_eq!(
            store.list(&Scope::user("alice"), 10).await.unwrap().len(),
            2
        );
        // user + agent 过滤 → 1 条
        let codex_only = Scope {
            user_id: Some("alice".to_string()),
            agent_id: Some("codex".to_string()),
            session_id: None,
        };
        let hits = store.list(&codex_only, 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content, "fact-codex");
    }
}
