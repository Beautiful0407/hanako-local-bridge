#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod access;
mod cloud;
mod execution;
mod manager;
mod mcp;
mod product;
mod service;
mod state;

use std::{
    env,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use hanako_bridge_core::{DeviceIdentity, RuntimeConfig};
use serde::Deserialize;
use serde_json::json;
use state::AppState;
use tower_http::trace::TraceLayer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn env_string(name: &str, fallback: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_u16(name: &str, fallback: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(fallback)
}

fn env_bool(name: &str, fallback: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pending_requests = state.access.pending_count().await;
    let pending_executions = state.execution.pending_count().await;
    let cloud_identity = state.cloud_identity().await;
    (
        StatusCode::OK,
        axum::Json(json!({
            "ok": true,
            "version": VERSION,
            "configPath": state.runtime.config_path,
            "dataDir": state.data_dir,
            "logDir": state.log_dir,
            "device": state.device,
            "trustMode": if state.full_trust { "full" } else { "approval" },
            "approvalUrl": format!("http://127.0.0.1:{}/", state.approval_port),
            "roots": state.resolver.grants(),
            "pendingRequests": pending_requests,
            "pendingExecutions": pending_executions,
            "capabilities": state.capabilities(),
            "cloud": cloud_identity,
            "metrics": state.metrics()
        })),
    )
}

async fn mcp_delete() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({ "ok": true })))
}

fn token_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-hanako-bridge-token")
                .and_then(|value| value.to_str().ok())
        })
}

async fn mcp_route(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if headers.contains_key(http::header::ORIGIN) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "browser origins are not allowed" })),
        );
    }
    let supplied = token_from_headers(&headers).unwrap_or_default().as_bytes();
    if !state.token_matches(supplied) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": "invalid MCP token" })),
        );
    }
    let response = mcp::handle_payload(Arc::clone(&state), payload).await;
    (StatusCode::OK, axum::Json(response))
}

async fn approval_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pending = state.access.pending_count().await;
    let pending_executions = state.execution.pending_count().await;
    (
        StatusCode::OK,
        axum::Json(json!({
            "ok": true,
            "version": VERSION,
            "runtime": "rust",
            "trustMode": if state.full_trust { "full" } else { "approval" },
            "approvalRequired": !state.full_trust,
            "pending": pending,
            "pendingExecutions": pending_executions
        })),
    )
}

async fn approval_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let body = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Hanako Local Bridge</title><style>body{{font-family:Segoe UI,Microsoft YaHei,sans-serif;margin:40px;background:#f5f6f7;color:#202124}}main{{max-width:760px;margin:auto;background:white;border:1px solid #d8dde3;border-radius:8px;padding:28px}}code{{font-family:Consolas,monospace}}.ok{{color:#177245}}</style></head><body><main><h1>Hanako Local Bridge</h1><p class=\"ok\">Rust service is running.</p><p>Device: <code>{}</code></p><p>Trust mode: <code>{}</code></p><p>This compatibility page will become the Rust manager UI during the migration.</p></main></body></html>",
        state.device.id,
        if state.full_trust { "full" } else { "approval" }
    );
    (
        [
            (http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (http::header::CACHE_CONTROL, "no-store"),
            (http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
}

fn approval_token_from_headers(headers: &HeaderMap) -> &[u8] {
    headers
        .get("x-approval-token")
        .map_or(&[][..], HeaderValue::as_bytes)
}

async fn approval_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    (
        StatusCode::OK,
        axum::Json(json!({
            "trustMode": if state.full_trust { "full" } else { "approval" },
            "approvalRequired": !state.full_trust,
            "grants": state.access.list_grants().await,
            "requests": state.access.list_requests().await,
            "executionAuthorizations": state.execution.list_authorizations().await,
            "executionRequests": state.execution.list_requests().await
        })),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessApprovalInput {
    name: Option<String>,
    mode: Option<String>,
}

async fn approve_access(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<AccessApprovalInput>,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state
        .access
        .approve_request(&id, input.name.as_deref(), input.mode.as_deref())
        .await
    {
        Ok(grant) => (StatusCode::OK, axum::Json(json!({ "grant": grant }))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "error": error.to_string(),
                "code": error.code(),
                "expected": error.expected(),
                "actual": error.actual()
            })),
        ),
    }
}

async fn deny_access(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state.access.deny_request(&id).await {
        Ok(request) => (StatusCode::OK, axum::Json(json!({ "request": request }))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

async fn revoke_access(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state.access.revoke_grant(&id).await {
        Ok(grant) => (StatusCode::OK, axum::Json(json!({ "grant": grant }))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

#[derive(Deserialize)]
struct ExecutionApprovalInput {
    scope: Option<String>,
}

async fn approve_execution(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<ExecutionApprovalInput>,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state
        .execution
        .approve_request(&id, input.scope.as_deref().unwrap_or("once"))
        .await
    {
        Ok(authorization) => (
            StatusCode::OK,
            axum::Json(json!({ "authorization": authorization })),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

async fn deny_execution(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state.execution.deny_request(&id).await {
        Ok(request) => (StatusCode::OK, axum::Json(json!({ "request": request }))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

async fn revoke_execution(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.token_matches(approval_token_from_headers(&headers)) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "invalid approval token" })),
        );
    }
    match state.execution.revoke_authorization(&id).await {
        Ok(authorization) => (
            StatusCode::OK,
            axum::Json(json!({ "authorization": authorization })),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

fn browser_origin_allowed(state: &AppState, origin: &str) -> bool {
    let Ok(origin_url) = url::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = origin_url.host_str() else {
        return false;
    };
    let mut allowed = Vec::new();
    if let Ok(cloud_url) = url::Url::parse(&state.runtime.config.cloud.url)
        && let Some(host) = cloud_url.host_str()
    {
        allowed.push(host.to_ascii_lowercase());
    }
    allowed.push(state.runtime.config.tunnel.server.to_ascii_lowercase());
    allowed.extend(
        env::var("HANA_BROWSER_IDENTITY_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
    );
    allowed
        .iter()
        .any(|host| host.eq_ignore_ascii_case(origin_host))
}

async fn client_identity(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
) -> impl IntoResponse {
    let origin = headers
        .get(http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !browser_origin_allowed(&state, origin) {
        return (
            StatusCode::FORBIDDEN,
            HeaderMap::new(),
            axum::Json(json!({ "error": "origin not allowed" })),
        );
    }
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(origin) {
        response_headers.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    response_headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, OPTIONS"),
    );
    response_headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    response_headers.insert(
        "access-control-allow-private-network",
        HeaderValue::from_static("true"),
    );
    if method == Method::OPTIONS {
        return (
            StatusCode::OK,
            response_headers,
            axum::Json(json!({ "ok": true })),
        );
    }
    let cloud_identity = state.cloud_identity().await;
    (
        StatusCode::OK,
        response_headers,
        axum::Json(json!({
            "ok": true,
            "version": VERSION,
            "device": state.device,
            "cloud": cloud_identity
        })),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if service::run_service_command_if_requested().await? {
        return Ok(());
    }
    if execution::run_worker_if_requested().await? {
        return Ok(());
    }
    if product::launch_manager_if_requested()? {
        return Ok(());
    }
    let install_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let runtime = RuntimeConfig::load(&install_dir, None).with_context(|| {
        format!(
            "cannot load bridge configuration from {}",
            install_dir.display()
        )
    })?;
    let config = runtime.config.clone();
    let data_dir = env::var_os("LOCAL_FS_MCP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.storage.data_dir.clone());
    let log_dir = env::var_os("LOCAL_FS_MCP_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.storage.log_dir.clone());
    tokio::fs::create_dir_all(&data_dir).await?;
    tokio::fs::create_dir_all(&log_dir).await?;
    let device = DeviceIdentity::load(&data_dir, &config.device.id, &config.device.name)?;
    let full_trust = env_string("LOCAL_AGENT_TRUST_MODE", &config.filesystem.trust_mode)
        .eq_ignore_ascii_case("full");
    let host = env_string("LOCAL_FS_MCP_HOST", &config.filesystem.host);
    let port = env_u16("LOCAL_FS_MCP_PORT", config.filesystem.port);
    let approval_port = env_u16(
        "LOCAL_FS_MCP_APPROVAL_PORT",
        config.filesystem.approval_port,
    );
    let state = Arc::new(
        AppState::new(
            runtime,
            device,
            data_dir,
            log_dir,
            full_trust,
            approval_port,
            env_bool(
                "LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION",
                config.filesystem.allow_chat_authorization,
            ),
        )
        .await?,
    );
    let cloud = Arc::new(
        cloud::CloudConnector::new(
            state.runtime.config.cloud.clone(),
            state.data_dir.clone(),
            state.device.clone(),
            VERSION,
            Arc::downgrade(&state),
        )
        .await?,
    );
    state
        .cloud
        .set(Arc::clone(&cloud))
        .map_err(|_| anyhow::anyhow!("cloud connector was already initialized"))?;
    cloud.start();

    let mcp_app = Router::new()
        .route("/health", get(health))
        .route("/mcp", post(mcp_route).delete(mcp_delete))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::clone(&state));
    let approval_app = Router::new()
        .route("/", get(approval_page))
        .route("/health", get(approval_health))
        .route("/api/state", get(approval_state))
        .route(
            "/api/client-identity",
            get(client_identity).options(client_identity),
        )
        .route("/api/requests/{id}/approve", post(approve_access))
        .route("/api/requests/{id}/deny", post(deny_access))
        .route("/api/grants/{id}/revoke", post(revoke_access))
        .route(
            "/api/execution/requests/{id}/approve",
            post(approve_execution),
        )
        .route("/api/execution/requests/{id}/deny", post(deny_execution))
        .route(
            "/api/execution/authorizations/{id}/revoke",
            post(revoke_execution),
        )
        .merge(manager::router())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(Arc::clone(&state));

    let address = SocketAddr::new(IpAddr::from_str(&host)?, port);
    let approval_address = SocketAddr::new(IpAddr::from_str("127.0.0.1")?, approval_port);
    let mcp_listener = tokio::net::TcpListener::bind(address).await?;
    let approval_listener = tokio::net::TcpListener::bind(approval_address).await?;

    println!("[hanako-bridge] v{VERSION} listening on http://{address}/mcp");
    println!(
        "[hanako-bridge] trust mode: {}",
        if state.full_trust { "full" } else { "approval" }
    );
    println!("[hanako-bridge] status UI: http://{approval_address}/");
    println!(
        "[hanako-bridge] cloud connector: {}",
        if state.runtime.config.cloud.enabled {
            &state.runtime.config.cloud.url
        } else {
            "disabled"
        }
    );

    tokio::try_join!(
        axum::serve(mcp_listener, mcp_app),
        axum::serve(approval_listener, approval_app),
    )?;
    Ok(())
}
