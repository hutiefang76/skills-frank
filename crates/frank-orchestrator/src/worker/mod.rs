//! Worker 抽象 — 不同 provider (REST / 本地 CLI / MCP) 实现同一个 trait。
//!
//! P0 只交付 [`rest::RestWorker`] (Anthropic /v1/messages 兼容);
//! 本地 CLI / MCP worker 后续按 ADR-004 补。

use std::fmt;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::job::{Step, StepOutput};

pub mod rest;

/// Worker 注册名 (例如 "claude" / "openai" / "codex-local")。
///
/// 是个 newtype, 防止业务里直接误传普通 `String` 当 provider id。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub String);

impl WorkerId {
    /// 从字面值构造 `WorkerId`。
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// 借出底层字符串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for WorkerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WorkerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// 日志级别 (跟 `tracing` 同语义, 但走自家枚举方便 JSON / WS 序列化)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// 极细粒度调试。
    Trace,
    /// 调试。
    Debug,
    /// 常规信息 (默认级)。
    Info,
    /// 警告 (可恢复异常)。
    Warn,
    /// 错误 (不可恢复)。
    Error,
}

/// 一条日志, worker 通过 `mpsc::Sender<LogLine>` 推给 executor。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// 时间戳 (UTC)。
    pub ts: DateTime<Utc>,
    /// 级别。
    pub level: LogLevel,
    /// 文本消息。
    pub message: String,
}

impl LogLine {
    /// 现在时间 + 指定 level / 消息。
    #[must_use]
    pub fn now(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            ts: Utc::now(),
            level,
            message: message.into(),
        }
    }

    /// `Info` 级简写。
    #[must_use]
    pub fn info(message: impl Into<String>) -> Self {
        Self::now(LogLevel::Info, message)
    }

    /// `Error` 级简写。
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::now(LogLevel::Error, message)
    }
}

/// 单个 provider 的执行抽象。
///
/// executor 调度时:
/// 1. 调 [`Worker::health`] 健康检查。
/// 2. 起一个 `mpsc::Sender<LogLine>`, 把 `LogLine` 串入 store。
/// 3. 调 [`Worker::run`] 跑一个 [`Step`], 拿 [`StepOutput`]。
#[async_trait]
pub trait Worker: Send + Sync {
    /// 该 worker 的 id (用于在 [`crate::Executor`] 里查找)。
    fn id(&self) -> &WorkerId;

    /// 健康检查 — 通常是 ping API / 检查 binary 是否在 `$PATH`。
    async fn health(&self) -> bool;

    /// 跑一个 step, 把日志通过 `log_tx` 推送, 返回最终 [`StepOutput`]。
    async fn run(&self, step: &Step, log_tx: mpsc::Sender<LogLine>) -> Result<StepOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_id_display() {
        let id = WorkerId::new("claude");
        assert_eq!(id.to_string(), "claude");
        assert_eq!(id.as_str(), "claude");
    }

    #[test]
    fn worker_id_from_str() {
        let id: WorkerId = "codex-local".into();
        assert_eq!(id.0, "codex-local");
    }

    #[test]
    fn log_level_serializes_snake_case() {
        let s = serde_json::to_string(&LogLevel::Warn).unwrap();
        assert_eq!(s, "\"warn\"");
    }

    #[test]
    fn log_line_info_helper_sets_level() {
        let l = LogLine::info("hi");
        assert!(matches!(l.level, LogLevel::Info));
        assert_eq!(l.message, "hi");
    }
}
