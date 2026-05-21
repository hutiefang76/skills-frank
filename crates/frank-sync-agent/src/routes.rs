//! HTTP / WebSocket 路由定义。
//!
//! # 路由清单 (v1)
//!
//! ```text
//! GET    /healthz                     存活探针 (顶层)
//! GET    /memory/healthz              存活探针 (memory 子前缀, 便于 Caddy /memory/* 反代统一测)
//! POST   /memory/add                  添加记忆 (LLM 抽取多条 fact)
//! POST   /memory/add_raw              添加单条已成型 fact (跳过 LLM)
//! POST   /memory/search               按 query 检索
//! GET    /memory/:id                  按 ID 取一条
//! DELETE /memory/:id                  按 ID 删除
//! POST   /memory/list                 按 scope 列出
//! ```
//!
//! 待加 (P6 orchestrator):
//! ```text
//! POST   /orchestrator/jobs           提交新 job
//! GET    /orchestrator/jobs/:id       取 job 状态
//! GET    /orchestrator/jobs/:id/ws    WebSocket 流式日志
//! ```

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use frank_memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// 顶层 router 构造。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/memory/healthz", get(healthz))
        .route("/memory/add", post(memory_add))
        .route("/memory/add_raw", post(memory_add_raw))
        .route("/memory/search", post(memory_search))
        .route("/memory/list", post(memory_list))
        .route("/memory/:id", get(memory_get).delete(memory_delete))
        .route("/memory/delete/:id", delete(memory_delete)) // 同上, 给老 client 兜底
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn healthz() -> &'static str {
    "ok"
}

// ---- /memory/add ----

#[derive(Deserialize)]
struct AddRequest {
    content: String,
    scope: Scope,
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct AddResponse {
    ids: Vec<MemoryId>,
}

async fn memory_add(
    State(state): State<AppState>,
    Json(req): Json<AddRequest>,
) -> ApiResult<Json<AddResponse>> {
    let ids = state
        .memory
        .add(&req.content, req.scope, req.metadata)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(AddResponse { ids }))
}

// ---- /memory/add_raw ----

#[derive(Deserialize)]
struct AddRawRequest {
    fact: String,
    scope: Scope,
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct AddRawResponse {
    id: MemoryId,
}

async fn memory_add_raw(
    State(state): State<AppState>,
    Json(req): Json<AddRawRequest>,
) -> ApiResult<Json<AddRawResponse>> {
    let id = state
        .memory
        .add_raw(&req.fact, req.scope, req.metadata)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(AddRawResponse { id }))
}

// ---- /memory/search ----

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    scope: Scope,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    score_threshold: Option<f32>,
}

#[derive(Serialize)]
struct SearchResponse {
    matches: Vec<MemoryMatch>,
}

async fn memory_search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> ApiResult<Json<SearchResponse>> {
    let mut opts = SearchOpts::default();
    if let Some(l) = req.limit {
        opts.limit = l;
    }
    if let Some(s) = req.score_threshold {
        opts.score_threshold = s;
    }
    let matches = state
        .memory
        .search(&req.query, req.scope, opts)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(SearchResponse { matches }))
}

// ---- /memory/list ----

#[derive(Deserialize)]
struct ListRequest {
    scope: Scope,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 {
    100
}

#[derive(Serialize)]
struct ListResponse {
    records: Vec<MemoryRecord>,
}

async fn memory_list(
    State(state): State<AppState>,
    Json(req): Json<ListRequest>,
) -> ApiResult<Json<ListResponse>> {
    let records = state
        .memory
        .list(req.scope, req.limit)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ListResponse { records }))
}

// ---- /memory/:id ----

async fn memory_get(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<Option<MemoryRecord>>> {
    let rec = state
        .memory
        .get(&MemoryId::from_uuid(id))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(rec))
}

async fn memory_delete(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<StatusCode> {
    state
        .memory
        .delete(&MemoryId::from_uuid(id))
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- 错误统一 ----

type ApiResult<T> = Result<T, ApiError>;

/// API 错误统一包装, 自动转 500 + JSON 错误体。
pub struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "error": format!("{:#}", self.0),
        });
        tracing::warn!(error = %self.0, "API error");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}
