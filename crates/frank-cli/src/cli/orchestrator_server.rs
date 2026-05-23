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
use axum::routing::get;
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
            detect_models(name).await
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

/// 每家 CLI 自家的 model 列表来源 — 全部走本机数据, 不调外网。
async fn detect_models(provider: &str) -> Vec<String> {
    match provider {
        // Anthropic 标准 alias (`claude --help` 文档: "alias 'sonnet' or 'opus'")
        "claude" => vec!["opus".into(), "sonnet".into(), "haiku".into()],
        // codex 用户本机 ~/.codex/models_cache.json (登录后 codex CLI 自己刷的)
        "codex" => read_codex_models(),
        // opencode: subprocess `opencode models` 一行一个
        "opencode" => list_opencode_models().await,
        // gemini CLI 没列模型命令, 用 Google 公开 alias
        "gemini" => vec![
            "gemini-2.5-pro".into(),
            "gemini-2.5-flash".into(),
            "gemini-2.0-flash".into(),
        ],
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

async fn list_opencode_models() -> Vec<String> {
    let out = tokio::process::Command::new("opencode")
        .arg("models")
        .output()
        .await
        .ok();
    let Some(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
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
