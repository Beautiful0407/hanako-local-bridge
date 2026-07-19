use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use hanako_bridge_core::{
    config::{RootConfig, RootMode},
    device::clean_device_id,
    store::write_json_atomic,
};
use serde::Deserialize;
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::{
    service::{self, service_status},
    state::AppState,
};

const MANAGER_HTML: &str = include_str!("../assets/manager.html");

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/manager", get(manager_page))
        .route("/manager/", get(manager_page))
        .route("/favicon.ico", get(favicon))
        .route("/api/manager/snapshot", get(snapshot))
        .route("/api/manager/action", post(action))
        .route("/api/manager/settings", post(save_settings))
        .route("/api/manager/logs", get(logs))
        .route("/api/manager/logs/{*path}", get(log_tail))
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn manager_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = MANAGER_HTML.replace("__HANA_APPROVAL_TOKEN__", &state.approval_token());
    (
        [
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (axum::http::header::X_FRAME_OPTIONS, "DENY"),
        ],
        Html(html),
    )
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    state.token_matches(
        headers
            .get("x-approval-token")
            .map_or(&[][..], axum::http::HeaderValue::as_bytes),
    )
}

fn forbidden() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "error": "invalid approval token" })),
    )
}

async fn snapshot(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    let task_name = format!("{} MCP", state.runtime.config.service.task_prefix.trim());
    let service = service_status(&state.runtime, &task_name)
        .await
        .unwrap_or_else(|_error| service::ServiceStatus {
            task_name,
            task_exists: false,
            health_ok: false,
            health_url: format!(
                "http://127.0.0.1:{}/health",
                state.runtime.config.filesystem.port
            ),
            executable: state.runtime.install_dir.join("hanako-bridge.exe"),
        });
    let checks = vec![
        json!({
            "code": "config",
            "status": if state.runtime.config_path.is_file() { "pass" } else { "warning" },
            "detail": state.runtime.config_path
        }),
        json!({
            "code": "service_task",
            "status": if service.task_exists { "pass" } else { "error" },
            "detail": service.task_name
        }),
        json!({
            "code": "mcp_health",
            "status": if service.health_ok { "pass" } else { "error" },
            "detail": service.health_url
        }),
        json!({
            "code": "cloud",
            "status": match state.cloud_identity().await["status"].as_str() {
                Some("active") => "pass",
                Some("pending_claim") => "warning",
                Some("disabled") => "warning",
                _ => "error"
            },
            "detail": state.runtime.config.cloud.url
        }),
    ];
    (
        StatusCode::OK,
        Json(json!({
            "capturedAt": chrono::Utc::now().to_rfc3339(),
            "overall": if checks.iter().any(|check| check["status"] == "error") { "error" } else { "healthy" },
            "version": env!("CARGO_PKG_VERSION"),
            "installRoot": state.runtime.install_dir,
            "configPath": state.runtime.config_path,
            "device": state.device,
            "local": {
                "mcpPort": state.runtime.config.filesystem.port,
                "statusPort": state.runtime.config.filesystem.approval_port,
                "trustMode": if state.full_trust { "full" } else { "approval" },
                "pending": state.access.pending_count().await,
                "pendingExecutions": state.execution.pending_count().await,
                "roots": state.access.list_grants().await
            },
            "cloud": state.cloud_identity().await,
            "service": service,
            "checks": checks,
            "settings": {
                "deviceId": state.runtime.config.device.id,
                "deviceName": state.runtime.config.device.name,
                "trustMode": state.runtime.config.filesystem.trust_mode,
                "mcpPort": state.runtime.config.filesystem.port,
                "approvalPort": state.runtime.config.filesystem.approval_port,
                "cloudEnabled": state.runtime.config.cloud.enabled,
                "cloudUrl": state.runtime.config.cloud.url,
                "updateManifest": state.runtime.config.update.manifest,
                "roots": state.runtime.config.filesystem.roots
            }
        })),
    )
}

#[derive(Deserialize)]
struct ActionInput {
    action: String,
}

async fn action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ActionInput>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    if !matches!(input.action.as_str(), "restart" | "stop" | "repair") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported manager action" })),
        );
    }
    match service::spawn_deferred_action(&state.runtime, &input.action) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "action": input.action })),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootInput {
    name: String,
    path: PathBuf,
    mode: RootMode,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInput {
    device_id: String,
    device_name: String,
    trust_mode: String,
    mcp_port: u16,
    approval_port: u16,
    cloud_enabled: bool,
    cloud_url: String,
    update_manifest: String,
    roots: Vec<RootInput>,
}

async fn save_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<SettingsInput>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    let device_id = clean_device_id(&input.device_id);
    if device_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "device ID is invalid" })),
        );
    }
    if input.mcp_port == input.approval_port {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "MCP and status ports must be different" })),
        );
    }
    if !matches!(input.trust_mode.as_str(), "full" | "approval") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "trustMode must be full or approval" })),
        );
    }
    if !input.cloud_url.starts_with("ws://") && !input.cloud_url.starts_with("wss://") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "cloudUrl must use ws:// or wss://" })),
        );
    }
    if input.update_manifest.starts_with("http://") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "remote update manifests must use HTTPS" })),
        );
    }
    let mut config = state.runtime.config.clone();
    config.device.id = device_id;
    config.device.name = input.device_name.trim().to_string();
    config.filesystem.trust_mode = input.trust_mode;
    config.filesystem.port = input.mcp_port;
    config.filesystem.approval_port = input.approval_port;
    config.filesystem.roots = input
        .roots
        .into_iter()
        .filter(|root| !root.name.trim().is_empty() && root.path.is_absolute())
        .map(|root| RootConfig {
            name: root.name.trim().to_string(),
            path: root.path,
            mode: root.mode,
        })
        .collect();
    if config.filesystem.roots.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "at least one absolute local root is required" })),
        );
    }
    config.cloud.enabled = input.cloud_enabled;
    config.cloud.url = input.cloud_url.trim().to_string();
    config.update.manifest = input.update_manifest.trim().to_string();
    match write_json_atomic(&state.runtime.config_path, &config) {
        Ok(()) => {
            let restart = service::spawn_deferred_action(&state.runtime, "restart");
            (
                if restart.is_ok() {
                    StatusCode::OK
                } else {
                    StatusCode::ACCEPTED
                },
                Json(json!({
                    "ok": true,
                    "restartScheduled": restart.is_ok()
                })),
            )
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string(), "code": error.code() })),
        ),
    }
}

async fn logs(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    let root = state.runtime.config.storage.log_dir.clone();
    let mut files = WalkDir::new(&root)
        .max_depth(2)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let relative = entry.path().strip_prefix(&root).ok()?;
            Some(json!({
                "name": relative.to_string_lossy().replace('\\', "/"),
                "length": metadata.len(),
                "modifiedAt": metadata.modified().ok().map(chrono::DateTime::<chrono::Utc>::from).map(|value| value.to_rfc3339())
            }))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        right["modifiedAt"]
            .as_str()
            .cmp(&left["modifiedAt"].as_str())
    });
    (StatusCode::OK, Json(json!({ "logs": files })))
}

async fn log_tail(
    State(state): State<Arc<AppState>>,
    AxumPath(relative): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    let root = state.runtime.config.storage.log_dir.clone();
    let path = root.join(relative.replace('/', "\\"));
    if !is_inside(&path, &root) || !path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "log file not found" })),
        );
    }
    let content = read_tail(&path, 256 * 1024).await;
    (StatusCode::OK, Json(json!({ "content": content })))
}

async fn read_tail(path: &Path, max_bytes: usize) -> String {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let path = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let root = root
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    path == root || path.starts_with(&(root + "\\"))
}
