use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use futures_util::future::join_all;
use hanako_bridge_core::{device::clean_device_id, store::write_json_atomic};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct RouterPaths {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub queue: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterConfig {
    #[serde(default = "schema_one")]
    schema_version: u32,
    #[serde(default)]
    default_device_id: String,
    devices: Vec<DeviceConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceConfig {
    id: String,
    name: String,
    url: String,
    health_url: String,
    #[serde(default)]
    mcp_token: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceStatus {
    online: bool,
    checked_at: String,
    last_seen_at: Option<String>,
    latency_ms: u64,
    version: Option<String>,
    trust_mode: Option<String>,
    device: Value,
    capabilities: Value,
    error: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCache {
    #[serde(default = "schema_one")]
    schema_version: u32,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    source_device_id: String,
    #[serde(default)]
    tools: Vec<Value>,
}

impl Default for ToolCache {
    fn default() -> Self {
        Self {
            schema_version: 1,
            updated_at: String::new(),
            source_device_id: String::new(),
            tools: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueStore {
    #[serde(default = "schema_one")]
    schema_version: u32,
    #[serde(default)]
    items: Vec<QueueItem>,
}

impl Default for QueueStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueItem {
    id: String,
    device_id: String,
    tool: String,
    arguments: Value,
    status: String,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    attempts: u32,
    error: String,
    response: Option<Value>,
}

struct RouterState {
    config: RouterConfig,
    status: HashMap<String, DeviceStatus>,
    tool_cache: ToolCache,
    queue: QueueStore,
}

pub struct RouterService {
    paths: RouterPaths,
    health_interval: Duration,
    client: Client,
    state: RwLock<RouterState>,
    queue_processing: Mutex<bool>,
    remote_port_min: u16,
    remote_port_max: u16,
}

#[derive(Debug)]
struct RouterError {
    code: &'static str,
    message: String,
    device_id: Option<String>,
    requested_devices: Option<Vec<String>>,
}

type RouterResult<T> = Result<T, RouterError>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterInput {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    health_url: String,
    #[serde(default)]
    mcp_token: String,
    remote_port: Option<u16>,
}

impl RouterError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            device_id: None,
            requested_devices: None,
        }
    }

    fn device(code: &'static str, message: impl Into<String>, device_id: &str) -> Self {
        Self {
            code,
            message: message.into(),
            device_id: Some(device_id.to_string()),
            requested_devices: None,
        }
    }
}

impl RouterService {
    pub async fn load(paths: RouterPaths, health_interval: Duration) -> anyhow::Result<Self> {
        let config: RouterConfig = read_required_json(&paths.config)?;
        anyhow::ensure!(
            config.schema_version == 1,
            "unsupported device router config schema"
        );
        let tool_cache = read_optional_json(&paths.cache)?.unwrap_or_default();
        let queue = read_optional_json(&paths.queue)?.unwrap_or_default();
        let client = Client::builder().timeout(Duration::from_secs(35)).build()?;
        Ok(Self {
            paths,
            health_interval,
            client,
            state: RwLock::new(RouterState {
                config: normalize_config(config),
                status: HashMap::new(),
                tool_cache,
                queue,
            }),
            queue_processing: Mutex::new(false),
            remote_port_min: env_u16("HANA_DEVICE_REMOTE_PORT_MIN", 18787).max(1024),
            remote_port_max: env_u16("HANA_DEVICE_REMOTE_PORT_MAX", 19999),
        })
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/devices/register", post(register))
            .route("/mcp", post(mcp))
            .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
            .with_state(Arc::clone(self))
    }

    pub fn start_background_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.health_interval);
            loop {
                interval.tick().await;
                self.refresh_all().await;
            }
        });
    }

    pub async fn refresh_all(&self) -> Vec<Value> {
        self.reload_config().await;
        let devices = {
            self.state
                .read()
                .await
                .config
                .devices
                .iter()
                .filter(|device| device.enabled)
                .cloned()
                .collect::<Vec<_>>()
        };
        let results = join_all(
            devices
                .iter()
                .cloned()
                .map(|device| self.refresh_device(device)),
        )
        .await;
        {
            let mut state = self.state.write().await;
            for (id, status) in results {
                state.status.insert(id, status);
            }
        }
        self.process_queue().await;
        self.public_devices().await
    }

    pub async fn refresh_tools(&self) -> Vec<Value> {
        let devices = {
            self.state
                .read()
                .await
                .config
                .devices
                .iter()
                .filter(|device| device.enabled)
                .cloned()
                .collect::<Vec<_>>()
        };
        for device in devices {
            let (id, status) = self.refresh_device(device.clone()).await;
            self.state.write().await.status.insert(id, status.clone());
            if !status.online {
                continue;
            }
            let response = self
                .call_device(
                    &device,
                    json!({
                        "jsonrpc": "2.0",
                        "id": format!("tools-{}", Uuid::new_v4().simple()),
                        "method": "tools/list",
                        "params": {}
                    }),
                )
                .await;
            let Ok(response) = response else {
                continue;
            };
            let Some(tools) = response.pointer("/result/tools").and_then(Value::as_array) else {
                continue;
            };
            let cache = ToolCache {
                schema_version: 1,
                updated_at: Utc::now().to_rfc3339(),
                source_device_id: device.id,
                tools: tools.iter().cloned().map(adapt_tool).collect(),
            };
            let _ = write_json_atomic(&self.paths.cache, &cache);
            self.state.write().await.tool_cache = cache;
            break;
        }
        self.all_tools().await
    }

    async fn reload_config(&self) {
        let Ok(config) = read_required_json::<RouterConfig>(&self.paths.config) else {
            return;
        };
        self.state.write().await.config = normalize_config(config);
    }

    async fn refresh_device(&self, device: DeviceConfig) -> (String, DeviceStatus) {
        let started = std::time::Instant::now();
        let previous = self.state.read().await.status.get(&device.id).cloned();
        let response = self
            .client
            .get(&device.health_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let status = match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<Value>().await {
                    Ok(health) if health["ok"] == true => DeviceStatus {
                        online: true,
                        checked_at: Utc::now().to_rfc3339(),
                        last_seen_at: Some(Utc::now().to_rfc3339()),
                        latency_ms: started.elapsed().as_millis() as u64,
                        version: health["version"].as_str().map(ToOwned::to_owned),
                        trust_mode: health["trustMode"].as_str().map(ToOwned::to_owned),
                        device: health["device"].clone(),
                        capabilities: health["capabilities"].clone(),
                        error: String::new(),
                    },
                    Ok(_) => offline_status(
                        &device,
                        previous,
                        started.elapsed(),
                        "health response did not report ok",
                    ),
                    Err(error) => {
                        offline_status(&device, previous, started.elapsed(), &error.to_string())
                    }
                }
            }
            Ok(response) => offline_status(
                &device,
                previous,
                started.elapsed(),
                &format!("health returned HTTP {}", response.status()),
            ),
            Err(error) => offline_status(&device, previous, started.elapsed(), &error.to_string()),
        };
        (device.id, status)
    }

    async fn public_devices(&self) -> Vec<Value> {
        let state = self.state.read().await;
        state
            .config
            .devices
            .iter()
            .filter(|device| device.enabled)
            .map(|device| {
                let status = state.status.get(&device.id).cloned().unwrap_or_default();
                json!({
                    "id": device.id,
                    "name": device.name,
                    "default": device.id == state.config.default_device_id,
                    "online": status.online,
                    "checkedAt": nullable_string(&status.checked_at),
                    "lastSeenAt": status.last_seen_at,
                    "latencyMs": status.latency_ms,
                    "version": status.version,
                    "trustMode": status.trust_mode,
                    "hostname": status.device["hostname"],
                    "capabilities": status.capabilities,
                    "error": status.error,
                    "pathPrefix": format!("device://{}/", device.id)
                })
            })
            .collect()
    }

    async fn call_device(&self, device: &DeviceConfig, message: Value) -> RouterResult<Value> {
        let mut request = self.client.post(&device.url).json(&message);
        if !device.mcp_token.is_empty() {
            request = request.bearer_auth(&device.mcp_token);
        }
        let response = request.send().await.map_err(|error| {
            RouterError::device(
                "device_request_failed",
                format!("device {} request failed: {error}", device.id),
                &device.id,
            )
        })?;
        if !response.status().is_success() {
            return Err(RouterError::device(
                "device_request_failed",
                format!("device {} returned HTTP {}", device.id, response.status()),
                &device.id,
            ));
        }
        response.json().await.map_err(|error| {
            RouterError::device(
                "device_request_failed",
                format!("device {} returned invalid JSON: {error}", device.id),
                &device.id,
            )
        })
    }

    async fn all_tools(&self) -> Vec<Value> {
        let mut tools = device_tools();
        tools.extend(self.state.read().await.tool_cache.tools.clone());
        tools
    }

    async fn select_device(&self, arguments: &Value) -> RouterResult<DeviceConfig> {
        let mut requested = BTreeSet::new();
        if let Some(id) = arguments["deviceId"].as_str() {
            requested.insert(id.to_ascii_lowercase());
        }
        extract_device_ids(arguments, &mut requested);
        if requested.len() > 1 {
            return Err(RouterError {
                code: "cross_device_operation_not_supported",
                message: "one tool call cannot target multiple devices".to_string(),
                device_id: None,
                requested_devices: Some(requested.into_iter().collect()),
            });
        }
        let state = self.state.read().await;
        let enabled = state
            .config
            .devices
            .iter()
            .filter(|device| device.enabled)
            .cloned()
            .collect::<Vec<_>>();
        let device_id = requested
            .into_iter()
            .next()
            .or_else(|| (enabled.len() == 1).then(|| enabled[0].id.clone()))
            .ok_or_else(|| {
                RouterError::new(
                    "device_required",
                    "deviceId is required when multiple devices are configured",
                )
            })?;
        let device = enabled
            .into_iter()
            .find(|device| device.id == device_id)
            .ok_or_else(|| {
                RouterError::device(
                    "device_not_found",
                    format!("device is not configured: {device_id}"),
                    &device_id,
                )
            })?;
        let status = state.status.get(&device.id).cloned();
        drop(state);
        let stale = status
            .as_ref()
            .and_then(|status| parse_time(&status.checked_at))
            .is_none_or(|checked| {
                SystemTime::now()
                    .duration_since(checked)
                    .unwrap_or_default()
                    > self.health_interval
            });
        let status = if stale {
            let (id, refreshed) = self.refresh_device(device.clone()).await;
            self.state
                .write()
                .await
                .status
                .insert(id, refreshed.clone());
            refreshed
        } else {
            status.unwrap_or_default()
        };
        if !status.online {
            return Err(RouterError::device(
                "device_offline",
                format!("device is offline: {}", device.id),
                &device.id,
            ));
        }
        Ok(device)
    }

    async fn queue_call(&self, device_id: &str, tool: &str, mut arguments: Value) -> QueueItem {
        if let Some(map) = arguments.as_object_mut() {
            map.remove("deviceId");
            map.remove("queueIfOffline");
        }
        let now = Utc::now().to_rfc3339();
        let item = QueueItem {
            id: format!("queue_{}", Uuid::new_v4()),
            device_id: device_id.to_string(),
            tool: tool.to_string(),
            arguments,
            status: "queued".to_string(),
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            finished_at: None,
            attempts: 0,
            error: String::new(),
            response: None,
        };
        let queue = {
            let mut state = self.state.write().await;
            state.queue.items.push(item.clone());
            if state.queue.items.len() > 1000 {
                while state.queue.items.len() > 1000 {
                    let removable = state.queue.items.iter().position(|entry| {
                        matches!(entry.status.as_str(), "completed" | "failed" | "cancelled")
                    });
                    if let Some(index) = removable {
                        state.queue.items.remove(index);
                    } else {
                        break;
                    }
                }
            }
            state.queue.clone()
        };
        let _ = write_json_atomic(&self.paths.queue, &queue);
        item
    }

    async fn process_queue(&self) {
        {
            let mut processing = self.queue_processing.lock().await;
            if *processing {
                return;
            }
            *processing = true;
        }
        let ids = {
            self.state
                .read()
                .await
                .queue
                .items
                .iter()
                .filter(|item| item.status == "queued")
                .map(|item| item.id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            let pending = {
                let state = self.state.read().await;
                let Some(item) = state.queue.items.iter().find(|item| item.id == id) else {
                    continue;
                };
                let Some(device) = state
                    .config
                    .devices
                    .iter()
                    .find(|device| device.id == item.device_id && device.enabled)
                else {
                    continue;
                };
                if !state
                    .status
                    .get(&device.id)
                    .is_some_and(|status| status.online)
                {
                    continue;
                }
                (item.clone(), device.clone())
            };
            {
                let mut state = self.state.write().await;
                if let Some(item) = state.queue.items.iter_mut().find(|item| item.id == id) {
                    let now = Utc::now().to_rfc3339();
                    item.status = "running".to_string();
                    item.started_at = Some(now.clone());
                    item.updated_at = now;
                    item.attempts += 1;
                }
                let _ = write_json_atomic(&self.paths.queue, &state.queue);
            }
            let response = self
                .call_device(
                    &pending.1,
                    json!({
                        "jsonrpc": "2.0",
                        "id": pending.0.id,
                        "method": "tools/call",
                        "params": {
                            "name": pending.0.tool,
                            "arguments": pending.0.arguments
                        }
                    }),
                )
                .await;
            let retry = matches!(
                response,
                Err(RouterError {
                    code: "device_request_failed",
                    ..
                })
            );
            {
                let mut state = self.state.write().await;
                if let Some(item) = state.queue.items.iter_mut().find(|item| item.id == id) {
                    let now = Utc::now().to_rfc3339();
                    match response {
                        Ok(value) => {
                            item.status = if value.get("error").is_some() {
                                "failed"
                            } else {
                                "completed"
                            }
                            .to_string();
                            item.error = value
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            item.response = Some(value);
                            item.finished_at = Some(now.clone());
                        }
                        Err(error) if retry => {
                            item.status = "queued".to_string();
                            item.error = error.message;
                        }
                        Err(error) => {
                            item.status = "failed".to_string();
                            item.error = error.message;
                            item.finished_at = Some(now.clone());
                        }
                    }
                    item.updated_at = now;
                }
                let _ = write_json_atomic(&self.paths.queue, &state.queue);
            }
            if retry {
                let (device_id, status) = self.refresh_device(pending.1).await;
                self.state.write().await.status.insert(device_id, status);
            }
        }
        *self.queue_processing.lock().await = false;
    }

    async fn register_device(&self, input: RegisterInput) -> RouterResult<Value> {
        let id = clean_device_id(&input.id);
        if id.is_empty() {
            return Err(RouterError::new(
                "device_id_required",
                "device id is required",
            ));
        }
        let explicit_url = normalize_loopback_url(&input.url, "device url")?;
        let explicit_health = normalize_loopback_url(&input.health_url, "device healthUrl")?;
        let mut state = self.state.write().await;
        let remote_port = explicit_url
            .as_ref()
            .and_then(|url| Url::parse(url).ok())
            .and_then(|url| url.port())
            .unwrap_or_else(|| {
                allocate_remote_port(
                    &state.config,
                    &id,
                    input.remote_port,
                    self.remote_port_min,
                    self.remote_port_max,
                )
                .unwrap_or(0)
            });
        if remote_port == 0 {
            return Err(RouterError::new(
                "remote_port_unavailable",
                "no remote tunnel ports are available",
            ));
        }
        let name = if input.name.trim().is_empty() {
            id.clone()
        } else {
            input.name.trim().chars().take(120).collect()
        };
        let url = explicit_url.unwrap_or_else(|| format!("http://127.0.0.1:{remote_port}/mcp"));
        let health_url = explicit_health.unwrap_or_else(|| {
            if url.ends_with("/mcp") {
                format!("{}/health", url.trim_end_matches("/mcp"))
            } else {
                format!("http://127.0.0.1:{remote_port}/health")
            }
        });
        let device = DeviceConfig {
            id: id.clone(),
            name: name.clone(),
            url,
            health_url,
            mcp_token: input.mcp_token.trim().to_string(),
            enabled: true,
        };
        if let Some(existing) = state
            .config
            .devices
            .iter_mut()
            .find(|device| device.id == id)
        {
            *existing = device;
        } else {
            state.config.devices.push(device);
        }
        if state.config.default_device_id.is_empty() {
            state.config.default_device_id = id.clone();
        }
        write_json_atomic(&self.paths.config, &state.config)
            .map_err(|error| RouterError::new("config_write_failed", error.to_string()))?;
        state.status.remove(&id);
        Ok(json!({
            "ok": true,
            "deviceId": id,
            "deviceName": name,
            "remotePort": remote_port,
            "default": state.config.default_device_id == id
        }))
    }

    async fn handle_rpc(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let result = self.handle_rpc_inner(&message).await;
        if message["method"] == "notifications/initialized" {
            return None;
        }
        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32000,
                    "message": error.message,
                    "data": {
                        "code": error.code,
                        "deviceId": error.device_id,
                        "requestedDevices": error.requested_devices
                    }
                }
            }),
        })
    }

    async fn handle_rpc_inner(&self, message: &Value) -> RouterResult<Value> {
        match message["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": {
                    "name": "hanako-local-device-router",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            "notifications/initialized" | "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.refresh_tools().await })),
            "tools/call" => {
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                self.call_tool(
                    &name,
                    arguments,
                    message.get("id").cloned().unwrap_or(Value::Null),
                )
                .await
            }
            method => Err(RouterError::new(
                "method_not_supported",
                format!("unsupported MCP method: {method}"),
            )),
        }
    }

    async fn call_tool(
        &self,
        name: &str,
        mut arguments: Value,
        request_id: Value,
    ) -> RouterResult<Value> {
        match name {
            "local_device.devices" => {
                let devices = if arguments["refresh"] == true {
                    self.refresh_all().await
                } else {
                    self.public_devices().await
                };
                return Ok(content_json(json!({ "devices": devices })));
            }
            "local_device.queue" => {
                let limit = arguments["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
                let state = self.state.read().await;
                let mut items = state
                    .queue
                    .items
                    .iter()
                    .filter(|item| {
                        arguments["queueId"]
                            .as_str()
                            .is_none_or(|value| item.id == value)
                    })
                    .filter(|item| {
                        arguments["deviceId"]
                            .as_str()
                            .is_none_or(|value| item.device_id.eq_ignore_ascii_case(value))
                    })
                    .filter(|item| {
                        arguments["status"]
                            .as_str()
                            .is_none_or(|value| item.status == value)
                    })
                    .rev()
                    .take(limit)
                    .map(public_queue_item)
                    .collect::<Vec<_>>();
                items.shrink_to_fit();
                return Ok(content_json(json!({ "items": items })));
            }
            "local_device.cancel_queued" => {
                let queue_id = arguments["queueId"].as_str().unwrap_or("");
                let queue = {
                    let mut state = self.state.write().await;
                    let Some(item) = state
                        .queue
                        .items
                        .iter_mut()
                        .find(|item| item.id == queue_id)
                    else {
                        return Err(RouterError::new("queue_not_found", "queued call not found"));
                    };
                    if item.status != "queued" {
                        return Err(RouterError::new(
                            "queue_not_cancellable",
                            format!("queued call is already {}", item.status),
                        ));
                    }
                    let now = Utc::now().to_rfc3339();
                    item.status = "cancelled".to_string();
                    item.finished_at = Some(now.clone());
                    item.updated_at = now;
                    let result = public_queue_item(item);
                    let queue = state.queue.clone();
                    (result, queue)
                };
                let _ = write_json_atomic(&self.paths.queue, &queue.1);
                return Ok(content_json(queue.0));
            }
            _ => {}
        }
        let queue_if_offline = arguments["queueIfOffline"] == true;
        let device = match self.select_device(&arguments).await {
            Ok(device) => device,
            Err(error)
                if queue_if_offline
                    && error.code == "device_offline"
                    && error.device_id.is_some() =>
            {
                let queued = self
                    .queue_call(
                        error.device_id.as_deref().unwrap_or_default(),
                        name,
                        arguments,
                    )
                    .await;
                return Ok(content_json(json!({
                    "status": "queued",
                    "queue": public_queue_item(&queued)
                })));
            }
            Err(error) => return Err(error),
        };
        if let Some(map) = arguments.as_object_mut() {
            map.remove("deviceId");
            map.remove("queueIfOffline");
        }
        match self
            .call_device(
                &device,
                json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "tools/call",
                    "params": { "name": name, "arguments": arguments }
                }),
            )
            .await
        {
            Ok(value) => Ok(value),
            Err(error) if queue_if_offline && error.code == "device_request_failed" => {
                let queued = self.queue_call(&device.id, name, arguments).await;
                Ok(content_json(json!({
                    "status": "queued",
                    "queue": public_queue_item(&queued)
                })))
            }
            Err(error) => Err(error),
        }
    }
}

async fn health(State(service): State<Arc<RouterService>>) -> impl IntoResponse {
    let devices = service.refresh_all().await;
    let state = service.state.read().await;
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "devices": devices,
        "toolCount": device_tools().len() + state.tool_cache.tools.len(),
        "queue": {
            "queued": state.queue.items.iter().filter(|item| item.status == "queued").count(),
            "running": state.queue.items.iter().filter(|item| item.status == "running").count()
        }
    }))
}

async fn register(
    State(service): State<Arc<RouterService>>,
    Json(input): Json<RegisterInput>,
) -> impl IntoResponse {
    match service.register_device(input).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.message, "code": error.code })),
        ),
    }
}

async fn mcp(
    State(service): State<Arc<RouterService>>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let response = if let Some(messages) = payload.as_array() {
        let responses = join_all(
            messages
                .iter()
                .cloned()
                .map(|message| service.handle_rpc(message)),
        )
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        Value::Array(responses)
    } else {
        service.handle_rpc(payload).await.unwrap_or(Value::Null)
    };
    (
        StatusCode::OK,
        [(
            "mcp-session-id",
            HeaderValue::from_static("hana-device-router"),
        )],
        Json(response),
    )
}

fn normalize_config(mut config: RouterConfig) -> RouterConfig {
    config.default_device_id = clean_device_id(&config.default_device_id);
    config.devices = config
        .devices
        .into_iter()
        .filter(|device| device.enabled)
        .filter_map(|mut device| {
            device.id = clean_device_id(&device.id);
            if device.id.is_empty() || device.url.is_empty() {
                return None;
            }
            if device.name.trim().is_empty() {
                device.name = device.id.clone();
            }
            Some(device)
        })
        .collect();
    config
}

fn normalize_loopback_url(value: &str, label: &str) -> RouterResult<Option<String>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let url = Url::parse(value.trim()).map_err(|_| {
        RouterError::new("device_url_invalid", format!("{label} must be a valid URL"))
    })?;
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if url.scheme() != "http" || !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err(RouterError::new(
            "device_url_invalid",
            format!("{label} must use a loopback HTTP URL"),
        ));
    }
    Ok(Some(url.to_string()))
}

fn allocate_remote_port(
    config: &RouterConfig,
    device_id: &str,
    requested: Option<u16>,
    min: u16,
    max: u16,
) -> Option<u16> {
    let used = config
        .devices
        .iter()
        .filter(|device| device.id != device_id)
        .filter_map(|device| Url::parse(&device.url).ok()?.port())
        .collect::<BTreeSet<_>>();
    if let Some(requested) = requested
        && requested >= 1024
        && !used.contains(&requested)
    {
        return Some(requested);
    }
    (min..=max).find(|port| !used.contains(port))
}

fn offline_status(
    device: &DeviceConfig,
    previous: Option<DeviceStatus>,
    elapsed: Duration,
    error: &str,
) -> DeviceStatus {
    DeviceStatus {
        online: false,
        checked_at: Utc::now().to_rfc3339(),
        last_seen_at: previous
            .as_ref()
            .and_then(|status| status.last_seen_at.clone()),
        latency_ms: elapsed.as_millis() as u64,
        version: previous.as_ref().and_then(|status| status.version.clone()),
        trust_mode: previous
            .as_ref()
            .and_then(|status| status.trust_mode.clone()),
        device: previous
            .as_ref()
            .map(|status| status.device.clone())
            .unwrap_or_else(|| json!({ "id": device.id, "name": device.name })),
        capabilities: previous
            .map(|status| status.capabilities)
            .unwrap_or_else(|| json!({})),
        error: error.to_string(),
    }
}

fn adapt_tool(mut tool: Value) -> Value {
    // A malformed (non-object) tool entry from a downstream device must not
    // crash tool refresh; leave it untouched rather than panicking.
    let Some(object) = tool.as_object_mut() else {
        return tool;
    };
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    object.insert(
        "description".to_string(),
        Value::String(format!(
            "{description} Route to a specific computer with deviceId or a device://<deviceId>/C:/... path."
        )),
    );
    let input = object
        .entry("inputSchema")
        .or_insert_with(|| json!({ "type": "object", "properties": {} }));
    // If a device advertised a non-object inputSchema, replace it with a valid
    // one rather than unwrapping into a panic.
    if !input.is_object() {
        *input = json!({ "type": "object", "properties": {} });
    }
    let Some(properties) = input.as_object_mut().and_then(|input| {
        input
            .entry("properties")
            .or_insert_with(|| json!({}))
            .as_object_mut()
    }) else {
        return tool;
    };
    properties.insert(
        "deviceId".to_string(),
        json!({
            "type": "string",
            "description": "Target device ID. Optional when a device:// path is present or only one device is configured."
        }),
    );
    properties.insert(
        "queueIfOffline".to_string(),
        json!({
            "type": "boolean",
            "description": "When true, persist this call and run it automatically after the target device reconnects."
        }),
    );
    tool
}

fn device_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "local_device.devices",
            "title": "List local Windows devices",
            "description": "List configured Windows bridge devices, online state, version, latency, and device:// path prefix.",
            "inputSchema": { "type": "object", "properties": { "refresh": { "type": "boolean" } } }
        }),
        json!({
            "name": "local_device.queue",
            "title": "List offline device queue",
            "description": "List queued, running, completed, failed, or cancelled calls for local Windows devices.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "queueId": { "type": "string" },
                    "deviceId": { "type": "string" },
                    "status": { "type": "string" },
                    "limit": { "type": "number" }
                }
            }
        }),
        json!({
            "name": "local_device.cancel_queued",
            "title": "Cancel an offline queued call",
            "description": "Cancel a queued call before it starts running on the target Windows device.",
            "inputSchema": {
                "type": "object",
                "properties": { "queueId": { "type": "string" } },
                "required": ["queueId"]
            }
        }),
    ]
}

fn content_json(value: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
        }]
    })
}

fn public_queue_item(item: &QueueItem) -> Value {
    json!({
        "id": item.id,
        "deviceId": item.device_id,
        "tool": item.tool,
        "status": item.status,
        "createdAt": item.created_at,
        "updatedAt": item.updated_at,
        "startedAt": item.started_at,
        "finishedAt": item.finished_at,
        "attempts": item.attempts,
        "error": item.error,
        "response": item.response
    })
}

fn extract_device_ids(value: &Value, target: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            if let Some(path) = value.strip_prefix("device://")
                && let Some((id, _)) = path.split_once('/')
            {
                target.insert(id.to_ascii_lowercase());
            }
        }
        Value::Array(values) => {
            for value in values {
                extract_device_ids(value, target);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                extract_device_ids(value, target);
            }
        }
        _ => {}
    }
}

fn read_required_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_optional_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn parse_time(value: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc).into())
}

fn nullable_string(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

fn env_u16(name: &str, fallback: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

const fn schema_one() -> u32 {
    1
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_one_device_from_nested_paths() {
        let mut ids = BTreeSet::new();
        extract_device_ids(
            &json!({
                "source": "device://laptop/C:/one.txt",
                "nested": ["device://laptop/D:/two.txt"]
            }),
            &mut ids,
        );
        assert_eq!(ids, BTreeSet::from(["laptop".to_string()]));
    }

    #[test]
    fn allocates_ports_without_reusing_existing_devices() {
        let config = RouterConfig {
            schema_version: 1,
            default_device_id: String::new(),
            devices: vec![DeviceConfig {
                id: "one".to_string(),
                name: "One".to_string(),
                url: "http://127.0.0.1:18787/mcp".to_string(),
                health_url: "http://127.0.0.1:18787/health".to_string(),
                mcp_token: String::new(),
                enabled: true,
            }],
        };
        assert_eq!(
            allocate_remote_port(&config, "two", Some(18787), 18787, 18789),
            Some(18788)
        );
    }

    #[test]
    fn adapted_tools_gain_device_routing_fields() {
        let tool = adapt_tool(json!({
            "name": "local_fs.read_text",
            "description": "Read text.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        assert!(tool["inputSchema"]["properties"]["deviceId"].is_object());
        assert!(tool["inputSchema"]["properties"]["queueIfOffline"].is_object());
    }

    #[test]
    fn adapt_tool_leaves_non_object_values_unchanged() {
        // A downstream device returning a malformed (non-object) tool entry must
        // not crash tool refresh. Previously this hit unreachable!().
        assert_eq!(adapt_tool(json!("not a tool")), json!("not a tool"));
        assert_eq!(adapt_tool(json!(42)), json!(42));
    }

    #[test]
    fn adapt_tool_synthesizes_input_schema_when_missing() {
        let tool = adapt_tool(json!({ "name": "x", "description": "d" }));
        assert!(tool["inputSchema"]["properties"]["deviceId"].is_object());
    }

    // Build a RouterService from an on-disk config fixture so private async
    // methods (select_device, queue_call) can be exercised in-crate.
    async fn service_with_devices(devices: Value) -> RouterService {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("devices.json");
        let config = json!({ "schemaVersion": 1, "devices": devices });
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let paths = RouterPaths {
            config: config_path,
            cache: dir.path().join("tools-cache.json"),
            queue: dir.path().join("offline-queue.json"),
        };
        // Keep the tempdir alive for the duration by leaking it into the paths;
        // the OS cleans %TEMP% and each test uses a unique dir.
        std::mem::forget(dir);
        RouterService::load(paths, Duration::from_secs(30))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn select_device_requires_id_when_no_devices_configured() {
        let service = service_with_devices(json!([])).await;
        let err = service.select_device(&json!({})).await.unwrap_err();
        assert_eq!(err.code, "device_required");
    }

    #[tokio::test]
    async fn select_device_requires_id_with_multiple_devices_and_no_hint() {
        let service = service_with_devices(json!([
            { "id": "a", "name": "A", "url": "http://127.0.0.1:1/mcp", "healthUrl": "http://127.0.0.1:1/health" },
            { "id": "b", "name": "B", "url": "http://127.0.0.1:2/mcp", "healthUrl": "http://127.0.0.1:2/health" }
        ]))
        .await;
        let err = service.select_device(&json!({})).await.unwrap_err();
        assert_eq!(err.code, "device_required");
    }

    #[tokio::test]
    async fn select_device_rejects_conflicting_targets() {
        let service = service_with_devices(json!([
            { "id": "a", "name": "A", "url": "http://127.0.0.1:1/mcp", "healthUrl": "http://127.0.0.1:1/health" }
        ]))
        .await;
        let err = service
            .select_device(&json!({ "deviceId": "a", "path": "device://b/C:/x.txt" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, "cross_device_operation_not_supported");
    }

    #[tokio::test]
    async fn select_device_rejects_unknown_id() {
        let service = service_with_devices(json!([
            { "id": "a", "name": "A", "url": "http://127.0.0.1:1/mcp", "healthUrl": "http://127.0.0.1:1/health" }
        ]))
        .await;
        let err = service
            .select_device(&json!({ "deviceId": "ghost" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, "device_not_found");
    }

    #[tokio::test]
    async fn queue_call_strips_routing_fields() {
        let service = service_with_devices(json!([])).await;
        let item = service
            .queue_call(
                "a",
                "local_fs.read_text",
                json!({ "path": "device://a/C:/x.txt", "deviceId": "a", "queueIfOffline": true }),
            )
            .await;
        assert!(item.arguments.get("deviceId").is_none());
        assert!(item.arguments.get("queueIfOffline").is_none());
        assert_eq!(item.arguments["path"], "device://a/C:/x.txt");
        assert_eq!(item.status, "queued");
    }

    #[tokio::test]
    async fn queue_cap_evicts_terminal_items_when_over_limit() {
        let service = service_with_devices(json!([])).await;
        // Pre-fill with 1000 completed items, then add one more: a terminal item
        // should be evicted so the queue stays at 1000.
        {
            let mut state = service.state.write().await;
            for index in 0..1000 {
                state.queue.items.push(QueueItem {
                    id: format!("done_{index}"),
                    device_id: "a".to_string(),
                    tool: "t".to_string(),
                    arguments: json!({}),
                    status: "completed".to_string(),
                    created_at: String::new(),
                    updated_at: String::new(),
                    started_at: None,
                    finished_at: None,
                    attempts: 0,
                    error: String::new(),
                    response: None,
                });
            }
        }
        service.queue_call("a", "t", json!({})).await;
        let state = service.state.read().await;
        assert_eq!(
            state.queue.items.len(),
            1000,
            "a terminal item is evicted to make room"
        );
    }
}
