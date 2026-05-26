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

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::TryRngCore;
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
                  WHERE deletion_scheduled_at IS NOT NULL;
                CREATE TABLE IF NOT EXISTS machines (
                    machine_code TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    fingerprint_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_seen INTEGER NOT NULL,
                    FOREIGN KEY (tenant_id) REFERENCES tenants(tenant_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_machine_tenant ON machines(tenant_id);
                -- v0.14: 跨机 skill 同步表. 服务端只存 (skill 名 + git url + ref + visibility),
                -- 不存 SKILL.md 内容. 同一 tenant 不同机器 install/uninstall 时上报, 新机器 sync 拉.
                -- 隐私: 服务端仅接受 visibility ∈ {frank-official, frank-recommended} (端点层硬过滤,
                -- community/team/private/url-installed 不进表). 用户 --url 装的不上报.
                CREATE TABLE IF NOT EXISTS tenant_skills (
                    tenant_id TEXT NOT NULL,
                    skill_name TEXT NOT NULL,
                    source_url TEXT,
                    source_ref TEXT,
                    visibility TEXT NOT NULL,
                    last_seen INTEGER NOT NULL,
                    PRIMARY KEY (tenant_id, skill_name),
                    FOREIGN KEY (tenant_id) REFERENCES tenants(tenant_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_tenant_skills ON tenant_skills(tenant_id);",
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

    // ════════════════════════════════════════════════════════════════
    // v0.13.0 — server-side token + machine binding
    // ════════════════════════════════════════════════════════════════

    /// v0.13.0: provision 一台新机器 — server 端生成 token + tenant + machine 三元组.
    /// 返回 [`ProvisionResult`]. 客户端拿 token 后存盘 + 后续请求带它.
    ///
    /// # 防 spam
    /// machine_code 已存在 → 返回 Err (前端引导用户跑 `frank tenant reset` 或 `frank tenant link`).
    pub async fn provision_machine(&self, fingerprint_json: &str) -> Result<ProvisionResult> {
        let machine_code = derive_machine_code(fingerprint_json);
        let token = generate_token()?;
        let tenant_id = derive_tenant_id(&token);
        let now = chrono::Utc::now().timestamp();
        let fp = fingerprint_json.to_string();
        let mc = machine_code.clone();
        let tid = tenant_id.clone();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut conn = conn.blocking_lock();
            // 先查 machine 是否已 provisioned (防 spam)
            let existing: Option<String> = conn
                .query_row(
                    "SELECT tenant_id FROM machines WHERE machine_code = ?1",
                    params![mc],
                    |r| r.get(0),
                )
                .ok();
            if let Some(existing_tid) = existing {
                return Err(anyhow!(
                    "machine already provisioned: existing tenant_id={existing_tid}"
                ));
            }
            // 事务保证 tenants + machines 双表原子写
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO tenants (tenant_id, created_at, last_seen)
                 VALUES (?1, ?2, ?2)
                 ON CONFLICT(tenant_id) DO UPDATE SET last_seen = ?2",
                params![tid, now],
            )?;
            tx.execute(
                "INSERT INTO machines (machine_code, tenant_id, fingerprint_json, created_at, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![mc, tid, fp, now],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .context("spawn_blocking provision_machine")??;
        Ok(ProvisionResult {
            token,
            tenant_id,
            machine_code,
        })
    }

    /// v0.13.0: machine_code 已存在? 用于 client 重连
    /// (拿回 token 不可能, 但能知道这台机器已有 tenant).
    ///
    /// 当前 v0.13.0 routes 未挂端点 (留 v0.13.1 加 GET /tenant/machine/:code), 但单测要用.
    #[allow(dead_code)]
    pub async fn lookup_machine(&self, machine_code: &str) -> Result<Option<MachineInfo>> {
        let mc = machine_code.to_string();
        let conn = self.conn.clone();
        let info = tokio::task::spawn_blocking(move || -> Result<Option<MachineInfo>> {
            let conn = conn.blocking_lock();
            let mut stmt = conn.prepare(
                "SELECT machine_code, tenant_id, created_at
                 FROM machines WHERE machine_code = ?1",
            )?;
            let row = stmt
                .query_row(params![mc], |r| {
                    Ok(MachineInfo {
                        machine_code: r.get(0)?,
                        tenant_id: r.get(1)?,
                        created_at: r.get(2)?,
                    })
                })
                .ok();
            Ok(row)
        })
        .await
        .context("spawn_blocking lookup_machine")??;
        Ok(info)
    }

    /// v0.13.0: link machine to existing tenant — 用户用 `frank tenant link --token <existing>`
    /// 把额外的机器挂到一个已有 tenant 上 (多机共享同一份记忆库).
    ///
    /// 返回新机器的 `machine_code`. 若 tenant 不存在或 machine 已挂别处 → Err.
    pub async fn link_machine(&self, tenant_id: &str, fingerprint_json: &str) -> Result<String> {
        let machine_code = derive_machine_code(fingerprint_json);
        let tid = tenant_id.to_string();
        let mc = machine_code.clone();
        let fp = fingerprint_json.to_string();
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut conn = conn.blocking_lock();
            // 验 tenant 存在
            let tenant_exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE tenant_id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            if tenant_exists == 0 {
                return Err(anyhow!("tenant not found: {tid}"));
            }
            // machine 已绑别处 → Err
            let existing: Option<String> = conn
                .query_row(
                    "SELECT tenant_id FROM machines WHERE machine_code = ?1",
                    params![mc],
                    |r| r.get(0),
                )
                .ok();
            if let Some(existing_tid) = existing {
                return Err(anyhow!(
                    "machine already linked: existing tenant_id={existing_tid}"
                ));
            }
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO machines (machine_code, tenant_id, fingerprint_json, created_at, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![mc, tid, fp, now],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .context("spawn_blocking link_machine")??;
        Ok(machine_code)
    }

    // ─── v0.14 跨机 skill 同步 ───────────────────────────────────────

    /// 客户端上报"我装了这个 skill" (visibility 由调用方端点层过滤后才进表).
    ///
    /// 幂等: 同 (tenant_id, skill_name) 已存在则 UPSERT (更新 last_seen / ref / visibility).
    pub async fn report_skill(&self, tenant_id: &str, skill: &ReportedSkill) -> Result<()> {
        let conn = self.conn.clone();
        let tid = tenant_id.to_string();
        let s = skill.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let now = chrono::Utc::now().timestamp();
            let conn = conn.blocking_lock();
            conn.execute(
                "INSERT INTO tenant_skills (tenant_id, skill_name, source_url, source_ref, visibility, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(tenant_id, skill_name) DO UPDATE SET
                   source_url = excluded.source_url,
                   source_ref = excluded.source_ref,
                   visibility = excluded.visibility,
                   last_seen  = excluded.last_seen",
                rusqlite::params![tid, s.name, s.source_url, s.source_ref, s.visibility, now],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking report_skill")?
    }

    /// 客户端拉"我装过哪些" — 新机器 sync 时用. 返回按 last_seen DESC 排序.
    pub async fn list_skills(&self, tenant_id: &str) -> Result<Vec<ReportedSkill>> {
        let conn = self.conn.clone();
        let tid = tenant_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<ReportedSkill>> {
            let conn = conn.blocking_lock();
            let mut stmt = conn.prepare(
                "SELECT skill_name, source_url, source_ref, visibility
                 FROM tenant_skills WHERE tenant_id = ?1
                 ORDER BY last_seen DESC",
            )?;
            let rows = stmt
                .query_map([tid], |row| {
                    Ok(ReportedSkill {
                        name: row.get(0)?,
                        source_url: row.get(1)?,
                        source_ref: row.get(2)?,
                        visibility: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .context("spawn_blocking list_skills")?
    }

    /// 客户端 uninstall 后从 server 也撤掉.
    pub async fn forget_skill(&self, tenant_id: &str, skill_name: &str) -> Result<()> {
        let conn = self.conn.clone();
        let tid = tenant_id.to_string();
        let name = skill_name.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = conn.blocking_lock();
            conn.execute(
                "DELETE FROM tenant_skills WHERE tenant_id = ?1 AND skill_name = ?2",
                rusqlite::params![tid, name],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking forget_skill")?
    }
}

/// v0.14 跨机 skill 同步条目 (服务端 ↔ 客户端 wire format, 跟 SQLite schema 对齐).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReportedSkill {
    /// skill 名 (manifest 内唯一).
    pub name: String,
    /// git URL — 客户端 sync 重装时用. None = MCP source.
    pub source_url: Option<String>,
    /// branch/tag/sha — 跟 source_url 配对. None = 默认分支.
    pub source_ref: Option<String>,
    /// 必填; 服务端端点过滤后只可能是 `frank-official` / `frank-recommended`.
    pub visibility: String,
}

/// v0.13.0: provision 接口返回值 (server-side token + 派生 tenant_id + machine_code).
#[derive(Debug, Clone)]
pub struct ProvisionResult {
    /// 服务端生成的 base64url 32-byte 随机 token (~43 字符). 只在 provision 时返回一次.
    pub token: String,
    /// `sha256(token)[:12]` hex (与现有 `derive_tenant_id` 一致, 派生关系不变).
    pub tenant_id: String,
    /// `sha256(fingerprint_json)[:16]` hex (64 bit, 比 tenant_id 长更稳定).
    pub machine_code: String,
}

/// v0.13.0: 已注册 machine 的元信息 (lookup 接口返回).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MachineInfo {
    /// `sha256(fingerprint_json)[:16]` hex.
    pub machine_code: String,
    /// 关联的 tenant_id.
    pub tenant_id: String,
    /// 创建时间 (epoch sec).
    pub created_at: i64,
}

/// v0.13.0: 从客户端 fingerprint JSON 派生 machine_code (sha256 前 16 hex = 64 bit).
#[must_use]
pub fn derive_machine_code(fingerprint_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(fingerprint_json.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// v0.13.0: 生成 32-byte 随机 token, base64url 无 padding 编码 (~43 字符).
fn generate_token() -> Result<String> {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut buf)
        .context("OsRng.try_fill_bytes")?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
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
            store
                .status("dt")
                .await
                .unwrap()
                .unwrap()
                .deletion_scheduled_at,
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

    // ─── v0.13.0 machine-bound provisioning ───

    #[tokio::test]
    async fn provision_inserts_two_tables() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        let fp = r#"{"hostname":"laptop-a","mac":"aa:bb"}"#;
        let result = store.provision_machine(fp).await.unwrap();

        // token 是 base64url 32 byte → 43 字符 (no padding)
        assert_eq!(result.token.len(), 43);
        // tenant_id = sha256(token)[:12] = 12 hex 字符
        assert_eq!(result.tenant_id.len(), 12);
        // machine_code = sha256(fp)[:16] = 16 hex 字符
        assert_eq!(result.machine_code.len(), 16);

        // tenants 表有一行
        assert!(store.is_registered(&result.tenant_id).await.unwrap());

        // machines 表有一行
        let info = store
            .lookup_machine(&result.machine_code)
            .await
            .unwrap()
            .expect("machine row present");
        assert_eq!(info.tenant_id, result.tenant_id);
        assert_eq!(info.machine_code, result.machine_code);
    }

    #[tokio::test]
    async fn provision_duplicate_machine_rejects() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        let fp = r#"{"hostname":"laptop-b","mac":"cc:dd"}"#;
        store.provision_machine(fp).await.unwrap();

        // 同 fingerprint 二次 → Err
        let err = store.provision_machine(fp).await.unwrap_err();
        assert!(
            err.to_string().contains("already provisioned"),
            "expected duplicate err, got: {err}"
        );
    }

    #[tokio::test]
    async fn link_machine_adds_to_existing_tenant() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();

        // Machine A 先 provision
        let fp_a = r#"{"hostname":"machine-a"}"#;
        let provisioned = store.provision_machine(fp_a).await.unwrap();

        // Machine B link 到同一 tenant
        let fp_b = r#"{"hostname":"machine-b"}"#;
        let machine_b_code = store
            .link_machine(&provisioned.tenant_id, fp_b)
            .await
            .unwrap();

        // 两个 machine_code 都在 machines 表里, tenant_id 一致
        let info_a = store
            .lookup_machine(&provisioned.machine_code)
            .await
            .unwrap()
            .expect("machine A");
        let info_b = store
            .lookup_machine(&machine_b_code)
            .await
            .unwrap()
            .expect("machine B");
        assert_eq!(info_a.tenant_id, provisioned.tenant_id);
        assert_eq!(info_b.tenant_id, provisioned.tenant_id);
        assert_ne!(info_a.machine_code, info_b.machine_code);
    }

    #[tokio::test]
    async fn provision_token_is_random() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        let r1 = store
            .provision_machine(r#"{"hostname":"x1"}"#)
            .await
            .unwrap();
        let r2 = store
            .provision_machine(r#"{"hostname":"x2"}"#)
            .await
            .unwrap();
        assert_ne!(r1.token, r2.token, "tokens must be random");
        assert_ne!(r1.tenant_id, r2.tenant_id);
        assert_ne!(r1.machine_code, r2.machine_code);
    }

    #[tokio::test]
    async fn link_machine_unknown_tenant_errors() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        let err = store
            .link_machine("ghost_tenant", r#"{"hostname":"orphan"}"#)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("tenant not found"),
            "expected tenant not found err, got: {err}"
        );
    }

    #[tokio::test]
    async fn lookup_machine_returns_none_for_unknown() {
        let dir = tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("t.db")).await.unwrap();
        assert!(store
            .lookup_machine("0123456789abcdef")
            .await
            .unwrap()
            .is_none());
    }
}
