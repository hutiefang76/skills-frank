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
                let (sref, plats, enabled) = state.get(&s.name).map_or(
                    (String::new(), Vec::new(), false),
                    |st| {
                        (
                            st.source_ref.chars().take(7).collect::<String>(),
                            st.platforms.iter().map(|p| format!("{p:?}")).collect(),
                            st.enabled,
                        )
                    },
                );
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
    .map(|()| Json(OkResp { ok: true, error: None }))
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
    .map(|()| Json(OkResp { ok: true, error: None }))
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
    .map(|()| Json(OkResp { ok: true, error: None }))
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
    .map(|()| Json(OkResp { ok: true, error: None }))
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
