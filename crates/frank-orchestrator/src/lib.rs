//! `frank-orchestrator` — 多 AI Agent 协作总线核心库 (P6)。
//!
//! 把"一次跨多 provider 的协作任务"建模成一个 [`Job`], 由若干 [`Step`] 组成,
//! 每个 step 投递给某个 [`Worker`] (Claude / OpenAI / 本地 CLI ...) 执行。
//!
//! # 抽象层
//!
//! - [`job`] — 数据类型 ([`Job`], [`Step`], 状态枚举)
//! - [`worker`] — [`Worker`] trait + REST 实现 ([`worker::rest::RestWorker`])
//! - [`store`] — [`JobStore`] trait + 内存实现 ([`store::memory::InMemoryJobStore`])
//! - [`executor`] — [`Executor`] 调度核心: 顺序跑 step + 失败回滚
//!
//! # 设计依据
//!
//! 详见 `docs/ADR/004-frank-orchestrator.md`。
//!
//! # 质量基线
//!
//! 沿用 ADR-001: `clippy::pedantic` warn, `missing_docs` warn, `unsafe_code` forbid,
//! 每文件 < 300 行。

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod executor;
pub mod job;
pub mod store;
pub mod worker;

pub use executor::Executor;
pub use job::{Job, JobId, JobStatus, Step, StepId, StepKind, StepOutput, StepStatus};
pub use store::{memory::InMemoryJobStore, JobStore};
pub use worker::{LogLevel, LogLine, Worker, WorkerId};
