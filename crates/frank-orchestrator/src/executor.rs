//! Job 调度核心。
//!
//! P0 行为:
//! - [`Executor::submit`]: 把 job 持久化到 [`JobStore`], 状态留 `Pending`。
//! - [`Executor::run`]: 顺序跑每个 step;
//!   * 任一 step 失败 -> 后续 step 标 `Skipped`, job 落 `Failed`。
//!   * 所有 step 成功 -> job 落 `Done`。
//!   * worker 找不到 -> 当 step 失败处理 (并写 error 日志)。
//!
//! 并发 step / DAG 调度按 ADR-004 留给 P6 后期。

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Utc;
use tokio::sync::mpsc;

use crate::job::{Job, JobId, JobStatus, StepStatus};
use crate::store::JobStore;
use crate::worker::{LogLine, Worker};

/// Job 调度器。
///
/// 注册若干 [`Worker`], 然后 [`Executor::submit`] + [`Executor::run`] 推进 job。
pub struct Executor {
    workers: HashMap<String, Arc<dyn Worker>>,
    store: Arc<dyn JobStore>,
}

impl Executor {
    /// 用一个 store 起一个空 executor (没 worker 注册)。
    #[must_use]
    pub fn new(store: Arc<dyn JobStore>) -> Self {
        Self {
            workers: HashMap::new(),
            store,
        }
    }

    /// 注册一个 worker。`id` 是 step.provider 里要写的字符串。
    pub fn register(&mut self, worker: Arc<dyn Worker>) -> &mut Self {
        let id = worker.id().as_str().to_string();
        self.workers.insert(id, worker);
        self
    }

    /// 写入一个新 job 到 store, 返回它的 id。状态保持 `Pending`。
    pub async fn submit(&self, mut job: Job) -> Result<JobId> {
        job.status = JobStatus::Pending;
        job.updated_at = Utc::now();
        let id = job.id;
        self.store.put(job).await?;
        Ok(id)
    }

    /// 跑指定 job 的所有 step (顺序)。
    ///
    /// # Errors
    /// 当 job 不存在 / store 写入失败时返回 error。
    /// step 自身失败不会让 `run` 返回 error — job 状态会落 `Failed` 由调用方读出。
    pub async fn run(&self, id: &JobId) -> Result<()> {
        let mut job = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| anyhow!("job {id} not found"))?;
        job.status = JobStatus::Running;
        job.updated_at = Utc::now();
        self.store.put(job.clone()).await?;

        let mut failed = false;
        let step_count = job.steps.len();
        for idx in 0..step_count {
            if failed {
                job.steps[idx].status = StepStatus::Skipped;
                continue;
            }
            let step = job.steps[idx].clone();
            let Some(worker) = self.workers.get(&step.provider) else {
                let line = LogLine::error(format!(
                    "no worker registered for provider '{}'",
                    step.provider
                ));
                self.store.append_log(&job.id, &step.id, line).await?;
                job.steps[idx].status = StepStatus::Failed;
                failed = true;
                continue;
            };

            job.steps[idx].status = StepStatus::Running;
            job.steps[idx].started_at = Some(Utc::now());
            job.updated_at = Utc::now();
            self.store.put(job.clone()).await?;

            let (tx, mut rx) = mpsc::channel::<LogLine>(64);
            let store_clone = self.store.clone();
            let job_id = job.id;
            let step_id = step.id;
            let log_drain = tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    let _ = store_clone.append_log(&job_id, &step_id, line).await;
                }
            });

            let result = worker.run(&step, tx).await;
            // tx dropped here -> log_drain ends
            let _ = log_drain.await;

            match result {
                Ok(out) => {
                    job.steps[idx].status = StepStatus::Done;
                    job.steps[idx].output = Some(out);
                    job.steps[idx].completed_at = Some(Utc::now());
                }
                Err(e) => {
                    let line = LogLine::error(format!("worker error: {e}"));
                    self.store.append_log(&job.id, &step.id, line).await?;
                    job.steps[idx].status = StepStatus::Failed;
                    job.steps[idx].completed_at = Some(Utc::now());
                    failed = true;
                }
            }
            job.updated_at = Utc::now();
            self.store.put(job.clone()).await?;
        }

        job.status = if failed {
            JobStatus::Failed
        } else {
            JobStatus::Done
        };
        job.updated_at = Utc::now();
        self.store.put(job).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::anyhow;
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use super::*;
    use crate::job::{Job, Step, StepKind, StepOutput};
    use crate::store::memory::InMemoryJobStore;
    use crate::worker::{LogLine, Worker, WorkerId};

    /// 永远成功的 mock worker, 计数被调用次数。
    struct OkWorker {
        id: WorkerId,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Worker for OkWorker {
        fn id(&self) -> &WorkerId {
            &self.id
        }
        async fn health(&self) -> bool {
            true
        }
        async fn run(&self, step: &Step, tx: mpsc::Sender<LogLine>) -> Result<StepOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(LogLine::info(format!("ok step={}", step.id))).await;
            Ok(StepOutput {
                stdout: format!("done {}", step.prompt),
                structured: serde_json::Value::Null,
            })
        }
    }

    /// 永远失败的 mock worker。
    struct FailWorker {
        id: WorkerId,
    }

    #[async_trait]
    impl Worker for FailWorker {
        fn id(&self) -> &WorkerId {
            &self.id
        }
        async fn health(&self) -> bool {
            true
        }
        async fn run(&self, _step: &Step, _tx: mpsc::Sender<LogLine>) -> Result<StepOutput> {
            Err(anyhow!("synthetic failure"))
        }
    }

    fn make_job(providers: &[&str]) -> Job {
        let mut job = Job::new("test", PathBuf::from("/tmp/jobs/x"));
        for p in providers {
            job.push_step(Step::new(StepKind::Plan, *p, "do it"));
        }
        job
    }

    #[tokio::test]
    async fn submit_persists_pending() {
        let store = Arc::new(InMemoryJobStore::new());
        let exec = Executor::new(store.clone());
        let job = make_job(&["claude"]);
        let id = exec.submit(job).await.unwrap();
        let back = store.get(&id).await.unwrap().unwrap();
        assert!(matches!(back.status, JobStatus::Pending));
    }

    #[tokio::test]
    async fn run_two_step_job_to_done() {
        let store = Arc::new(InMemoryJobStore::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut exec = Executor::new(store.clone());
        exec.register(Arc::new(OkWorker {
            id: WorkerId::new("claude"),
            calls: calls.clone(),
        }));
        let job = make_job(&["claude", "claude"]);
        let id = exec.submit(job).await.unwrap();
        exec.run(&id).await.unwrap();
        let back = store.get(&id).await.unwrap().unwrap();
        assert!(matches!(back.status, JobStatus::Done));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for s in &back.steps {
            assert!(matches!(s.status, StepStatus::Done));
            assert!(s.output.is_some());
            assert!(s.completed_at.is_some());
        }
    }

    #[tokio::test]
    async fn run_fails_skips_remaining() {
        let store = Arc::new(InMemoryJobStore::new());
        let mut exec = Executor::new(store.clone());
        exec.register(Arc::new(FailWorker {
            id: WorkerId::new("bad"),
        }));
        exec.register(Arc::new(OkWorker {
            id: WorkerId::new("claude"),
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        let job = make_job(&["bad", "claude"]);
        let id = exec.submit(job).await.unwrap();
        exec.run(&id).await.unwrap();
        let back = store.get(&id).await.unwrap().unwrap();
        assert!(matches!(back.status, JobStatus::Failed));
        assert!(matches!(back.steps[0].status, StepStatus::Failed));
        assert!(matches!(back.steps[1].status, StepStatus::Skipped));
    }

    #[tokio::test]
    async fn run_unknown_provider_fails_step() {
        let store = Arc::new(InMemoryJobStore::new());
        let exec = Executor::new(store.clone());
        let job = make_job(&["nope"]);
        let id = exec.submit(job).await.unwrap();
        exec.run(&id).await.unwrap();
        let back = store.get(&id).await.unwrap().unwrap();
        assert!(matches!(back.status, JobStatus::Failed));
        assert!(matches!(back.steps[0].status, StepStatus::Failed));
        let logs = store.get_logs(&id, &back.steps[0].id).await.unwrap();
        assert!(logs.iter().any(|l| l.message.contains("no worker")));
    }

    #[tokio::test]
    async fn run_unknown_job_errors() {
        let store = Arc::new(InMemoryJobStore::new());
        let exec = Executor::new(store);
        let err = exec.run(&JobId::new()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn worker_log_lines_persisted() {
        let store = Arc::new(InMemoryJobStore::new());
        let mut exec = Executor::new(store.clone());
        exec.register(Arc::new(OkWorker {
            id: WorkerId::new("claude"),
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        let job = make_job(&["claude"]);
        let step_id = job.steps[0].id;
        let id = exec.submit(job).await.unwrap();
        exec.run(&id).await.unwrap();
        let logs = store.get_logs(&id, &step_id).await.unwrap();
        assert!(!logs.is_empty());
        assert!(logs.iter().any(|l| l.message.contains("ok step=")));
    }
}
