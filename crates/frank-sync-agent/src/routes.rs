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

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use frank_memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_http::trace::TraceLayer;

use crate::state::AppState;
use crate::tenant::TenantStatus;

/// v0.11.1 用户隔离: 从 X-Frank-Token 派生 tenant_id (12 hex 字符 = 48 bit,
/// 生日攻击碰撞门槛 ~16M 用户, 单机部署绰绰有余).
///
/// **作用**:
/// - 所有 memory add/search/list/delete 操作前, 服务端用此覆盖 scope.user_id
/// - 不同 token (e.g. `frank login --new` 生成的随机 uuid) → 不同 tenant
/// - 老的共享 FRANK_API_TOKEN → 共享"demo" tenant (数据公开混在一起)
///
/// **限制**: 不防恶意 (用户可以伪造任意 token), 只防"无意泄漏"和"普通隔离".
/// 真正的多用户安全要做 OAuth / mTLS, 留 v0.13+.
fn tenant_id_from_headers(headers: &HeaderMap) -> String {
    let token = headers
        .get("X-Frank-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous");
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..6])
}

/// 把 server 派生的 tenant_id 强制注入 scope.user_id, 覆盖客户端任何传值.
/// agent_id / session_id 保留客户端控制.
fn inject_tenant(headers: &HeaderMap, mut scope: Scope) -> Scope {
    let tenant = tenant_id_from_headers(headers);
    scope.user_id = Some(format!("t_{tenant}"));
    scope
}

/// v0.12.0 ensure registered + quota check (写操作前用). 返回 tenant_id 给后续 update.
async fn ensure_registered(headers: &HeaderMap, state: &AppState) -> ApiResult<String> {
    let tenant_id = tenant_id_from_headers(headers);
    if !state
        .tenants
        .is_registered(&tenant_id)
        .await
        .map_err(ApiError::from)?
    {
        return Err(ApiError::status(
            StatusCode::UNAUTHORIZED,
            format!(
                "tenant 未注册. 跑 `frank login` 或 POST /tenant/register (token: {tenant_id}...)"
            ),
        ));
    }
    Ok(tenant_id)
}

async fn ensure_within_quota(
    headers: &HeaderMap,
    state: &AppState,
    add_n: i64,
) -> ApiResult<String> {
    let tenant_id = ensure_registered(headers, state).await?;
    let status = state
        .tenants
        .status(&tenant_id)
        .await
        .map_err(ApiError::from)?;
    let used = status.as_ref().map_or(0, |s| s.records_count);
    if used + add_n > state.quota_per_tenant {
        return Err(ApiError::status(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "quota_exceeded: tenant 已用 {used}/{} records (申请加配额请联系 hutiefang@gmail.com)",
                state.quota_per_tenant
            ),
        ));
    }
    Ok(tenant_id)
}

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
        // ─── v0.12.0 tenant registry + quota + deletion ───
        .route("/tenant/register", post(tenant_register))
        .route("/tenant/status", get(tenant_status))
        .route("/tenant/request-deletion", post(tenant_request_deletion))
        .route("/tenant/cancel-deletion", post(tenant_cancel_deletion))
        // ─── v0.13.0 server-side machine-bound token provisioning ───
        .route("/tenant/provision", post(tenant_provision))
        .route("/tenant/link-machine", post(tenant_link_machine))
        // ─── 跨设备 skills 同步 (v0.4 — 用户需求 2.3) ───
        .route("/sync/skills/push", post(sync_push))
        .route("/sync/skills/pull", get(sync_pull))
        .route("/sync/skills/devices", get(sync_devices))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// v0.12.0 后台 retention worker — 每小时扫一次, 把到期 tenant 真删 (qdrant + sqlite).
pub fn spawn_retention_worker(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = run_retention_pass(&state).await {
                tracing::warn!(error = ?e, "retention pass failed");
            }
        }
    });
}

async fn run_retention_pass(state: &AppState) -> anyhow::Result<()> {
    let due = state.tenants.list_due_for_deletion().await?;
    if due.is_empty() {
        tracing::debug!("retention pass: 0 tenant 到期");
        return Ok(());
    }
    tracing::info!(count = due.len(), "retention pass: 真删 tenant");

    // 用 qdrant_client 直接按 user_id filter 删 points (Memory::delete 只能按 id 单删)
    use qdrant_client::qdrant::{Condition, Filter};
    use qdrant_client::Qdrant;

    let qdrant = Qdrant::from_url(&state.qdrant_url).build()?;
    for tenant_id in &due {
        let user_id_value = format!("t_{tenant_id}");
        let filter = Filter::must([Condition::matches("user_id", user_id_value.clone())]);
        let delete = qdrant_client::qdrant::DeletePointsBuilder::new(&state.qdrant_collection)
            .points(filter)
            .wait(true);
        if let Err(e) = qdrant.delete_points(delete).await {
            tracing::warn!(tenant_id, error = ?e, "qdrant delete_points failed; 跳过本轮, 下次重试");
            continue;
        }
        // qdrant 真删后, sqlite 清 row (整个 tenant 移除 registry)
        state.tenants.delete_tenant(tenant_id).await?;
        tracing::info!(tenant_id, "tenant 已真删 (qdrant points + sqlite row)");
    }
    Ok(())
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
    headers: HeaderMap,
    Json(req): Json<AddRequest>,
) -> ApiResult<Json<AddResponse>> {
    let tenant_id = ensure_within_quota(&headers, &state, 1).await?;
    let scope = inject_tenant(&headers, req.scope);
    let ids = state
        .memory
        .add(&req.content, scope, req.metadata)
        .await
        .map_err(ApiError::from)?;
    let _ = state
        .tenants
        .bump_records(&tenant_id, i64::try_from(ids.len()).unwrap_or(i64::MAX))
        .await;
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
    headers: HeaderMap,
    Json(req): Json<AddRawRequest>,
) -> ApiResult<Json<AddRawResponse>> {
    let tenant_id = ensure_within_quota(&headers, &state, 1).await?;
    let scope = inject_tenant(&headers, req.scope);
    let id = state
        .memory
        .add_raw(&req.fact, scope, req.metadata)
        .await
        .map_err(ApiError::from)?;
    let _ = state.tenants.bump_records(&tenant_id, 1).await;
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
    headers: HeaderMap,
    Json(req): Json<SearchRequest>,
) -> ApiResult<Json<SearchResponse>> {
    let _ = ensure_registered(&headers, &state).await?;
    let scope = inject_tenant(&headers, req.scope);
    let mut opts = SearchOpts::default();
    if let Some(l) = req.limit {
        opts.limit = l;
    }
    if let Some(s) = req.score_threshold {
        opts.score_threshold = s;
    }
    let matches = state
        .memory
        .search(&req.query, scope, opts)
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
    headers: HeaderMap,
    Json(req): Json<ListRequest>,
) -> ApiResult<Json<ListResponse>> {
    let _ = ensure_registered(&headers, &state).await?;
    let scope = inject_tenant(&headers, req.scope);
    let records = state
        .memory
        .list(scope, req.limit)
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
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<StatusCode> {
    let tenant_id = ensure_registered(&headers, &state).await?;
    state
        .memory
        .delete(&MemoryId::from_uuid(id))
        .await
        .map_err(ApiError::from)?;
    let _ = state.tenants.bump_records(&tenant_id, -1).await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- 错误统一 ----

type ApiResult<T> = Result<T, ApiError>;

/// API 错误统一包装, 自动转 JSON 错误体. v0.12.0 加 status code 支持.
pub struct ApiError {
    err: anyhow::Error,
    status: StatusCode,
}

impl ApiError {
    /// 显式指定 HTTP status code (e.g. 401 未注册, 429 quota).
    pub fn status(status: StatusCode, msg: impl Into<String>) -> Self {
        Self {
            err: anyhow::anyhow!(msg.into()),
            status,
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self {
            err: e.into(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "error": format!("{:#}", self.err),
        });
        tracing::warn!(status = %self.status, error = %self.err, "API error");
        (self.status, Json(body)).into_response()
    }
}

// ════════════════════════════════════════════════════════════════
// v0.12.0 tenant registry + 删除流程
// ════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct RegisterResponse {
    tenant_id: String,
    /// "registered" 新注册 / "already_registered" 已有 (但 last_seen 更新).
    status: String,
}

async fn tenant_register(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<RegisterResponse>> {
    let tenant_id = tenant_id_from_headers(&headers);
    let was_registered = state
        .tenants
        .is_registered(&tenant_id)
        .await
        .map_err(ApiError::from)?;
    state
        .tenants
        .register(&tenant_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(RegisterResponse {
        tenant_id: tenant_id.clone(),
        status: if was_registered {
            "already_registered".to_string()
        } else {
            "registered".to_string()
        },
    }))
}

async fn tenant_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<TenantStatus>> {
    let tenant_id = tenant_id_from_headers(&headers);
    let status = state
        .tenants
        .status(&tenant_id)
        .await
        .map_err(ApiError::from)?;
    match status {
        Some(s) => Ok(Json(s)),
        None => Err(ApiError::status(
            StatusCode::NOT_FOUND,
            "tenant 未注册. POST /tenant/register 注册",
        )),
    }
}

#[derive(Serialize)]
struct DeletionResponse {
    tenant_id: String,
    deletion_scheduled_at: i64,
    real_delete_at_human: String,
}

async fn tenant_request_deletion(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<DeletionResponse>> {
    let tenant_id = ensure_registered(&headers, &state).await?;
    let wait_secs = state.deletion_wait_days * 86400;
    let schedule_at = chrono::Utc::now().timestamp() + wait_secs;
    state
        .tenants
        .schedule_deletion(&tenant_id, schedule_at)
        .await
        .map_err(ApiError::from)?;
    let human = chrono::DateTime::<chrono::Utc>::from_timestamp(schedule_at, 0)
        .map_or_else(|| "unknown".to_string(), |dt| dt.to_rfc3339());
    Ok(Json(DeletionResponse {
        tenant_id,
        deletion_scheduled_at: schedule_at,
        real_delete_at_human: human,
    }))
}

async fn tenant_cancel_deletion(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let tenant_id = ensure_registered(&headers, &state).await?;
    state
        .tenants
        .cancel_deletion(&tenant_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "tenant_id": tenant_id,
        "deletion_scheduled_at": null,
        "status": "cancelled"
    })))
}

// ════════════════════════════════════════════════════════════════
// v0.13.0 server-side token provisioning
//
// 流程:
//   1. 客户端 (frank-cli) 收集 machine fingerprint (hostname / mac / cpu_id 等)
//   2. POST /tenant/provision 带 fingerprint JSON, **不带 X-Frank-Token** (bootstrap)
//   3. 服务端: sha256(fp)[:16] = machine_code; 查重 → 生成 32-byte random token
//      → derive tenant_id → INSERT tenants + machines (事务)
//   4. 返回 token + tenant_id + machine_code; 客户端存 ~/.frank/.token (chmod 600)
//   5. 后续请求带 X-Frank-Token: <token>, 走原来的 ensure_registered / quota 路径
//
// 跨机场景: 用户在 B 机想用 A 机的 tenant → A 机 cat ~/.frank/.token 给 B,
//   B 机 POST /tenant/link-machine + X-Frank-Token + fingerprint → 服务端 INSERT machines.
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct ProvisionRequest {
    /// 客户端 `MachineFingerprint` 整体 JSON (服务端只 sha256, 不解 schema)
    fingerprint: serde_json::Value,
}

#[derive(Serialize)]
struct ProvisionResponse {
    /// 服务端生成的 base64url 32-byte token. 客户端存 ~/.frank/.token (chmod 600).
    /// **只返回一次** — 丢了只能 `frank tenant reset` 拿新 token.
    token: String,
    /// `sha256(token)[:12]` hex, 与现有 derive 一致.
    tenant_id: String,
    /// `sha256(fingerprint_json)[:16]` hex; 客户端可选存 ~/.frank/.machine_id (info only).
    machine_code: String,
    /// 提示文本 (客户端 UI 可显示).
    note: String,
}

async fn tenant_provision(
    State(state): State<AppState>,
    Json(req): Json<ProvisionRequest>,
) -> ApiResult<Json<ProvisionResponse>> {
    // 不需要 X-Frank-Token — 这是 bootstrap (拿 token 的入口).
    // 防 spam 留 v0.13.1: 同 IP 1 / 15min (caddy rate_limit 也能做).
    let fp_json = serde_json::to_string(&req.fingerprint).map_err(ApiError::from)?;
    let result = state
        .tenants
        .provision_machine(&fp_json)
        .await
        .map_err(|e| ApiError::status(StatusCode::CONFLICT, format!("{e:#}")))?;
    Ok(Json(ProvisionResponse {
        token: result.token,
        tenant_id: result.tenant_id,
        machine_code: result.machine_code,
        note: "保存 token 到 ~/.frank/.token (chmod 600), 后续请求带 X-Frank-Token".to_string(),
    }))
}

#[derive(Deserialize)]
struct LinkMachineRequest {
    /// 当前机器的 fingerprint JSON (与 provision 同形状, 服务端只 sha256).
    fingerprint: serde_json::Value,
}

async fn tenant_link_machine(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LinkMachineRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // 必须带 X-Frank-Token (= 已有 tenant 的 token), tenant 派生由 token sha256 决定.
    let tenant_id = ensure_registered(&headers, &state).await?;
    let fp_json = serde_json::to_string(&req.fingerprint).map_err(ApiError::from)?;
    let machine_code = state
        .tenants
        .link_machine(&tenant_id, &fp_json)
        .await
        .map_err(|e| ApiError::status(StatusCode::CONFLICT, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({
        "tenant_id": tenant_id,
        "machine_code": machine_code,
        "status": "linked",
    })))
}

// ════════════════════════════════════════════════════════════════
// 跨设备 skills 同步 (v0.4 — 用户需求 2.3)
//
// 每台设备唯一 device_id (默认 hostname), push 把本机 state.json 透传给服务端,
// pull 按 device_id 拿别人的列表. 服务端 KV store (HashMap<id, JSON>), 不解 schema.
// v0.5 改 SQLite 持久化.
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct SyncPushRequest {
    device_id: String,
    state: serde_json::Value,
}

#[derive(Serialize)]
struct SyncPushResponse {
    device_id: String,
    skills_count: usize,
}

async fn sync_push(
    State(state): State<AppState>,
    Json(req): Json<SyncPushRequest>,
) -> ApiResult<Json<SyncPushResponse>> {
    let count = req
        .state
        .get("skills")
        .and_then(|v| v.as_object())
        .map_or(0, serde_json::Map::len);
    state
        .skills_sync
        .write()
        .await
        .insert(req.device_id.clone(), req.state);
    tracing::info!(device = %req.device_id, skills = count, "sync push received");
    Ok(Json(SyncPushResponse {
        device_id: req.device_id,
        skills_count: count,
    }))
}

#[derive(Deserialize)]
struct SyncPullQuery {
    device_id: String,
}

async fn sync_pull(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<SyncPullQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state.skills_sync.read().await;
    let payload = store
        .get(&q.device_id)
        .cloned()
        .unwrap_or(serde_json::json!({
            "schema_version": 1,
            "profile": "personal",
            "skills": {}
        }));
    Ok(Json(payload))
}

#[derive(Serialize)]
struct SyncDevicesResponse {
    devices: Vec<DeviceInfo>,
}

#[derive(Serialize)]
struct DeviceInfo {
    device_id: String,
    skills_count: usize,
}

async fn sync_devices(State(state): State<AppState>) -> Json<SyncDevicesResponse> {
    let store = state.skills_sync.read().await;
    let devices: Vec<DeviceInfo> = store
        .iter()
        .map(|(id, v)| DeviceInfo {
            device_id: id.clone(),
            skills_count: v
                .get("skills")
                .and_then(|s| s.as_object())
                .map_or(0, serde_json::Map::len),
        })
        .collect();
    Json(SyncDevicesResponse { devices })
}
