//! frank-sync-agent REST API 同步客户端。
//!
//! # 设计动机
//!
//! frank-cli 是同步 CLI, 不引入 tokio runtime; 因此使用 `reqwest::blocking::Client`
//! 直接发请求。每个方法对应 sync-agent `routes.rs` 的一个端点, 用 serde 定义 wire
//! 结构与服务端保持对称 (在 [`wire`] 子模块)。
//!
//! # base_url 解析优先级
//!
//! 1. 显式构造时传入的 `base_url`
//! 2. 环境变量 `FRANK_SYNC_AGENT_URL`
//! 3. `~/.frank/config.toml` 中 `[sync] agent_url = "..."`
//! 4. 缺省 `http://frank.hutiefang.com:8318`
//!
//! 错误处理: API 非 2xx 时把响应 JSON 中的 `error` 字段提到 `anyhow::Error`
//! 用户消息里, 而不是吞掉只显示 status code。

pub mod wire;

use std::time::Duration;

use anyhow::{Context, Result};
use frank_memory::{MemoryId, MemoryMatch, MemoryRecord, Scope};
use serde::{Deserialize, Serialize};

use wire::{
    config_path, extract_error, normalize, resolve_base_url, AddRawRequest, AddRawResponse,
    AddRequest, AddResponse, ListRequest, ListResponse, SearchRequest, SearchResponse,
    DEFAULT_BASE_URL,
};

/// 默认请求超时 (秒)。
const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// frank-sync-agent HTTP 客户端。
///
/// 持有 base_url + 复用的 reqwest blocking client。
pub struct SyncClient {
    base_url: String,
    http: reqwest::blocking::Client,
}

impl SyncClient {
    /// 用显式 base_url 构造客户端 (例如测试)。
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .gzip(true)
            .build()
            .context("build reqwest blocking client")?;
        Ok(Self {
            base_url: normalize(&base_url.into()),
            http,
        })
    }

    /// 用环境/配置/缺省顺序解析 base_url 后构造。
    pub fn from_env_or_config() -> Result<Self> {
        Self::new(resolve_base_url(config_path().ok().as_deref()))
    }

    /// 返回当前生效的 base URL (便于诊断打印)。
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 返回内置默认 URL (静态字符串, 便于 CLI 帮助文档展示)。
    #[must_use]
    pub fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }

    /// GET /healthz — 探活。返回服务端的纯文本响应 (期望 "ok")。
    pub fn healthz(&self) -> Result<String> {
        let url = format!("{}/healthz", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let body = resp.text().context("read healthz body")?;
        if !status.is_success() {
            return Err(extract_error(status, &body));
        }
        Ok(body)
    }

    /// POST /memory/add — 通过 LLM 抽取 fact 后存入, 返回写入的 id 列表。
    pub fn add(
        &self,
        content: &str,
        scope: &Scope,
        metadata: Option<&serde_json::Value>,
    ) -> Result<Vec<MemoryId>> {
        let body = AddRequest {
            content,
            scope,
            metadata,
        };
        let resp: AddResponse = self.post_json("/memory/add", &body)?;
        Ok(resp.ids)
    }

    /// POST /memory/add_raw — 跳过 LLM, 直接写入一条 fact。
    pub fn add_raw(
        &self,
        fact: &str,
        scope: &Scope,
        metadata: Option<&serde_json::Value>,
    ) -> Result<MemoryId> {
        let body = AddRawRequest {
            fact,
            scope,
            metadata,
        };
        let resp: AddRawResponse = self.post_json("/memory/add_raw", &body)?;
        Ok(resp.id)
    }

    /// POST /memory/search — 向量检索。
    pub fn search(
        &self,
        query: &str,
        scope: &Scope,
        limit: Option<u64>,
        score_threshold: Option<f32>,
    ) -> Result<Vec<MemoryMatch>> {
        let body = SearchRequest {
            query,
            scope,
            limit,
            score_threshold,
        };
        let resp: SearchResponse = self.post_json("/memory/search", &body)?;
        Ok(resp.matches)
    }

    /// POST /memory/list — 按 scope 列出。
    pub fn list(&self, scope: &Scope, limit: u64) -> Result<Vec<MemoryRecord>> {
        let body = ListRequest { scope, limit };
        let resp: ListResponse = self.post_json("/memory/list", &body)?;
        Ok(resp.records)
    }

    /// GET /memory/:id — 取单条。服务端返回 `Option<MemoryRecord>`, None 表示找不到。
    pub fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>> {
        let url = format!("{}/memory/{}", self.base_url, id);
        let resp = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().context("read get body")?;
        if !status.is_success() {
            return Err(extract_error(status, &text));
        }
        let parsed: Option<MemoryRecord> =
            serde_json::from_str(&text).context("decode memory get response")?;
        Ok(parsed)
    }

    /// DELETE /memory/:id — 删除。成功返回 204, 这里就吞掉 status。
    pub fn delete(&self, id: &MemoryId) -> Result<()> {
        let url = format!("{}/memory/{}", self.base_url, id);
        let resp = self
            .http
            .delete(&url)
            .send()
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(extract_error(status, &body));
        }
        Ok(())
    }

    // ---- 内部辅助 ----

    fn post_json<B: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().context("read response body")?;
        if !status.is_success() {
            return Err(extract_error(status, &text));
        }
        serde_json::from_str(&text).with_context(|| format!("decode response from {path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SyncClient::new 把末尾斜杠剥掉, 路径拼接不会出现 `//`。
    #[test]
    fn client_normalizes_trailing_slash() {
        let c = SyncClient::new("http://x.test:1/").expect("build client");
        assert_eq!(c.base_url(), "http://x.test:1");
    }

    /// `default_base_url()` 返回常量 (用于帮助文档展示)。
    #[test]
    fn default_url_is_exposed() {
        assert_eq!(SyncClient::default_base_url(), DEFAULT_BASE_URL);
    }
}
