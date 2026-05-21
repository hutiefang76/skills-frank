//! 记忆数据类型: scope / record / match / search opts。
//!
//! 所有类型都是 `Serialize + Deserialize`, 方便在 REST API + Qdrant payload 双向流转。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 一条记忆的唯一 ID。封装 `Uuid` 防止业务里随便构造字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(pub Uuid);

impl MemoryId {
    /// 生成一个新的 v4 UUID 作为 MemoryId。
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

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// 记忆归属作用域: 三级嵌套 (user → agent → session)。
///
/// 任一字段 `None` 表示"不限"。检索时 `Scope` 同时作为过滤条件。
/// 至少必须指定 `user_id` (除非显式 global 查询; 后续做权限隔离时是底线)。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Scope {
    /// 用户标识 (例如 GitHub username / 邮箱)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// AI agent 标识 (例如 `claude-code` / `codex` / `openai-o1`)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// 会话标识 (例如 timestamp + 自增 / 集成 frank-orchestrator job-id)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Scope {
    /// 只填 user 的便捷构造。
    #[must_use]
    pub fn user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: Some(user_id.into()),
            ..Default::default()
        }
    }

    /// 是否完全为空 (三字段都 None) — 通常意味着全局 scope, 调用方应明确允许。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.user_id.is_none() && self.agent_id.is_none() && self.session_id.is_none()
    }
}

/// 一条完整的记忆记录 (含全部元数据)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryRecord {
    /// 唯一 ID。
    pub id: MemoryId,

    /// 自然语言事实 (例如 "user prefers vim over emacs")。
    pub content: String,

    /// 归属 scope。
    pub scope: Scope,

    /// 自由 JSON 元数据 (例如 source: "chat-2026-05-21")。
    #[serde(default, skip_serializing_if = "is_null_or_empty")]
    pub metadata: serde_json::Value,

    /// 创建时间 (UTC)。
    pub created_at: DateTime<Utc>,

    /// 最近更新时间 (UTC)。
    pub updated_at: DateTime<Utc>,
}

fn is_null_or_empty(v: &serde_json::Value) -> bool {
    v.is_null() || v.as_object().is_some_and(serde_json::Map::is_empty)
}

/// 检索结果: 一条命中 + 相似度得分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMatch {
    /// 命中的记录。
    pub record: MemoryRecord,

    /// 余弦相似度得分, [0.0, 1.0] (Qdrant 用 Cosine distance, score 越大越像)。
    pub score: f32,
}

/// 检索参数。
#[derive(Debug, Clone)]
pub struct SearchOpts {
    /// 返回前 K 条 (默认 10)。
    pub limit: u64,

    /// 余弦相似度阈值 (低于此值的过滤掉, 默认 0.5)。
    pub score_threshold: f32,

    /// 任意 metadata 过滤 (Qdrant Filter; v1 接受 serde_json::Value 由 store 转换)。
    pub filter: Option<serde_json::Value>,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            limit: 10,
            score_threshold: 0.5,
            filter: None,
        }
    }
}

impl SearchOpts {
    /// 仅指定 limit 的便捷构造。
    #[must_use]
    pub fn with_limit(limit: u64) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_id_serializes_as_uuid_string() {
        let id = MemoryId(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap());
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, r#""550e8400-e29b-41d4-a716-446655440000""#);
    }

    #[test]
    fn scope_user_builder() {
        let s = Scope::user("alice");
        assert_eq!(s.user_id.as_deref(), Some("alice"));
        assert!(s.agent_id.is_none());
        assert!(!s.is_empty());
    }

    #[test]
    fn empty_scope_detected() {
        let s = Scope::default();
        assert!(s.is_empty());
    }

    #[test]
    fn search_opts_defaults() {
        let opts = SearchOpts::default();
        assert_eq!(opts.limit, 10);
        assert!((opts.score_threshold - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_record_json_roundtrip() {
        let rec = MemoryRecord {
            id: MemoryId::new(),
            content: "user prefers vim".to_string(),
            scope: Scope::user("alice"),
            metadata: serde_json::json!({ "source": "chat-1" }),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let s = serde_json::to_string(&rec).unwrap();
        let back: MemoryRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, rec.id);
        assert_eq!(back.content, rec.content);
        assert_eq!(back.scope.user_id, rec.scope.user_id);
    }
}
