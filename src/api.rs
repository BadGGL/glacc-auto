// HTTP API 与静态资源。所有写操作经 web_auth 中间件校验 Cookie + CSRF 头。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::glacc::diag;
use crate::glacc::history;
use crate::glacc::settings::AppSettings;
use crate::glacc::store;
use crate::scheduler::{ScheduleRule, Scheduler};
use crate::service::{
    accounts_dto, cancel_claim, lock_runtime, login_blocking, progress_dto, send_sms_blocking,
    snapshot_blocking, start_claim, AppState,
};
use crate::web_auth::{auth_middleware, token_from_headers, WebAuth};

#[derive(Clone)]
pub struct ApiState {
    pub app: Arc<AppState>,
    pub auth: WebAuth,
    pub scheduler: Arc<Scheduler>,
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl From<String> for ApiError {
    fn from(value: String) -> Self {
        ApiError::bad(value)
    }
}

pub fn router(
    app: Arc<AppState>,
    auth: WebAuth,
    scheduler: Arc<Scheduler>,
    web_dir: &str,
) -> Router {
    let state = ApiState {
        app,
        auth: auth.clone(),
        scheduler,
    };
    let index = format!("{web_dir}/index.html");

    Router::new()
        .route("/healthz", get(health))
        .route("/api/auth/state", get(auth_state))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/status", get(status))
        .route("/api/accounts", get(get_accounts).post(set_active))
        .route("/api/accounts/logout", post(logout_account))
        .route("/api/auth/sms", post(send_sms))
        .route("/api/auth/login-code", post(login_code))
        .route("/api/snapshot", get(snapshot))
        .route("/api/claim/progress", get(progress))
        .route("/api/claim/start", post(claim_start))
        .route("/api/claim/cancel", post(claim_cancel))
        .route("/api/settings", get(get_settings).post(save_settings))
        .route("/api/schedules", get(list_schedules).post(create_schedule))
        .route(
            "/api/schedules/{id}",
            put(update_schedule).delete(delete_schedule),
        )
        .route("/api/schedules/{id}/run", post(run_schedule))
        .route("/api/history", get(get_history))
        .route("/api/logs", get(get_logs))
        .route("/api/export", get(export_accounts))
        .route("/api/import", post(import_accounts))
        .fallback_service(
            ServeDir::new(web_dir).not_found_service(ServeFile::new(index.as_str())),
        )
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn_with_state(
            auth,
            auth_middleware,
        ))
        .with_state(state)
}

// ── 基础 ──

async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthState {
    authenticated: bool,
    password_set: bool,
}

async fn auth_state(State(st): State<ApiState>, headers: HeaderMap) -> Json<AuthState> {
    let authenticated = token_from_headers(&headers)
        .map(|t| st.auth.store.verify_token(&t))
        .unwrap_or(false);
    Json(AuthState {
        authenticated,
        password_set: true,
    })
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

async fn login(
    State(st): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    let ip = st.auth.client_ip(&headers, addr.ip());
    if !st.auth.limiter.allow(ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error":"尝试过于频繁，请 5 分钟后再试"})),
        )
            .into_response();
    }
    if !st.auth.store.verify_password(&req.password) {
        st.auth.limiter.record_failure(ip);
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"密码错误"})),
        )
            .into_response();
    }
    st.auth.limiter.clear(ip);
    let token = st.auth.store.issue_token();
    let cookie = st.auth.cookie_header(&token);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({"ok": true})),
    )
        .into_response()
}

async fn logout(State(st): State<ApiState>) -> Response {
    (
        StatusCode::OK,
        [(header::SET_COOKIE, st.auth.clear_cookie_header())],
        Json(serde_json::json!({"ok": true})),
    )
        .into_response()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusDto {
    version: String,
    uptime_secs: u64,
    tls_ready: bool,
    running: bool,
    status_text: String,
    status_is_error: bool,
}

async fn status(State(st): State<ApiState>) -> Json<StatusDto> {
    let runtime = lock_runtime(&st.app.runtime);
    Json(StatusDto {
        version: st.app.version.to_string(),
        uptime_secs: st.app.started_at.elapsed().as_secs(),
        tls_ready: st.app.tls_ready,
        running: runtime.running,
        status_text: runtime.status.clone(),
        status_is_error: runtime.error,
    })
}

// ── 账号 ──

async fn get_accounts() -> Json<Vec<crate::service::AccountDto>> {
    Json(accounts_dto())
}

#[derive(Deserialize)]
struct KeyRequest {
    key: String,
}

async fn set_active(Json(req): Json<KeyRequest>) -> Result<Json<Vec<crate::service::AccountDto>>, ApiError> {
    if !store::list_accounts().iter().any(|a| a.key == req.key) {
        return Err(ApiError::bad("该账号不存在"));
    }
    store::touch(&req.key)?;
    diag::info("已切换当前账号");
    Ok(Json(accounts_dto()))
}

#[derive(Deserialize)]
struct OptionalKeyRequest {
    key: Option<String>,
}

async fn logout_account(Json(req): Json<OptionalKeyRequest>) -> Result<Json<Vec<crate::service::AccountDto>>, ApiError> {
    let target = match req.key.filter(|k| !k.is_empty()) {
        Some(key) => key,
        None => store::list_accounts()
            .first()
            .map(|a| a.key.clone())
            .ok_or_else(|| ApiError::bad("没有可退出的账号"))?,
    };
    store::remove(&target);
    diag::info("已退出登录：账号凭据已删除");
    Ok(Json(accounts_dto()))
}

#[derive(Deserialize)]
struct PhoneRequest {
    phone: String,
}

async fn send_sms(State(st): State<ApiState>, Json(req): Json<PhoneRequest>) -> Result<Json<crate::service::SmsCodeDto>, ApiError> {
    let gate = st.app.auth_gate.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|e| e.into_inner());
        send_sms_blocking(&req.phone)
    })
    .await
    .map_err(|e| ApiError::internal(format!("发送验证码任务异常：{e}")))?
    .map(Json)
    .map_err(ApiError::bad)
}

#[derive(Deserialize)]
struct CodeRequest {
    code: String,
}

async fn login_code(State(st): State<ApiState>, Json(req): Json<CodeRequest>) -> Result<Json<crate::service::LoginResultDto>, ApiError> {
    let gate = st.app.auth_gate.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|e| e.into_inner());
        login_blocking(&req.code)
    })
    .await
    .map_err(|e| ApiError::internal(format!("登录任务异常：{e}")))?
    .map(Json)
    .map_err(ApiError::bad)
}

// ── 首页 / 领取 ──

async fn snapshot() -> Result<Json<crate::service::SnapshotDto>, ApiError> {
    tokio::task::spawn_blocking(snapshot_blocking)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(format!("快照任务异常：{e}")))
}

async fn progress(State(st): State<ApiState>) -> Json<crate::service::ClaimProgressDto> {
    let runtime = lock_runtime(&st.app.runtime);
    Json(progress_dto(&runtime))
}

#[derive(Deserialize, Default)]
struct StartRequest {
    scope: Option<String>,
}

async fn claim_start(
    State(st): State<ApiState>,
    body: Option<Json<StartRequest>>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let scope = body
        .and_then(|b| b.0.scope)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "all".to_string());
    start_claim(st.app.clone(), &scope).map_err(|e| {
        if e.contains("正在领取") {
            ApiError::conflict(e)
        } else {
            ApiError::bad(e)
        }
    })?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({"started": true}))))
}

async fn claim_cancel(State(st): State<ApiState>) -> Json<serde_json::Value> {
    cancel_claim(&st.app);
    Json(serde_json::json!({"cancelling": true}))
}

// ── 设置 ──

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDto {
    server_key: String,
    interval_min_sec: i32,
    interval_max_sec: i32,
    network_retry_count: i32,
    theme: String,
    scheduled_time: String,
    #[serde(default)]
    version: String,
}

impl From<AppSettings> for SettingsDto {
    fn from(s: AppSettings) -> Self {
        Self {
            server_key: s.server_key,
            interval_min_sec: s.interval_min_sec,
            interval_max_sec: s.interval_max_sec,
            network_retry_count: s.network_retry_count,
            theme: s.theme,
            scheduled_time: s.scheduled_time,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

async fn get_settings() -> Json<SettingsDto> {
    Json(AppSettings::load().into())
}

async fn save_settings(Json(input): Json<SettingsDto>) -> Result<Json<SettingsDto>, ApiError> {
    let mut settings = AppSettings {
        scheduled_time: input.scheduled_time.trim().to_string(),
        server_key: input.server_key.trim().to_string(),
        interval_min_sec: input.interval_min_sec,
        interval_max_sec: input.interval_max_sec,
        network_retry_count: input.network_retry_count,
        theme: input.theme,
    };
    settings.normalize();
    settings.save();
    Ok(Json(settings.into()))
}

// ── 定时任务 ──

async fn list_schedules(State(st): State<ApiState>) -> Json<Vec<ScheduleRule>> {
    Json(st.scheduler.list())
}

async fn create_schedule(
    State(st): State<ApiState>,
    Json(mut rule): Json<ScheduleRule>,
) -> Result<Json<ScheduleRule>, ApiError> {
    rule.id.clear();
    st.scheduler.create(rule).map(Json).map_err(ApiError::bad)
}

async fn update_schedule(
    State(st): State<ApiState>,
    Path(id): Path<String>,
    Json(rule): Json<ScheduleRule>,
) -> Result<Json<ScheduleRule>, ApiError> {
    st.scheduler.update(&id, rule).map(Json).map_err(ApiError::bad)
}

async fn delete_schedule(
    State(st): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    st.scheduler.delete(&id).map_err(ApiError::bad)?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn run_schedule(
    State(st): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    st.scheduler.run_now(&id).map_err(ApiError::bad)?;
    Ok(Json(serde_json::json!({"started": true})))
}

// ── 历史 / 日志 ──

async fn get_history() -> Json<Vec<history::HistoryDay>> {
    Json(history::last_days(30))
}

#[derive(Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogPage {
    file_name: String,
    exists: bool,
    lines: Vec<String>,
    truncated: bool,
}

async fn get_logs(Query(q): Query<LogsQuery>) -> Json<LogPage> {
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let path = diag::current_file_path();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !path.exists() {
        return Json(LogPage {
            file_name,
            exists: false,
            lines: Vec::new(),
            truncated: false,
        });
    }
    let bytes = std::fs::read(&path).unwrap_or_default();
    const MAX: usize = 512 * 1024;
    let truncated = bytes.len() > MAX;
    let slice = if truncated { &bytes[bytes.len() - MAX..] } else { &bytes[..] };
    let mut text = String::from_utf8_lossy(slice).to_string();
    if truncated {
        if let Some(pos) = text.find('\n') {
            text.drain(..=pos);
        }
    }
    let mut lines: Vec<String> = text.lines().map(|s| s.trim_end_matches('\r').to_string()).collect();
    if lines.len() > limit {
        lines.drain(..lines.len() - limit);
    }
    Json(LogPage {
        file_name,
        exists: true,
        lines,
        truncated,
    })
}

// ── 凭据导入导出 ──

async fn export_accounts() -> Result<Response, ApiError> {
    let path = std::env::temp_dir().join(format!(
        "glacc-export-{}.json",
        uuid::Uuid::new_v4()
    ));
    let envelope = store::export_accounts(&path).map_err(|e| ApiError::bad(e.to_string()))?;
    let bytes = std::fs::read(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    let filename = format!("glacc-accounts-{}.json", chrono::Local::now().format("%Y%m%d%H%M%S"));
    let _ = envelope;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn import_accounts(body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let path = std::env::temp_dir().join(format!(
        "glacc-import-{}.json",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, &body).map_err(|e| ApiError::internal(format!("写入临时文件失败：{e}")))?;
    let result = store::import_accounts(&path);
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(summary) => Ok(Json(serde_json::json!({
            "imported": summary.imported,
            "skipped": summary.skipped,
        }))),
        Err(e) => Err(ApiError::bad(e.to_string())),
    }
}
