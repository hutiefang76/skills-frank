//! sync-agent HTTP wire 结构 + URL / 错误辅助函数。
//!
//! 抽出来给 `mod.rs` 瘦身 (每文件 < 300 行, ADR-001)。

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use frank_memory::{MemoryId, MemoryMatch, MemoryRecord, Scope};
use serde::{Deserialize, Serialize};

/// 默认 sync-agent base URL (生产 1Panel openresty :443 反代地址, 无端口 HTTPS)。
pub const DEFAULT_BASE_URL: &str = "https://frank.hutiefang.com";

// ---- 请求 / 响应 wire 结构 (与 sync-agent routes.rs 对称) ----

/// `POST /memory/add` 请求体。
#[derive(Serialize)]
pub struct AddRequest<'a> {
    /// 自然语言内容 (服务端将 LLM 抽取多条 fact)。
    pub content: &'a str,
    /// 归属 scope。
    pub scope: &'a Scope,
    /// 可选元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<&'a serde_json::Value>,
}

/// `POST /memory/add` 响应体。
#[derive(Deserialize)]
pub struct AddResponse {
    /// 写入的记忆 ID 列表。
    pub ids: Vec<MemoryId>,
}

/// `POST /memory/add_raw` 请求体。
#[derive(Serialize)]
pub struct AddRawRequest<'a> {
    /// 已成型的单条 fact (跳过 LLM)。
    pub fact: &'a str,
    /// 归属 scope。
    pub scope: &'a Scope,
    /// 可选元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<&'a serde_json::Value>,
}

/// `POST /memory/add_raw` 响应体。
#[derive(Deserialize)]
pub struct AddRawResponse {
    /// 写入的记忆 ID。
    pub id: MemoryId,
}

/// `POST /memory/search` 请求体。
#[derive(Serialize)]
pub struct SearchRequest<'a> {
    /// 自然语言查询。
    pub query: &'a str,
    /// 过滤 scope。
    pub scope: &'a Scope,
    /// 可选 top-K。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// 可选相似度阈值 0..1。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
}

/// `POST /memory/search` 响应体。
#[derive(Deserialize)]
pub struct SearchResponse {
    /// 命中列表 (按 score 倒序)。
    pub matches: Vec<MemoryMatch>,
}

/// `POST /memory/list` 请求体。
#[derive(Serialize)]
pub struct ListRequest<'a> {
    /// 过滤 scope。
    pub scope: &'a Scope,
    /// 最多返回条数。
    pub limit: u64,
}

/// `POST /memory/list` 响应体。
#[derive(Deserialize)]
pub struct ListResponse {
    /// 记录列表。
    pub records: Vec<MemoryRecord>,
}

/// `~/.frank/config.toml` 的 `[sync]` 节。
#[derive(Debug, Default, Deserialize)]
struct ConfigSync {
    #[serde(default)]
    agent_url: Option<String>,
}

/// 整个 `~/.frank/config.toml` 文件。
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    sync: ConfigSync,
}

/// 去掉末尾斜杠, 防止路径拼接成 `//foo`。
pub fn normalize(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

/// 把服务端 `{ "error": "..." }` 提到 anyhow 消息里, 找不到字段就用原文。
pub fn extract_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = v.get("error").and_then(serde_json::Value::as_str) {
            return anyhow!("sync-agent error ({status}): {msg}");
        }
    }
    if body.is_empty() {
        anyhow!("sync-agent error ({status}): <empty body>")
    } else {
        anyhow!("sync-agent error ({status}): {body}")
    }
}

/// 解析 base URL: env > config > default。
///
/// `config_path` 用 `Option<&Path>` 方便单测注入。
pub fn resolve_base_url(config_path: Option<&std::path::Path>) -> String {
    if let Ok(v) = std::env::var("FRANK_SYNC_AGENT_URL") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Some(path) = config_path {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(cfg) = toml::from_str::<ConfigFile>(&text) {
                if let Some(url) = cfg.sync.agent_url {
                    if !url.trim().is_empty() {
                        return url;
                    }
                }
            }
        }
    }
    DEFAULT_BASE_URL.to_string()
}

/// `~/.frank/config.toml` 路径; 与 `state::store::default_path` 同目录。
pub fn config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate user home dir")?
        .join(".frank")
        .join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没有 env、没有 config 文件时, 落到默认 URL。
    #[test]
    fn base_url_falls_back_to_default() {
        let _g = EnvGuard::unset("FRANK_SYNC_AGENT_URL");
        let url = resolve_base_url(None);
        assert_eq!(url, DEFAULT_BASE_URL);
    }

    /// env 优先于 config 与默认值。
    #[test]
    fn env_overrides_default() {
        let _g = EnvGuard::set("FRANK_SYNC_AGENT_URL", "http://example.com:9999");
        let url = resolve_base_url(None);
        assert_eq!(url, "http://example.com:9999");
    }

    /// 没有 env 时, 读 toml config。
    #[test]
    fn config_file_provides_url_when_env_missing() {
        let _g = EnvGuard::unset("FRANK_SYNC_AGENT_URL");
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        std::fs::write(
            tmp.path(),
            "[sync]\nagent_url = \"http://cfg.example:8888\"\n",
        )
        .expect("write");
        let url = resolve_base_url(Some(tmp.path()));
        assert_eq!(url, "http://cfg.example:8888");
    }

    /// 错误体里有 `error` 字段时, 会被提到 anyhow 消息里。
    #[test]
    fn extract_error_picks_up_error_field() {
        let err = extract_error(
            reqwest::StatusCode::BAD_REQUEST,
            "{\"error\":\"scope is empty\"}",
        );
        let s = format!("{err}");
        assert!(s.contains("400"), "msg: {s}");
        assert!(s.contains("scope is empty"), "msg: {s}");
    }

    /// normalize 把末尾斜杠剥掉。
    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize("http://x.test:1/"), "http://x.test:1");
        assert_eq!(normalize("http://x.test:1"), "http://x.test:1");
    }

    // ---- env 隔离助手 ----

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self {
                key,
                prev,
                _lock: lock,
            }
        }

        fn unset(key: &'static str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self {
                key,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
