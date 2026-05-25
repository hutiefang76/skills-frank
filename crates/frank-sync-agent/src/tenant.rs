//! Tenant registry — SQLite 持久化 tenants 表 (v0.12.0 ADR-012 / PHASE-10-PLAN).
//!
//! # 表 schema
//!
//! ```sql
//! CREATE TABLE tenants (
//!   tenant_id TEXT PRIMARY KEY,                  -- sha256(token)[:12] hex
//!   created_at INTEGER NOT NULL,                 -- unix epoch sec
//!   last_seen INTEGER NOT NULL,                  -- 任何操作更新
//!   records_count INTEGER NOT NULL DEFAULT 0,    -- 已用 quota
//!   deletion_scheduled_at INTEGER                -- NULL = 不删; >0 = 14d 后真删 epoch
//! );
//! ```
//!
//! # 用法
//!
//! ```ignore
//! let store = TenantStore::open("/var/lib/frank/tenants.db").await?;
//! store.register(&tenant_id).await?;
//! let status = store.status(&tenant_id).await?;
//! if status.records_count >= quota {
//!     return Err("quota exceeded");
//! }
//! store.bump_records(&tenant_id, +1).await?;
//! ```
//!
//! # 并发模型
//!
//! - SQLite 走 WAL mode + 单 writer / 多 reader
//! - 异步层用 `tokio::sync::Mutex` 包 `Connection` 串行化 (写少, 锁开销可忽略)
//! - 也可走 `tokio::task::spawn_blocking` + `Arc<Mutex<Conn>>`, 这里选简单的内部锁

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// Tenant 的派生 ID (来自 token).
pub type TenantId = String;

/// 从 token 派生 tenant_id (sha256 前 12 hex 字符 = 48 bit).
/// 单测用 + 未来 client-side 复用. 当前 routes 已经走 `tenant_id_from_headers` 重复逻辑.
#[allow(dead_code)]
#[must_use]
pub fn derive_tenant_id(token: &str) -> TenantId {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(&hasher.finalize()[..6])
}

/// tenant 的当前状态 (供 /tenant/status 返回 + 内部 quota 检查).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TenantStatus {
    /// tenant_id (12 hex).
    pub tenant_id: TenantId,
    /// 注册时间 (epoch sec).
    pub created_at: i64,
    /// 最后访问 (epoch sec).
    pub last_seen: i64,
    /// 已用 record 数.
    pub records_count: i64,
    /// 删除调度 (None = 未申请, Some(epoch) = 申请, 到点真删).
    pub deletion_scheduled_at: Option<i64>,
}

/// Tenant 表的持久化存储 (SQLite).
pub struct TenantStore {
    conn: Arc<Mutex<Connection>>,
}

impl TenantStore {
    /// 打开 / 创建 SQLite 文件. 父目录不存在会建.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create dir {}", parent.display()))?;
            }
            let conn = Connection::open(&path)
                .with_context(|| format!("open sqlite {}", path.display()))?;
            // WAL 模式 (多 reader + 单 writer 并发好)
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS tenants (
                    tenant_id TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL,
                    last_seen INTEGER NOT NULL,
                    records_count INTEGER NOT NULL DEFAULT 0,
                    deletion_scheduled_at INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_deletion
                  ON tenants(deletion_scheduled_at)
                  WHERE deletion_scheduled_at IS NOT NULL;",
            )?;
            Ok(conn)
        })
        .await
        .context("spawn_blocking sqlite open")??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 注册新 tenant. 已存在则更新 last_seen (幂等).
    pub async fn register(&self, tenant_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute(
                "INSERT INTO tenants (tenant_id, created_at, last_seen)
                 VALUES (?1, ?2, ?2)
                 ON CONFLICT(tenant_id) DO UPDATE SET last_seen = ?2",
                params![tid, now],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking register")??;
        Ok(())
    }

    /// 检查 tenant 是否已注册. 用作"必须注册才能写"的 gate.
    pub async fn is_registered(&self, tenant_id: &str) -> Result<bool> {
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        let exists = tokio::task::spawn_blocking(move || -> Result<bool> {
            let conn = conn.blocking_lock();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE tenant_id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            Ok(count > 0)
        })
        .await
        .context("spawn_blocking is_registered")??;
        Ok(exists)
    }

    /// 看 tenant 当前状态 (供 GET /tenant/status).
    pub async fn status(&self, tenant_id: &str) -> Result<Option<TenantStatus>> {
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        let status = tokio::task::spawn_blocking(move || -> Result<Option<TenantStatus>> {
            let conn = conn.blocking_lock();
            let mut stmt = conn.prepare(
                "SELECT tenant_id, created_at, last_seen, records_count, deletion_scheduled_at
                 FROM tenants WHERE tenant_id = ?1",
            )?;
            let row = stmt
                .query_row(params![tid], |r| {
                    Ok(TenantStatus {
                        tenant_id: r.get(0)?,
                        created_at: r.get(1)?,
                        last_seen: r.get(2)?,
                        records_count: r.get(3)?,
                        deletion_scheduled_at: r.get(4)?,
                    })
                })
                .ok();
            Ok(row)
        })
        .await
        .context("spawn_blocking status")??;
        Ok(status)
    }

    /// 调整 quota 用量 (+1 / -1). 同时更新 last_seen.
    pub async fn bump_records(&self, tenant_id: &str, delta: i64) -> Result<()> {
        let tid = tenant_id.to_string();
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute(
                "UPDATE tenants
                 SET records_count = MAX(0, records_count + ?2), last_seen = ?3
                 WHERE tenant_id = ?1",
                params![tid, delta, now],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking bump_records")??;
        Ok(())
    }

    /// 申请删除 — schedule_at 是真删时刻 (now + 14d).
    pub async fn schedule_deletion(&self, tenant_id: &str, schedule_at: i64) -> Result<()> {
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute(
                "UPDATE tenants SET deletion_scheduled_at = ?2 WHERE tenant_id = ?1",
                params![tid, schedule_at],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking schedule_deletion")??;
        Ok(())
    }

    /// 取消删除申请.
    pub async fn cancel_deletion(&self, tenant_id: &str) -> Result<()> {
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute(
                "UPDATE tenants SET deletion_scheduled_at = NULL WHERE tenant_id = ?1",
                params![tid],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking cancel_deletion")??;
        Ok(())
    }

    /// 列出 deletion_scheduled_at <= now 的 tenant_id, 供 retention worker 真删用.
    pub async fn list_due_for_deletion(&self) -> Result<Vec<TenantId>> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.clone();
        let due = tokio::task::spawn_blocking(move || -> Result<Vec<TenantId>> {
            let conn = conn.blocking_lock();
            let mut stmt = conn.prepare(
                "SELECT tenant_id FROM tenants
                 WHERE deletion_scheduled_at IS NOT NULL AND deletion_scheduled_at <= ?1",
            )?;
            let ids: Vec<_> = stmt
                .query_map(params![now], |r| r.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            Ok(ids)
        })
        .await
        .context("spawn_blocking list_due_for_deletion")??;
        Ok(due)
    }

    /// 物理删除 tenant 行 (retention worker 真删 qdrant 之后再调).
    pub async fn delete_tenant(&self, tenant_id: &str) -> Result<()> {
        let tid = tenant_id.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute("DELETE FROM tenants WHERE tenant_id = ?1", params![tid])?;
            Ok(())
        })
        .await
        .context("spawn_blocking delete_tenant")??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn derive_consistent() {
        let a = derive_tenant_id("hello");
        let b = derive_tenant_id("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
    }

    #[tokio::test]
    async fn derive_different_tokens_different_ids() {
        assert_ne!(derive_tenant_id("alice"), derive_tenant_id("bob"));
    }

    #[tokio::test]
    async fn register_idempotent() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("abc123").await.unwrap();
        store.register("abc123").await.unwrap();
        assert!(store.is_registered("abc123").await.unwrap());
    }

    #[tokio::test]
    async fn unregistered_is_false() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        assert!(!store.is_registered("nope").await.unwrap());
    }

    #[tokio::test]
    async fn status_returns_none_for_unregistered() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        assert!(store.status("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn status_after_register() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("abc").await.unwrap();
        let s = store.status("abc").await.unwrap().expect("present");
        assert_eq!(s.tenant_id, "abc");
        assert_eq!(s.records_count, 0);
        assert!(s.deletion_scheduled_at.is_none());
    }

    #[tokio::test]
    async fn bump_records_increases_count() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("x").await.unwrap();
        store.bump_records("x", 3).await.unwrap();
        let s = store.status("x").await.unwrap().unwrap();
        assert_eq!(s.records_count, 3);
        store.bump_records("x", -1).await.unwrap();
        assert_eq!(store.status("x").await.unwrap().unwrap().records_count, 2);
    }

    #[tokio::test]
    async fn bump_records_never_negative() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("x").await.unwrap();
        store.bump_records("x", -100).await.unwrap();
        assert_eq!(store.status("x").await.unwrap().unwrap().records_count, 0);
    }

    #[tokio::test]
    async fn schedule_and_cancel_deletion() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("dt").await.unwrap();
        let future = chrono::Utc::now().timestamp() + 86400 * 14;
        store.schedule_deletion("dt", future).await.unwrap();
        assert_eq!(
            store.status("dt").await.unwrap().unwrap().deletion_scheduled_at,
            Some(future)
        );
        store.cancel_deletion("dt").await.unwrap();
        assert!(store
            .status("dt")
            .await
            .unwrap()
            .unwrap()
            .deletion_scheduled_at
            .is_none());
    }

    #[tokio::test]
    async fn list_due_for_deletion_returns_expired() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("expired").await.unwrap();
        store.register("future").await.unwrap();
        store.register("none").await.unwrap();

        let past = chrono::Utc::now().timestamp() - 60;
        let future = chrono::Utc::now().timestamp() + 86400;
        store.schedule_deletion("expired", past).await.unwrap();
        store.schedule_deletion("future", future).await.unwrap();

        let due = store.list_due_for_deletion().await.unwrap();
        assert!(due.contains(&"expired".to_string()));
        assert!(!due.contains(&"future".to_string()));
        assert!(!due.contains(&"none".to_string()));
    }

    #[tokio::test]
    async fn delete_tenant_removes_row() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        store.register("rm").await.unwrap();
        assert!(store.is_registered("rm").await.unwrap());
        store.delete_tenant("rm").await.unwrap();
        assert!(!store.is_registered("rm").await.unwrap());
    }
}
