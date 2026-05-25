//! `frank orchestrator serve` — P6 Milestone 2 本机 daemon。
//!
//! 起一个 axum HTTP server, 暴露 REST + WebSocket, 让浏览器实时看到多 Job 协作。
//!
//! # 为什么是"本机"而不是远程 sync-agent
//!
//! LocalCliWorker 要 spawn 本机 claude / codex / opencode 子进程; 这些 CLI 不在
//! tx (服务端) 上, 必须跑用户本机. 所以这个 daemon 跑用户机器, 默认 :7780, 浏览器
//! 直接连 localhost. 远程 sync-agent (tx:8318) 只管 frank-memory 跨设备同步.
//!
//! # 路由
//!
//! - `GET /` — 嵌入式静态前端 (HTML + vanilla JS, 不依赖任何打包工具)
//! - `POST /jobs` — 提交 Job (body: { provider, prompt, timeout? }, 返回 job_id)
//! - `GET /jobs` — 列出全部 Job (状态汇总)
//! - `GET /jobs/:id` — 单个 Job 详情 + 累积 log
//! - `GET /jobs/:id/stream` — WebSocket, 实时推 log 行
//!
//! # 并发隔离 (多任务不串)
//!
//! 每个 POST /jobs 都 `tokio::spawn` 独立 task 跑 LocalCliWorker, OS pid 级隔离.
//! N 个 Job 同时跑 = N 个 subprocess + N 个 broadcast channel, 互不交叉.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{ws::WebSocketUpgrade, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use frank_orchestrator::worker::local::{CliProvider, LocalCliWorker};
use frank_orchestrator::worker::{LogLine, Worker};
use frank_orchestrator::{JobId, Step, StepId, StepKind, StepStatus};
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::CorsLayer;

const INDEX_HTML: &str = include_str!("orchestrator_index.html");

/// 单 Job 服务端状态。
#[derive(Clone)]
struct JobEntry {
    id: JobId,
    provider: String,
    prompt: String,
    status: String,
    created_at: DateTime<Utc>,
    stdout: Arc<RwLock<String>>,
    logs: Arc<RwLock<Vec<LogLine>>>,
    /// 给 WebSocket 客户端订阅的实时 log 广播。
    log_bus: broadcast::Sender<LogLine>,
}

/// 服务端共享状态。
#[derive(Clone)]
struct AppState {
    jobs: Arc<RwLock<HashMap<JobId, JobEntry>>>,
}

#[derive(Deserialize)]
struct SubmitReq {
    provider: String,
    prompt: String,
    #[serde(default)]
    timeout: Option<u64>,
    /// 可选 model (例 `opus`, `gpt-5.5`, `mimo-v2.5-pro`)。空时各家 CLI 用自家默认。
    #[serde(default)]
    model: Option<String>,
}

#[derive(Serialize)]
struct SubmitResp {
    job_id: JobId,
}

#[derive(Serialize)]
struct JobSummary {
    id: JobId,
    provider: String,
    status: String,
    created_at: DateTime<Utc>,
    prompt_excerpt: String,
}

#[derive(Serialize)]
struct JobDetail {
    id: JobId,
    provider: String,
    prompt: String,
    status: String,
    created_at: DateTime<Utc>,
    stdout: String,
    logs: Vec<LogLineWire>,
}

#[derive(Serialize, Clone)]
struct LogLineWire {
    ts: DateTime<Utc>,
    level: String,
    message: String,
}

impl From<LogLine> for LogLineWire {
    fn from(l: LogLine) -> Self {
        Self {
            ts: l.ts,
            level: format!("{:?}", l.level).to_lowercase(),
            message: l.message,
        }
    }
}

/// 启动 axum daemon, 阻塞直到 ctrl-c。
pub async fn serve(addr: SocketAddr) -> Result<()> {
    let state = AppState {
        jobs: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/providers", get(list_providers))
        .route("/jobs", get(list_jobs).post(submit_job))
        .route("/jobs/:id", get(get_job))
        .route("/jobs/:id/stream", get(ws_stream))
        // v0.10.0: Skill 管理 REST (复用 frank-cli library 模块)
        .route("/api/skills", get(api_list_skills).post(api_install_skill))
        .route("/api/skills/:name", delete(api_uninstall_skill))
        .route("/api/skills/:name/enable", post(api_enable_skill))
        .route("/api/skills/:name/disable", post(api_disable_skill))
        // v0.10.3: Memory 浏览 REST (复用 sync_client → sync-agent)
        .route("/api/memory/list", post(api_memory_list))
        .route("/api/memory/search", post(api_memory_search))
        .route("/api/memory/add_raw", post(api_memory_add_raw))
        .route("/api/memory/:id", delete(api_memory_delete))
        // v0.10.7 D7: AI 历史浏览 REST (本地 ~/.frank/ai_history.jsonl, 不走 sync-agent)
        .route("/api/ai-history/list", get(api_ai_hist_list))
        .route("/api/ai-history/export", get(api_ai_hist_export))
        .route("/api/ai-history", delete(api_ai_hist_delete_before))
        .route(
            "/api/ai-history/:id",
            get(api_ai_hist_show).delete(api_ai_hist_delete),
        )
        .layer(CorsLayer::permissive())
        .with_state(state);

    crate::log::ui::section(&format!(
        "frank orchestrator daemon listening on http://{addr}"
    ));
    crate::log::ui::info(&format!("→ 浏览器打开 http://{addr}"));
    crate::log::ui::info("→ Ctrl-C 退出");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// `/providers` 响应: 每家 CLI 的可用性 + 本机已配置的 model 列表。
#[derive(Serialize)]
struct ProviderInfo {
    name: String,
    available: bool,
    models: Vec<String>,
}

/// `GET /providers` — 实时探测本机有哪些 AI CLI + 列出可用 model。
///
/// 不再硬编码 "Pro / Plus / setup-token" 这种用户套餐名 (用户隐私 / 不一定对).
/// 替换成 `which <bin>` + 各家自己列 model 的方式.
async fn list_providers() -> Json<Vec<ProviderInfo>> {
    let names = ["claude", "codex", "opencode", "gemini"];
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let available = which::which(name).is_ok();
        let models = if available {
            detect_models(name)
        } else {
            Vec::new()
        };
        out.push(ProviderInfo {
            name: name.to_string(),
            available,
            models,
        });
    }
    Json(out)
}

/// 每家 CLI 的 model 列表来源 — **只读本机文件 / 硬编码 alias, 不 spawn 任何子进程**。
///
/// 之前版本跑 `opencode models` 子进程, opencode 初始化时扫照片/音乐库/网络宗卷,
/// macOS TCC 弹一堆 "frank 想访问 ..." 权限对话框 + daemon 卡死. 用户原话:
/// "你干嘛了又在要访问权限?"
///
/// 现在所有 4 家全部 zero-IO 路径:
fn detect_models(provider: &str) -> Vec<String> {
    match provider {
        // Anthropic 文档标准 alias (`claude --help`: "alias 'sonnet' or 'opus'")
        "claude" => vec!["opus".into(), "sonnet".into(), "haiku".into()],
        // codex 自家 cache 文件 (用户跑 codex login 后 CLI 写的, frank 只读)
        "codex" => read_codex_models(),
        // gemini CLI 没列模型命令, Google 公开 alias
        "gemini" => vec![
            "gemini-2.5-pro".into(),
            "gemini-2.5-flash".into(),
            "gemini-2.0-flash".into(),
        ],
        // opencode 没有本地 model cache 文件, 也不能 spawn `opencode models` (会触发 TCC).
        // 返回空 → UI 提示"用 CLI 默认 model 或手输 model 名".
        // 用户跑 `opencode models` 看 model 名 (在自己终端, 跟 daemon 隔离).
        // _ 分支 fallthrough: opencode + 其他未识别 provider 都给空, 用户手输.
        _ => Vec::new(),
    }
}

fn read_codex_models() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".codex").join("models_cache.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("models")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("slug").and_then(serde_json::Value::as_str))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

async fn list_jobs(State(s): State<AppState>) -> Json<Vec<JobSummary>> {
    let jobs = s.jobs.read().await;
    let mut out: Vec<JobSummary> = jobs
        .values()
        .map(|j| JobSummary {
            id: j.id,
            provider: j.provider.clone(),
            status: j.status.clone(),
            created_at: j.created_at,
            prompt_excerpt: j.prompt.chars().take(80).collect(),
        })
        .collect();
    out.sort_by_key(|j| std::cmp::Reverse(j.created_at));
    Json(out)
}

async fn submit_job(
    State(s): State<AppState>,
    Json(req): Json<SubmitReq>,
) -> Result<Json<SubmitResp>, (StatusCode, String)> {
    let provider =
        parse_provider(&req.provider).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let mut worker = LocalCliWorker::new(provider).with_timeout(req.timeout.unwrap_or(300));
    if let Some(model) = req.model.as_ref().filter(|m| !m.trim().is_empty()) {
        worker = worker.with_model(model);
    }
    if !worker.health().await {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("`{}` CLI 不在 PATH; 跑 which 自查", req.provider),
        ));
    }

    let id = JobId::new();
    let (log_bus, _) = broadcast::channel::<LogLine>(256);
    let entry = JobEntry {
        id,
        provider: req.provider.clone(),
        prompt: req.prompt.clone(),
        status: "running".to_string(),
        created_at: Utc::now(),
        stdout: Arc::new(RwLock::new(String::new())),
        logs: Arc::new(RwLock::new(Vec::new())),
        log_bus: log_bus.clone(),
    };
    s.jobs.write().await.insert(id, entry.clone());

    let s_clone = s.clone();
    tokio::spawn(async move {
        run_job(s_clone, id, worker, req.prompt, log_bus).await;
    });

    Ok(Json(SubmitResp { job_id: id }))
}

async fn run_job(
    state: AppState,
    id: JobId,
    worker: LocalCliWorker,
    prompt: String,
    log_bus: broadcast::Sender<LogLine>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LogLine>(64);

    // 多路放 log: 写 jobs.logs 持久 + 广播到 ws 订阅者
    let jobs = state.jobs.clone();
    let log_bus_clone = log_bus.clone();
    let log_persist_task = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            // 写持久 log
            if let Some(entry) = jobs.read().await.get(&id) {
                entry.logs.write().await.push(line.clone());
            }
            // 广播 (ws 客户端订阅)
            let _ = log_bus_clone.send(line);
        }
    });

    let step = Step {
        id: StepId::new(),
        kind: StepKind::Custom("demo".to_string()),
        provider: worker.id().as_str().to_string(),
        prompt,
        status: StepStatus::Running,
        output: None,
        started_at: Some(Utc::now()),
        completed_at: None,
    };
    let result = worker.run(&step, tx).await;
    log_persist_task.abort();

    let mut jobs = state.jobs.write().await;
    if let Some(entry) = jobs.get_mut(&id) {
        match result {
            Ok(output) => {
                entry.status = "done".to_string();
                *entry.stdout.write().await = output.stdout;
                let _ = log_bus.send(LogLine::info("[job done]"));
            }
            Err(e) => {
                entry.status = "failed".to_string();
                let _ = log_bus.send(LogLine::error(format!("[job failed] {e:#}")));
            }
        }
    }
}

async fn get_job(
    State(s): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<JobDetail>, StatusCode> {
    // 先拿到 entry 的克隆 (release jobs lock), 再读子字段避免锁嵌套生命周期问题
    let entry = {
        let jobs = s.jobs.read().await;
        jobs.get(&JobId::from_uuid(id))
            .cloned()
            .ok_or(StatusCode::NOT_FOUND)?
    };
    let stdout = entry.stdout.read().await.clone();
    let logs: Vec<LogLineWire> = entry
        .logs
        .read()
        .await
        .iter()
        .cloned()
        .map(LogLineWire::from)
        .collect();
    Ok(Json(JobDetail {
        id: entry.id,
        provider: entry.provider.clone(),
        prompt: entry.prompt.clone(),
        status: entry.status.clone(),
        created_at: entry.created_at,
        stdout,
        logs,
    }))
}

async fn ws_stream(
    ws: WebSocketUpgrade,
    Path(id): Path<uuid::Uuid>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let jobs = s.jobs.read().await;
    let Some(entry) = jobs.get(&JobId::from_uuid(id)) else {
        return (StatusCode::NOT_FOUND, "job not found").into_response();
    };
    let mut rx = entry.log_bus.subscribe();
    let history: Vec<LogLine> = entry.logs.read().await.clone();
    drop(jobs);

    ws.on_upgrade(move |socket| async move {
        let (mut sender, _receiver) = socket.split();

        // 先把历史 log 推一遍 (重连客户端能看完整)
        for line in history {
            let wire: LogLineWire = line.into();
            if let Ok(json) = serde_json::to_string(&wire) {
                if sender
                    .send(axum::extract::ws::Message::Text(json))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }

        // 然后流式推新 log
        while let Ok(line) = rx.recv().await {
            let wire: LogLineWire = line.into();
            if let Ok(json) = serde_json::to_string(&wire) {
                if sender
                    .send(axum::extract::ws::Message::Text(json))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    })
    .into_response()
}

fn parse_provider(s: &str) -> anyhow::Result<CliProvider> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(CliProvider::Claude),
        "codex" => Ok(CliProvider::Codex),
        "opencode" => Ok(CliProvider::Opencode),
        "gemini" => Ok(CliProvider::Gemini),
        other => anyhow::bail!("unknown provider `{other}`"),
    }
}

// 给 futures_util::StreamExt::split 用
use futures_util::stream::StreamExt;

// ============================================================
// v0.10.0: Web UI Skill 管理 REST handlers
// ============================================================
//
// 设计:
// - 全部走 spawn_blocking (frank-cli 同步 API 不是 async)
// - 失败 → 400 + JSON { ok: false, error: "..." }
// - 成功 → 200 + JSON { ok: true, ... }

#[derive(Serialize)]
struct SkillRow {
    name: String,
    visibility: String,
    source_ref: String,
    enabled: bool,
    platforms: Vec<String>,
    installed: bool,
}

#[derive(Serialize)]
struct OkResp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct InstallReq {
    /// skill name (走 manifest) 或者 url 模式时为空
    #[serde(default)]
    name: Option<String>,
    /// 任意 git url (走 install --url 模式)
    #[serde(default)]
    url: Option<String>,
    /// git ref (默认 main)
    #[serde(default)]
    r#ref: Option<String>,
    /// 已装也强行覆盖
    #[serde(default)]
    force: bool,
    /// 升级 (保留 installed_at)
    #[serde(default)]
    upgrade: bool,
}

/// `GET /api/skills` — 列 manifest skills + state.json 真装状态。
async fn api_list_skills() -> Result<Json<Vec<SkillRow>>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(|| -> anyhow::Result<Vec<SkillRow>> {
        let manifests = crate::manifest::parser::discover()?;
        let skills = crate::manifest::parser::merge(manifests);
        let state = crate::state::State::load_default().unwrap_or_else(|_| {
            // load fail → fake empty state, list 不至于挂
            crate::state::State::load(std::path::PathBuf::from("/dev/null")).unwrap()
        });
        Ok(skills
            .iter()
            .map(|s| {
                let installed = state.get(&s.name).is_some();
                let (sref, plats, enabled) =
                    state
                        .get(&s.name)
                        .map_or((String::new(), Vec::new(), false), |st| {
                            (
                                st.source_ref.chars().take(7).collect::<String>(),
                                st.platforms.iter().map(|p| format!("{p:?}")).collect(),
                                st.enabled,
                            )
                        });
                SkillRow {
                    name: s.name.clone(),
                    visibility: format!("{:?}", s.visibility),
                    source_ref: sref,
                    enabled,
                    platforms: plats,
                    installed,
                }
            })
            .collect())
    })
    .await
    .map_err(internal_err)?
    .map(Json)
    .map_err(handler_err)
}

/// `POST /api/skills` — 装 skill (body: { name, url?, ref?, force?, upgrade? })。
async fn api_install_skill(
    Json(req): Json<InstallReq>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        crate::cli::install::run(crate::cli::install::Args {
            name: req.name,
            all: false,
            profile: None,
            skip_health_check: false,
            force: req.force,
            upgrade: req.upgrade,
            url: req.url,
            r#ref: req.r#ref,
        })
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

/// `DELETE /api/skills/:name` — 单卸一个 skill。
async fn api_uninstall_skill(
    Path(name): Path<String>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        crate::cli::uninstall::run(crate::cli::uninstall::Args {
            name: Some(name),
            including_3rd_party: false,
            keep_cache: false,
        })
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

/// `POST /api/skills/:name/enable` — 重建链接。
async fn api_enable_skill(
    Path(name): Path<String>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        crate::cli::enable::run(crate::cli::enable::Args { name })
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

/// `POST /api/skills/:name/disable` — 移链接但保留 state。
async fn api_disable_skill(
    Path(name): Path<String>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        crate::cli::disable::run(crate::cli::disable::Args { name })
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

// ============================================================
// v0.10.3: Memory 浏览 REST handlers
// ============================================================

#[derive(Deserialize)]
struct MemListReq {
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default = "default_mem_limit")]
    limit: u64,
}
fn default_mem_limit() -> u64 {
    50
}

#[derive(Deserialize)]
struct MemSearchReq {
    query: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default = "default_mem_limit")]
    limit: u64,
    #[serde(default)]
    score_threshold: Option<f32>,
}

#[derive(Deserialize)]
struct MemAddRawReq {
    fact: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    session: Option<String>,
}

fn scope_from(
    user: Option<String>,
    agent: Option<String>,
    session: Option<String>,
) -> frank_memory::Scope {
    frank_memory::Scope {
        user_id: user,
        agent_id: agent,
        session_id: session,
    }
}

/// `POST /api/memory/list` body: { user?, agent?, session?, limit }
async fn api_memory_list(
    Json(req): Json<MemListReq>,
) -> Result<Json<Vec<frank_memory::MemoryRecord>>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<frank_memory::MemoryRecord>> {
        let client = crate::sync_client::SyncClient::from_env_or_config()?;
        let scope = scope_from(req.user, req.agent, req.session);
        client.list(&scope, req.limit)
    })
    .await
    .map_err(internal_err)?
    .map(Json)
    .map_err(handler_err)
}

/// `POST /api/memory/search` body: { query, user?, agent?, session?, limit?, score_threshold? }
async fn api_memory_search(
    Json(req): Json<MemSearchReq>,
) -> Result<Json<Vec<frank_memory::MemoryMatch>>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<frank_memory::MemoryMatch>> {
        let client = crate::sync_client::SyncClient::from_env_or_config()?;
        let scope = scope_from(req.user, req.agent, req.session);
        client.search(&req.query, &scope, Some(req.limit), req.score_threshold)
    })
    .await
    .map_err(internal_err)?
    .map(Json)
    .map_err(handler_err)
}

/// `POST /api/memory/add_raw` body: { fact, user?, agent?, session? }
async fn api_memory_add_raw(
    Json(req): Json<MemAddRawReq>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let client = crate::sync_client::SyncClient::from_env_or_config()?;
        let scope = scope_from(req.user, req.agent, req.session);
        client.add_raw(&req.fact, &scope, None)?;
        Ok(())
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

/// `DELETE /api/memory/:id` — 单删 record
async fn api_memory_delete(
    Path(id_str): Path<String>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let client = crate::sync_client::SyncClient::from_env_or_config()?;
        // MemoryId 是 uuid::Uuid 包装, 走 serde 反序列化
        let quoted = serde_json::to_string(&id_str).expect("string always serializable");
        let id: frank_memory::MemoryId = serde_json::from_str(&quoted)
            .map_err(|e| anyhow::anyhow!("invalid memory id `{id_str}`: {e}"))?;
        client.delete(&id)
    })
    .await
    .map_err(internal_err)?
    .map(|()| {
        Json(OkResp {
            ok: true,
            error: None,
        })
    })
    .map_err(handler_err)
}

fn internal_err(e: tokio::task::JoinError) -> (StatusCode, Json<OkResp>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(OkResp {
            ok: false,
            error: Some(format!("task join: {e}")),
        }),
    )
}

fn handler_err(e: anyhow::Error) -> (StatusCode, Json<OkResp>) {
    (
        StatusCode::BAD_REQUEST,
        Json(OkResp {
            ok: false,
            error: Some(format!("{e:#}")),
        }),
    )
}

// ============================================================
// v0.10.7 D7: AI 历史 REST handlers
//
// 5 个端点, 全部 spawn_blocking 调 HistoryStore (因为 fs 操作 + fs2 锁是 blocking).
// 数据全在本地 ~/.frank/ai_history.jsonl + ai-history-full/, 不走 sync-agent.
// ============================================================

use crate::cli::ai::history_store::{FullRecord, HistoryEntry, HistoryStore, ListFilter};

/// `GET /api/ai-history/list?provider=&status=&since=&limit=&offset=`
///
/// query string 而非 POST body — list 是只读, GET 更符合 REST, 浏览器能保存历史.
#[derive(Deserialize)]
struct AiHistListQuery {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    status: Option<String>,
    /// ISO-8601 或 YYYY-MM-DD
    #[serde(default)]
    since: Option<String>,
    /// 取多少条, 默认 200 (几年下来超过几万再上分页)
    #[serde(default = "default_ai_hist_limit")]
    limit: usize,
    /// 偏移 (跳过前 N 条, 0 = 从最新开始)
    #[serde(default)]
    offset: usize,
}
fn default_ai_hist_limit() -> usize {
    200
}

async fn api_ai_hist_list(
    axum::extract::Query(q): axum::extract::Query<AiHistListQuery>,
) -> Result<Json<Vec<HistoryEntry>>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<HistoryEntry>> {
        let since = match q.since.as_deref() {
            Some(s) => Some(parse_date_loose(s)?),
            None => None,
        };
        let filter = ListFilter {
            provider: q.provider,
            status: q.status,
            since,
            cwd: None,
            // 拉全表, 再 skip+take (ListFilter::limit 是 take 不是 skip+take)
            limit: None,
        };
        let mut entries = HistoryStore::list(&filter)?;
        if q.offset > 0 {
            entries = entries.into_iter().skip(q.offset).collect();
        }
        if entries.len() > q.limit {
            entries.truncate(q.limit);
        }
        Ok(entries)
    })
    .await
    .map_err(internal_err)?
    .map(Json)
    .map_err(handler_err)
}

/// `GET /api/ai-history/:id` — 看一条全文.
async fn api_ai_hist_show(
    Path(id): Path<String>,
) -> Result<Json<FullRecord>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<FullRecord> { HistoryStore::show(&id) })
        .await
        .map_err(internal_err)?
        .map(Json)
        .map_err(handler_err)
}

/// `DELETE /api/ai-history/:id` — 单删一条 (索引行 + 全文文件).
async fn api_ai_hist_delete(
    Path(id): Path<String>,
) -> Result<Json<OkResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> { HistoryStore::delete(&id) })
        .await
        .map_err(internal_err)?
        .map(|()| {
            Json(OkResp {
                ok: true,
                error: None,
            })
        })
        .map_err(handler_err)
}

/// `DELETE /api/ai-history?before=YYYY-MM-DD` — 批删时间之前的全部.
#[derive(Deserialize)]
struct AiHistBeforeQuery {
    before: String,
}

/// 批删响应: 在通用 OkResp 之外多一个 `deleted_count` 字段.
#[derive(Serialize)]
struct AiHistDeletedResp {
    ok: bool,
    deleted_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn api_ai_hist_delete_before(
    axum::extract::Query(q): axum::extract::Query<AiHistBeforeQuery>,
) -> Result<Json<AiHistDeletedResp>, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let cutoff = parse_date_loose(&q.before)?;
        HistoryStore::delete_before(cutoff)
    })
    .await
    .map_err(internal_err)?
    .map(|n| {
        Json(AiHistDeletedResp {
            ok: true,
            deleted_count: n,
            error: None,
        })
    })
    .map_err(handler_err)
}

/// `GET /api/ai-history/export?format=jsonl|md` — 全量导出.
#[derive(Deserialize)]
struct AiHistExportQuery {
    #[serde(default = "default_export_format")]
    format: String,
}
fn default_export_format() -> String {
    "jsonl".to_string()
}

async fn api_ai_hist_export(
    axum::extract::Query(q): axum::extract::Query<AiHistExportQuery>,
) -> Result<String, (StatusCode, Json<OkResp>)> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        HistoryStore::export(&q.format)
    })
    .await
    .map_err(internal_err)?
    .map_err(handler_err)
}

/// 宽松解析日期 (跟 CLI 那边一样, 接受 `YYYY-MM-DD` 或 ISO-8601).
///
/// 复制一份小函数, 不引入 `cli::ai::parse_date_loose` 是因为它是私有的;
/// 公开化会增加 module API 表面, 不值. 5 行的副本更轻.
fn parse_date_loose(s: &str) -> anyhow::Result<DateTime<Utc>> {
    if let Ok(t) = s.parse::<DateTime<Utc>>() {
        return Ok(t);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let naive = d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid YYYY-MM-DD time"))?;
        return Ok(chrono::TimeZone::from_utc_datetime(&Utc, &naive));
    }
    anyhow::bail!("解析不动日期 `{s}` (支持 YYYY-MM-DD 或 ISO-8601)")
}

#[cfg(test)]
mod ai_hist_rest_tests {
    use super::*;

    /// 串行化 HOME mutation — 复用 history_store 的全局锁, 避免跟那边的测试互撞.
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let td = tempfile::tempdir().expect("tempdir");
        let old = std::env::var_os("HOME");
        std::env::set_var("HOME", td.path());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Some(o) = old {
            std::env::set_var("HOME", o);
        } else {
            std::env::remove_var("HOME");
        }
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }

    fn sample(id: &str, to: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.to_string(),
            ts: Utc::now().to_rfc3339(),
            from: "test".to_string(),
            to: to.to_string(),
            source_cwd: None,
            source_tag: None,
            model: None,
            prompt_excerpt: "q".to_string(),
            response_excerpt: "a".to_string(),
            status: "ok".to_string(),
            error: None,
            latency_ms: 0,
        }
    }

    #[test]
    fn list_filter_by_provider_via_store() {
        with_temp_home(|| {
            HistoryStore::append(&sample(&HistoryStore::new_id(), "claude"), "q", "a").unwrap();
            HistoryStore::append(&sample(&HistoryStore::new_id(), "codex"), "q", "a").unwrap();
            let f = ListFilter {
                provider: Some("claude".to_string()),
                ..Default::default()
            };
            let r = HistoryStore::list(&f).unwrap();
            assert_eq!(r.len(), 1);
            assert_eq!(r[0].to, "claude");
        });
    }

    #[test]
    fn show_via_store() {
        with_temp_home(|| {
            let id = HistoryStore::new_id();
            HistoryStore::append(&sample(&id, "claude"), "完整 Q", "完整 A").unwrap();
            let full = HistoryStore::show(&id).unwrap();
            assert_eq!(full.prompt, "完整 Q");
            assert_eq!(full.response, "完整 A");
        });
    }

    #[test]
    fn delete_via_store() {
        with_temp_home(|| {
            let id = HistoryStore::new_id();
            HistoryStore::append(&sample(&id, "claude"), "q", "a").unwrap();
            HistoryStore::delete(&id).unwrap();
            assert_eq!(HistoryStore::list(&ListFilter::default()).unwrap().len(), 0);
        });
    }

    #[test]
    fn delete_before_batch_via_store() {
        with_temp_home(|| {
            HistoryStore::append(&sample(&HistoryStore::new_id(), "claude"), "q", "a").unwrap();
            HistoryStore::append(&sample(&HistoryStore::new_id(), "codex"), "q", "a").unwrap();
            let cutoff = parse_date_loose("2099-12-31").unwrap();
            let n = HistoryStore::delete_before(cutoff).unwrap();
            assert_eq!(n, 2);
        });
    }

    #[test]
    fn export_jsonl_via_store() {
        with_temp_home(|| {
            HistoryStore::append(&sample(&HistoryStore::new_id(), "claude"), "q", "a").unwrap();
            let out = HistoryStore::export("jsonl").unwrap();
            assert!(out.contains("\"to\":\"claude\""));
        });
    }

    #[test]
    fn parse_date_loose_accepts_yyyy_mm_dd() {
        let dt = parse_date_loose("2026-05-25").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-05-25T00:00:00+00:00");
    }

    #[test]
    fn parse_date_loose_accepts_iso8601() {
        let dt = parse_date_loose("2026-05-25T14:30:22Z").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-05-25T14:30:22+00:00");
    }

    #[test]
    fn parse_date_loose_rejects_garbage() {
        assert!(parse_date_loose("not-a-date").is_err());
    }
}
