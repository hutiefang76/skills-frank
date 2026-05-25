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
//! 4. 缺省 `https://frank.hutiefang.com` (1Panel openresty :443 反代)
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
/// 持有 base_url + 复用的 reqwest blocking client + 可选鉴权 token。
/// sync-agent 在 Caddy 层强制 `X-Frank-Token` header 校验, 客户端不带 token 会 401。
pub struct SyncClient {
    base_url: String,
    http: reqwest::blocking::Client,
    token: Option<String>,
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
            token: None,
        })
    }

    /// 设置 X-Frank-Token 鉴权头 (后续所有 request 自动带上)。
    #[must_use]
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// 用环境/配置/缺省顺序解析 base_url + 鉴权 token 后构造。
    ///
    /// Token 来源 (按优先级):
    /// 1. 环境变量 `FRANK_API_TOKEN`
    /// 2. `~/.frank/.token` 文件 (单行明文, 600 权限推荐)
    ///
    /// v0.10.10: 如果落到默认公共 server (`frank.hutiefang.com`), 首次会打 demo
    /// 模式 warning, 提示用户数据未严格隔离. 后续 v0.11 加用户隔离后撤掉.
    pub fn from_env_or_config() -> Result<Self> {
        let base = resolve_base_url(config_path().ok().as_deref());
        // demo warning — 仅在用默认公共 server 时打, 每进程一次
        if base.trim_end_matches('/') == DEFAULT_BASE_URL.trim_end_matches('/') {
            print_demo_warning_once();
        }
        let mut client = Self::new(base)?;
        if let Some(t) = resolve_token() {
            client = client.with_token(t);
        }
        Ok(client)
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
    /// healthz 不鉴权, 不带 token。
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
        let mut req = self.http.get(&url);
        if let Some(t) = &self.token {
            req = req.header("X-Frank-Token", t);
        }
        let resp = req.send().with_context(|| format!("GET {url}"))?;
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
        let mut req = self.http.delete(&url);
        if let Some(t) = &self.token {
            req = req.header("X-Frank-Token", t);
        }
        let resp = req.send().with_context(|| format!("DELETE {url}"))?;
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
        let mut req = self.http.post(&url).json(body);
        if let Some(t) = &self.token {
            req = req.header("X-Frank-Token", t);
        }
        let resp = req.send().with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().context("read response body")?;
        if !status.is_success() {
            return Err(extract_error(status, &text));
        }
        serde_json::from_str(&text).with_context(|| format!("decode response from {path}"))
    }
}

/// v0.10.10: 默认公共 server demo 模式 warning. 每进程只打一次, 用 OnceLock 守.
/// 通过 `FRANK_SUPPRESS_DEMO_WARN=1` 或 config `sync.demo_acknowledged = true` 抑制.
fn print_demo_warning_once() {
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_err() {
        return;
    }
    // env 抑制
    if std::env::var("FRANK_SUPPRESS_DEMO_WARN").ok().as_deref() == Some("1") {
        return;
    }
    // config 抑制 (sync.demo_acknowledged = true; 支持 bool 和字符串 "true")
    if let Ok(path) = config_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = text.parse::<toml::Value>() {
                if let Some(val) = v.get("sync").and_then(|s| s.get("demo_acknowledged")) {
                    let is_true = val.as_bool() == Some(true)
                        || val.as_str().is_some_and(|s| s.eq_ignore_ascii_case("true"));
                    if is_true {
                        return;
                    }
                }
            }
        }
    }
    crate::log::ui::warn("当前连接公共 demo 服务器 frank.hutiefang.com");
    crate::log::ui::info("  ⚠️  v0.10.10 暂未做用户隔离, 数据混在同一库, 仅 demo 用");
    crate::log::ui::info("  隐私敏感请自建:");
    crate::log::ui::info("    curl -sSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/deploy/install-server.sh | bash");
    crate::log::ui::info("  已自建请配置:");
    crate::log::ui::info("    frank config set sync.agent_url http://<your-server>:8318");
    crate::log::ui::info("  不再提醒 (接受 demo 模式):");
    crate::log::ui::info("    frank config set sync.demo_acknowledged true");
    eprintln!();
}

/// Token 解析: env `FRANK_API_TOKEN` → `~/.frank/.token` 文件。
fn resolve_token() -> Option<String> {
    if let Ok(t) = std::env::var("FRANK_API_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    let path = dirs::home_dir()?.join(".frank").join(".token");
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
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
