//! Job 持久化抽象 + 内存实现。
//!
//! - [`JobStore`] trait: 元数据 CRUD + per-step 日志环。
//! - [`memory::InMemoryJobStore`]: P0 内存版本, 用 `Arc<RwLock<HashMap>>`。
//!   Postgres 后端按 ADR-004 P6 后期补。

use anyhow::Result;
use async_trait::async_trait;

use crate::job::{Job, JobId, StepId};
use crate::worker::LogLine;

pub mod memory;

/// Job 持久化抽象。
///
/// 当前 P0 只覆盖最基本的 CRUD + 日志追加 / 读取。后期 (P6 v2) 会加:
/// - 状态变更事件订阅 (WS push)
/// - 批量查询过滤 (status / created_at 范围)
/// - 归档 / 删除
#[async_trait]
pub trait JobStore: Send + Sync {
    /// 写入或覆盖一个 job (upsert 语义, 用 `job.id` 作 key)。
    async fn put(&self, job: Job) -> Result<()>;

    /// 按 id 取 job; 不存在返回 `Ok(None)`。
    async fn get(&self, id: &JobId) -> Result<Option<Job>>;

    /// 列出 job (按 `created_at` 倒序), 至多 `limit` 条。
    async fn list(&self, limit: u64) -> Result<Vec<Job>>;

    /// 给某 (job, step) 追加一条日志。
    async fn append_log(&self, job_id: &JobId, step_id: &StepId, line: LogLine) -> Result<()>;

    /// 取某 (job, step) 全部日志 (按插入顺序)。
    async fn get_logs(&self, job_id: &JobId, step_id: &StepId) -> Result<Vec<LogLine>>;
}
