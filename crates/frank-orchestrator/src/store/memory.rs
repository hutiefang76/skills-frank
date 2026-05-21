//! 内存版本的 [`JobStore`] 实现。
//!
//! 进程级别保留 job + log, 适合 P0 单测 / 单机 dev。生产用 Postgres 后端 (P6 v2)。

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::job::{Job, JobId, StepId};
use crate::store::JobStore;
use crate::worker::LogLine;

type LogKey = (JobId, StepId);

/// 进程内的 [`JobStore`] 实现 (P0)。
///
/// 多线程安全 (内部 `RwLock`)。Clone 廉价 — 共享同一份内部状态。
#[derive(Debug, Default, Clone)]
pub struct InMemoryJobStore {
    jobs: Arc<RwLock<HashMap<JobId, Job>>>,
    logs: Arc<RwLock<HashMap<LogKey, Vec<LogLine>>>>,
}

impl InMemoryJobStore {
    /// 构造一个空 store。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn put(&self, job: Job) -> Result<()> {
        let mut g = self.jobs.write().await;
        g.insert(job.id, job);
        Ok(())
    }

    async fn get(&self, id: &JobId) -> Result<Option<Job>> {
        let g = self.jobs.read().await;
        Ok(g.get(id).cloned())
    }

    async fn list(&self, limit: u64) -> Result<Vec<Job>> {
        let g = self.jobs.read().await;
        let mut all: Vec<Job> = g.values().cloned().collect();
        all.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        all.truncate(limit);
        Ok(all)
    }

    async fn append_log(&self, job_id: &JobId, step_id: &StepId, line: LogLine) -> Result<()> {
        let mut g = self.logs.write().await;
        g.entry((*job_id, *step_id)).or_default().push(line);
        Ok(())
    }

    async fn get_logs(&self, job_id: &JobId, step_id: &StepId) -> Result<Vec<LogLine>> {
        let g = self.logs.read().await;
        Ok(g.get(&(*job_id, *step_id)).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::job::{Job, Step, StepKind};
    use crate::worker::LogLine;

    #[tokio::test]
    async fn put_get_roundtrip() {
        let s = InMemoryJobStore::new();
        let job = Job::new("hello", PathBuf::from("/tmp/jobs/1"));
        let id = job.id;
        s.put(job.clone()).await.unwrap();
        let back = s.get(&id).await.unwrap().unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.title, "hello");
    }

    #[tokio::test]
    async fn get_missing_is_none() {
        let s = InMemoryJobStore::new();
        let id = JobId::new();
        assert!(s.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_orders_by_created_at_desc_and_limits() {
        let s = InMemoryJobStore::new();
        let j1 = Job::new("first", PathBuf::from("/tmp/jobs/1"));
        // sleep 1ms not deterministic; just bump time manually
        let mut j2 = Job::new("second", PathBuf::from("/tmp/jobs/2"));
        j2.created_at = j1.created_at + chrono::Duration::seconds(1);
        s.put(j1.clone()).await.unwrap();
        s.put(j2.clone()).await.unwrap();
        let all = s.list(10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].title, "second");
        let one = s.list(1).await.unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].title, "second");
    }

    #[tokio::test]
    async fn append_and_get_logs() {
        let s = InMemoryJobStore::new();
        let job_id = JobId::new();
        let step = Step::new(StepKind::Plan, "claude", "x");
        let step_id = step.id;
        s.append_log(&job_id, &step_id, LogLine::info("hello"))
            .await
            .unwrap();
        s.append_log(&job_id, &step_id, LogLine::info("world"))
            .await
            .unwrap();
        let logs = s.get_logs(&job_id, &step_id).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message, "hello");
        assert_eq!(logs[1].message, "world");
    }

    #[tokio::test]
    async fn get_logs_missing_is_empty() {
        let s = InMemoryJobStore::new();
        let logs = s.get_logs(&JobId::new(), &StepId::new()).await.unwrap();
        assert!(logs.is_empty());
    }
}
