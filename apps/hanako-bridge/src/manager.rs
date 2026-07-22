use std::{
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
    config::{RootConfig, RootMode, effective_update_channel, effective_update_manifest},
    decode_console_bytes,
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
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/manager", get(manager_page))
        .route("/manager/", get(manager_page))
        .route("/favicon.ico", get(favicon))
        .route("/api/manager/snapshot", get(snapshot))
        .route("/api/manager/action", post(action))
        .route("/api/manager/settings", post(save_settings))
        .route("/api/manager/update/check", get(check_update))
        .route("/api/manager/update/install", post(install_update))
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
    let cloud = state.cloud_identity().await;
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
        cloud_check(&cloud, &state.runtime.config.cloud.url),
        json!({
            "code": "maintenance",
            "status": if maintenance_executable(&state.runtime.install_dir).is_file() { "pass" } else { "error" },
            "detail": maintenance_executable(&state.runtime.install_dir)
        }),
    ];
    let update_state = read_json_value(&state.data_dir.join("update-state.json")).await;
    let update_manifest =
        effective_update_manifest(&state.runtime.config.update, env!("CARGO_PKG_VERSION"));
    let update_channel =
        effective_update_channel(&state.runtime.config.update, env!("CARGO_PKG_VERSION"));
    (
        StatusCode::OK,
        Json(json!({
            "capturedAt": chrono::Utc::now().to_rfc3339(),
            "overall": overall_status(&checks),
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
            "cloud": cloud,
            "service": service,
            "update": {
                "manifest": update_manifest,
                "channel": update_channel,
                "state": update_state
            },
            "metrics": state.metrics(),
            "checks": checks,
            "settings": {
                "deviceId": state.runtime.config.device.id,
                "deviceName": state.runtime.config.device.name,
                "trustMode": state.runtime.config.filesystem.trust_mode,
                "mcpPort": state.runtime.config.filesystem.port,
                "approvalPort": state.runtime.config.filesystem.approval_port,
                "cloudEnabled": state.runtime.config.cloud.enabled,
                "cloudUrl": state.runtime.config.cloud.url,
                "updateManifest": update_manifest,
                "roots": state.runtime.config.filesystem.roots
            }
        })),
    )
}

fn cloud_check(cloud: &Value, cloud_url: &str) -> Value {
    let state = cloud["status"].as_str().unwrap_or("offline");
    let status = match state {
        "active" | "disabled" => "pass",
        "connecting" | "authenticating" | "pending_claim" => "warning",
        _ => "error",
    };
    json!({
        "code": "cloud",
        "status": status,
        "state": state,
        "detail": cloud_url,
        "lastError": cloud["lastError"]
    })
}

fn overall_status(checks: &[Value]) -> &'static str {
    if checks.iter().any(|check| check["status"] == "error") {
        "error"
    } else if checks.iter().any(|check| check["status"] == "warning") {
        "warning"
    } else {
        "healthy"
    }
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

async fn check_update(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    let executable = maintenance_executable(&state.runtime.install_dir);
    let install_root = state.runtime.install_dir.clone();
    let manifest =
        effective_update_manifest(&state.runtime.config.update, env!("CARGO_PKG_VERSION"))
            .to_string();
    match tokio::task::spawn_blocking(move || {
        maintenance_json(
            &executable,
            &[
                "check".into(),
                "--install-root".into(),
                install_root.into_os_string(),
                "--manifest".into(),
                manifest.into(),
                "--current-version".into(),
                env!("CARGO_PKG_VERSION").into(),
            ],
        )
    })
    .await
    {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)),
        Ok(Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallUpdateInput {
    expected_version: String,
}

async fn install_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<InstallUpdateInput>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return forbidden();
    }
    if hanako_bridge_core::update::parse_version(&input.expected_version).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "expectedVersion is invalid" })),
        );
    }
    let executable = maintenance_executable(&state.runtime.install_dir);
    let install_root = state.runtime.install_dir.clone();
    let manifest =
        effective_update_manifest(&state.runtime.config.update, env!("CARGO_PKG_VERSION"))
            .to_string();
    let expected_version = input.expected_version;
    match tokio::task::spawn_blocking(move || {
        maintenance_json(
            &executable,
            &[
                "apply".into(),
                "--install-root".into(),
                install_root.into_os_string(),
                "--manifest".into(),
                manifest.into(),
                "--expected-version".into(),
                expected_version.into(),
            ],
        )
    })
    .await
    {
        Ok(Ok(value)) => (StatusCode::ACCEPTED, Json(value)),
        Ok(Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
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

async fn read_json_value(path: &Path) -> Value {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

fn maintenance_executable(install_root: &Path) -> PathBuf {
    let installed = install_root.join("hanako-maintenance.exe");
    if installed.is_file() {
        return installed;
    }
    install_root.join("hanako-maintenance")
}

fn maintenance_json(executable: &Path, arguments: &[std::ffi::OsString]) -> anyhow::Result<Value> {
    anyhow::ensure!(
        executable.is_file(),
        "Rust maintenance executable is missing: {}",
        executable.display()
    );
    let output = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let stdout = decode_console_bytes(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = decode_console_bytes(&output.stderr).trim().to_string();
        anyhow::bail!("{}", if stderr.is_empty() { stdout } else { stderr });
    }
    serde_json::from_str(&stdout)
        .map_err(|error| anyhow::anyhow!("maintenance returned invalid JSON: {error}: {stdout}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_page_localizes_protocol_statuses_and_diagnostics() {
        assert!(MANAGER_HTML.contains("active: \"已连接\""));
        assert!(MANAGER_HTML.contains("full: \"完全信任\""));
        assert!(MANAGER_HTML.contains("service_task: \"后台任务\""));
        assert!(MANAGER_HTML.contains("pass: \"正常\""));
        assert!(MANAGER_HTML.contains("Windows 拒绝了服务操作"));
        assert!(MANAGER_HTML.contains("waitForServiceRecovery"));
        assert!(MANAGER_HTML.contains("recoveryInProgress"));
        assert!(MANAGER_HTML.contains("apiWithConnectionRetry"));
        assert!(MANAGER_HTML.contains("checkedUpdate = await apiWithConnectionRetry"));
        assert!(MANAGER_HTML.contains("message.textContent = friendlyError(error)"));
        assert!(!MANAGER_HTML.contains("text(\"metric-cloud\", data.cloud.status"));
        assert!(!MANAGER_HTML.contains("${esc(item.code)}"));
        assert!(!MANAGER_HTML.contains("${esc(item.status)}"));
        assert!(!MANAGER_HTML.contains("setTimeout(refresh, 2500)"));
    }

    #[test]
    fn cloud_transitions_are_warnings_and_disabled_is_healthy() {
        let connecting = cloud_check(
            &json!({"status": "connecting", "lastError": null}),
            "wss://example.test/connect",
        );
        let authenticating = cloud_check(
            &json!({"status": "authenticating", "lastError": null}),
            "wss://example.test/connect",
        );
        let disabled = cloud_check(
            &json!({"status": "disabled", "lastError": null}),
            "wss://example.test/connect",
        );
        let offline = cloud_check(
            &json!({"status": "offline", "lastError": "connection refused"}),
            "wss://example.test/connect",
        );
        assert_eq!(connecting["status"], "warning");
        assert_eq!(authenticating["status"], "warning");
        assert_eq!(disabled["status"], "pass");
        assert_eq!(offline["status"], "error");
        assert_eq!(offline["lastError"], "connection refused");
        assert_eq!(overall_status(&[connecting]), "warning");
        assert_eq!(overall_status(&[disabled]), "healthy");
        assert_eq!(overall_status(&[offline]), "error");
    }
}
