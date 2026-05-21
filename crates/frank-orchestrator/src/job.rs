//! Job / Step 数据类型与状态枚举。
//!
//! 所有类型都是 `Serialize + Deserialize`, 方便走 REST / WS / Postgres 存储。

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Job 的唯一 ID。封装 `Uuid` 防止业务里随便拼字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub Uuid);

impl JobId {
    /// 生成一个新的 v4 UUID 作为 `JobId`。
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从已有 UUID 构造 (主要用于 from-DB-row)。
    #[must_use]
    pub fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Step 的唯一 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepId(pub Uuid);

impl StepId {
    /// 生成一个新的 v4 UUID 作为 `StepId`。
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从已有 UUID 构造。
    #[must_use]
    pub fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }
}

impl Default for StepId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for StepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Job 状态机。
///
/// `Pending` → (`submit`) → `Running` → (success) → `Done` |
/// (任一 step fail) → `Failed` | (取消) → `Cancelled`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// 已创建, 尚未调度。
    Pending,
    /// 调度中, 有 step 在跑。
    Running,
    /// 所有 step 都成功。
    Done,
    /// 任一 step 失败, 后续 step skip。
    Failed,
    /// 用户主动取消。
    Cancelled,
}

/// Step 状态机, 跟 `JobStatus` 平行但 step 粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// 尚未开始。
    Pending,
    /// worker 跑中。
    Running,
    /// 成功完成。
    Done,
    /// 执行失败。
    Failed,
    /// 因前一步失败被跳过。
    Skipped,
}

/// Step 类型 (语义化的 step 角色)。`Custom` 允许业务自定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// 规划 / 拆解任务。
    Plan,
    /// 写代码 / 改文件。
    Code,
    /// 审查别人的输出。
    Review,
    /// 跑测试 / 验证。
    Test,
    /// 业务自定义。
    Custom(String),
}

/// 一次 step 的产出 (供下一步消费 / 入库)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepOutput {
    /// 文本主输出 (worker 写的内容 / CLI stdout)。
    pub stdout: String,
    /// 可选的结构化输出 (例如评分 JSON / artifact 引用)。
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub structured: serde_json::Value,
}

/// 一个 step (Job 的最小执行单元)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// 唯一 ID。
    pub id: StepId,
    /// 语义角色 (Plan / Code / Review / ...)。
    pub kind: StepKind,
    /// 投递的 provider id (worker 注册名, 例如 "claude" / "codex-local")。
    pub provider: String,
    /// 给 worker 的 prompt 主体。
    pub prompt: String,
    /// 当前状态。
    pub status: StepStatus,
    /// 产出 (尚未跑完时为 None)。
    #[serde(default)]
    pub output: Option<StepOutput>,
    /// worker 实际开始时间 (UTC)。
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// worker 实际完成时间 (UTC)。
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

impl Step {
    /// 构造一个 Pending step。
    #[must_use]
    pub fn new(kind: StepKind, provider: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: StepId::new(),
            kind,
            provider: provider.into(),
            prompt: prompt.into(),
            status: StepStatus::Pending,
            output: None,
            started_at: None,
            completed_at: None,
        }
    }
}

/// 一个完整 Job (含全部元数据 + steps)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// 唯一 ID。
    pub id: JobId,
    /// 用户起的可读标题。
    pub title: String,
    /// 创建时间 (UTC)。
    pub created_at: DateTime<Utc>,
    /// 最近一次状态变更时间 (UTC)。
    pub updated_at: DateTime<Utc>,
    /// 整体状态。
    pub status: JobStatus,
    /// step 列表 (当前 P0: 顺序执行; 后期支持 DAG)。
    pub steps: Vec<Step>,
    /// Job 工作目录 (worker `cd` 到此再跑, 各 job 隔离)。
    pub workspace_path: PathBuf,
    /// 关联到 frank-memory 的 scope。
    ///
    /// 这里用 `serde_json::Value` 占位避免循环依赖 (frank-memory ↔ frank-orchestrator),
    /// 实际字段形如 `{ "user_id": "alice", "agent_id": "claude" }`。
    #[serde(default)]
    pub memory_scope: serde_json::Value,
}

impl Job {
    /// 构造一个 Pending Job (无 steps)。
    #[must_use]
    pub fn new(title: impl Into<String>, workspace_path: PathBuf) -> Self {
        let now = Utc::now();
        Self {
            id: JobId::new(),
            title: title.into(),
            created_at: now,
            updated_at: now,
            status: JobStatus::Pending,
            steps: Vec::new(),
            workspace_path,
            memory_scope: serde_json::Value::Null,
        }
    }

    /// 追加一个 step (返回 `&mut self` 链式调用)。
    pub fn push_step(&mut self, step: Step) -> &mut Self {
        self.steps.push(step);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_serializes_snake_case() {
        let s = serde_json::to_string(&JobStatus::Running).unwrap();
        assert_eq!(s, "\"running\"");
        let s = serde_json::to_string(&JobStatus::Cancelled).unwrap();
        assert_eq!(s, "\"cancelled\"");
    }

    #[test]
    fn step_status_serializes_snake_case() {
        let s = serde_json::to_string(&StepStatus::Skipped).unwrap();
        assert_eq!(s, "\"skipped\"");
    }

    #[test]
    fn step_kind_custom_roundtrip() {
        let k = StepKind::Custom("benchmark".to_string());
        let s = serde_json::to_string(&k).unwrap();
        let back: StepKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn step_kind_plan_serializes_as_string() {
        let s = serde_json::to_string(&StepKind::Plan).unwrap();
        assert_eq!(s, "\"plan\"");
    }

    #[test]
    fn job_id_serializes_as_uuid_string() {
        let id = JobId(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap());
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, r#""550e8400-e29b-41d4-a716-446655440000""#);
    }

    #[test]
    fn job_json_roundtrip() {
        let mut job = Job::new("test", PathBuf::from("/tmp/jobs/x"));
        job.push_step(Step::new(StepKind::Plan, "claude", "draft a plan"));
        job.push_step(Step::new(
            StepKind::Code,
            "codex-local",
            "implement the plan",
        ));
        job.memory_scope = serde_json::json!({ "user_id": "alice" });

        let s = serde_json::to_string(&job).unwrap();
        let back: Job = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, job.id);
        assert_eq!(back.title, "test");
        assert_eq!(back.steps.len(), 2);
        assert_eq!(back.steps[0].provider, "claude");
        assert_eq!(back.memory_scope["user_id"], "alice");
    }

    #[test]
    fn step_new_defaults_pending() {
        let s = Step::new(StepKind::Test, "claude", "run tests");
        assert_eq!(s.status, StepStatus::Pending);
        assert!(s.output.is_none());
        assert!(s.started_at.is_none());
    }

    #[test]
    fn step_output_skips_null_structured() {
        let out = StepOutput {
            stdout: "hi".to_string(),
            structured: serde_json::Value::Null,
        };
        let s = serde_json::to_string(&out).unwrap();
        assert!(!s.contains("structured"), "got: {s}");
    }
}
