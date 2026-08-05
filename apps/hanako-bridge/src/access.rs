use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use hanako_bridge_core::{
    BridgeError, BridgeResult, DeviceIdentity,
    config::{RootConfig, RootMode},
    path::{Grant, PathResolver},
    store::{load_json, write_json_atomic},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessGrant {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub mode: RootMode,
    pub enabled: bool,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRequest {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub mode: RootMode,
    pub reason: String,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantStore {
    schema_version: u32,
    grants: Vec<AccessGrant>,
}

impl Default for GrantStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            grants: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestStore {
    schema_version: u32,
    requests: Vec<AccessRequest>,
}

impl Default for RequestStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            requests: Vec::new(),
        }
    }
}

pub struct AccessController {
    grant_file: PathBuf,
    request_file: PathBuf,
    audit_file: PathBuf,
    resolver: Arc<PathResolver>,
    full_trust: bool,
    allow_chat_authorization: bool,
    chat_grant_minutes: u64,
    approval_url: String,
    device: DeviceIdentity,
    grants: Mutex<GrantStore>,
    requests: Mutex<RequestStore>,
}

impl AccessController {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        data_dir: PathBuf,
        log_dir: PathBuf,
        bootstrap_roots: Vec<RootConfig>,
        resolver: Arc<PathResolver>,
        full_trust: bool,
        allow_chat_authorization: bool,
        chat_grant_minutes: u64,
        device: DeviceIdentity,
        approval_port: u16,
    ) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&data_dir).await?;
        tokio::fs::create_dir_all(&log_dir).await?;
        let grant_file = data_dir.join("access-control.json");
        let request_file = data_dir.join("pending-requests.json");
        let mut grants = load_json(&grant_file, GrantStore::default)?;
        let mut requests = load_json(&request_file, RequestStore::default)?;
        grants.grants.retain(|grant| !grant.id.is_empty());
        requests.requests.retain(|request| !request.id.is_empty());
        let now = Utc::now().to_rfc3339();
        for root in bootstrap_roots {
            let id = clean_name(&root.name);
            let existing = grants.grants.iter_mut().find(|grant| {
                grant.source == "bootstrap"
                    && (grant.id.eq_ignore_ascii_case(&id) || paths_equal(&grant.path, &root.path))
            });
            let next = AccessGrant {
                id: if id.is_empty() {
                    format!("root-{}", &Uuid::new_v4().simple().to_string()[..6])
                } else {
                    id
                },
                name: root.name,
                path: root.path,
                mode: root.mode,
                enabled: true,
                source: "bootstrap".to_string(),
                created_at: existing
                    .as_ref()
                    .map_or_else(|| now.clone(), |grant| grant.created_at.clone()),
                updated_at: now.clone(),
                expires_at: None,
            };
            if let Some(existing) = existing {
                *existing = next;
            } else {
                grants.grants.push(next);
            }
        }
        if full_trust {
            for request in &mut requests.requests {
                if request.status == "pending" {
                    request.status = "bypassed_full_trust".to_string();
                    request.decided_at = Some(now.clone());
                }
            }
        }
        write_json_atomic(&grant_file, &grants)?;
        write_json_atomic(&request_file, &requests)?;
        let controller = Self {
            grant_file,
            request_file,
            audit_file: log_dir.join("access-audit.jsonl"),
            resolver,
            full_trust,
            allow_chat_authorization,
            chat_grant_minutes: chat_grant_minutes.clamp(5, 1440),
            approval_url: format!("http://127.0.0.1:{approval_port}/"),
            device,
            grants: Mutex::new(grants),
            requests: Mutex::new(requests),
        };
        controller.refresh_resolver().await;
        Ok(controller)
    }

    pub async fn pending_count(&self) -> usize {
        self.requests
            .lock()
            .await
            .requests
            .iter()
            .filter(|request| request.status == "pending")
            .count()
    }

    pub async fn list_grants(&self) -> Vec<AccessGrant> {
        self.grants
            .lock()
            .await
            .grants
            .iter()
            .filter(|grant| grant_active(grant))
            .cloned()
            .collect()
    }

    pub async fn list_requests(&self) -> Vec<AccessRequest> {
        let mut requests = self.requests.lock().await.requests.clone();
        requests.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        requests
    }

    pub async fn request_access(&self, input: &Value) -> BridgeResult<Value> {
        let path = normalize_local_path(
            input.get("path").and_then(Value::as_str).unwrap_or(""),
            &self.device,
        )?;
        let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BridgeError::tool("path_not_found", "requested path does not exist")
            } else {
                BridgeError::tool("path_stat_failed", error.to_string())
            }
        })?;
        if !metadata.is_dir() {
            return Err(BridgeError::tool(
                "directory_required",
                "access can only be requested for a directory",
            ));
        }
        let mode = parse_mode(input.get("mode").and_then(Value::as_str));
        if self.full_trust {
            self.audit(json!({
                "action": "full_trust_access_authorized",
                "path": path,
                "mode": mode,
                "success": true
            }))
            .await;
            return Ok(json!({
                "status": "authorized",
                "trustMode": "full",
                "approvalRequired": false,
                "path": path,
                "mode": mode
            }));
        }
        if let Some(grant) = self
            .grants
            .lock()
            .await
            .grants
            .iter()
            .find(|grant| {
                grant_active(grant)
                    && is_inside(&path, &grant.path)
                    && mode_rank(grant.mode) >= mode_rank(mode)
            })
            .cloned()
        {
            return Ok(json!({ "status": "authorized", "grant": grant }));
        }
        if let Some(quote) = input.get("userAuthorizationQuote").and_then(Value::as_str) {
            if !self.allow_chat_authorization {
                return Err(BridgeError::tool(
                    "chat_authorization_disabled",
                    "chat file authorization is disabled",
                ));
            }
            validate_chat_authorization(&path, quote)?;
            let grant = self.create_grant(
                input.get("name").and_then(Value::as_str),
                path,
                mode,
                "chat_authorization",
                Some(self.chat_grant_minutes),
            );
            self.grants.lock().await.grants.push(grant.clone());
            self.save_grants().await?;
            self.refresh_resolver().await;
            return Ok(json!({ "status": "authorized", "grant": grant }));
        }
        if let Some(existing) = self
            .requests
            .lock()
            .await
            .requests
            .iter()
            .find(|request| {
                request.status == "pending"
                    && paths_equal(&request.path, &path)
                    && request.mode == mode
            })
            .cloned()
        {
            return Ok(json!({
                "status": "pending",
                "request": existing,
                "approvalUrl": self.approval_url
            }));
        }
        let request = AccessRequest {
            id: Uuid::new_v4().to_string(),
            name: input
                .get("name")
                .and_then(Value::as_str)
                .map(clean_name)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .map(clean_name)
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "LocalFiles".to_string())
                }),
            path,
            mode,
            reason: input
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(500)
                .collect(),
            status: "pending".to_string(),
            created_at: Utc::now().to_rfc3339(),
            decided_at: None,
            grant_id: None,
        };
        self.requests.lock().await.requests.push(request.clone());
        self.save_requests().await?;
        Ok(json!({
            "status": "pending",
            "request": request,
            "approvalUrl": self.approval_url
        }))
    }

    pub async fn access_status(&self, id: &str) -> BridgeResult<Value> {
        let request = self
            .requests
            .lock()
            .await
            .requests
            .iter()
            .find(|request| request.id == id)
            .cloned()
            .ok_or_else(|| BridgeError::tool("request_not_found", "access request not found"))?;
        let grant = if let Some(grant_id) = &request.grant_id {
            self.grants
                .lock()
                .await
                .grants
                .iter()
                .find(|grant| grant.id == *grant_id)
                .cloned()
        } else {
            None
        };
        Ok(json!({
            "request": request,
            "grant": grant,
            "approvalUrl": if request.status == "pending" { Some(self.approval_url.clone()) } else { None }
        }))
    }

    pub async fn approve_request(
        &self,
        id: &str,
        name: Option<&str>,
        mode: Option<&str>,
    ) -> BridgeResult<AccessGrant> {
        let mut requests = self.requests.lock().await;
        let request = requests
            .requests
            .iter_mut()
            .find(|request| request.id == id)
            .ok_or_else(|| BridgeError::tool("request_not_found", "access request not found"))?;
        if request.status != "pending" {
            return Err(BridgeError::tool(
                "request_already_decided",
                format!("request is already {}", request.status),
            ));
        }
        let grant = self.create_grant(
            name.or(Some(&request.name)),
            request.path.clone(),
            mode.map(|value| parse_mode(Some(value)))
                .unwrap_or(request.mode),
            "local_approval",
            None,
        );
        request.status = "approved".to_string();
        request.decided_at = Some(Utc::now().to_rfc3339());
        request.grant_id = Some(grant.id.clone());
        let audit_event = json!({
            "action": "access_approved",
            "requestId": &request.id,
            "path": &request.path,
            "mode": request.mode,
            "grantId": &grant.id
        });
        write_json_atomic(&self.request_file, &*requests)?;
        drop(requests);
        self.grants.lock().await.grants.push(grant.clone());
        self.save_grants().await?;
        self.refresh_resolver().await;
        self.audit(audit_event).await;
        Ok(grant)
    }

    pub async fn deny_request(&self, id: &str) -> BridgeResult<AccessRequest> {
        let mut requests = self.requests.lock().await;
        let request = requests
            .requests
            .iter_mut()
            .find(|request| request.id == id)
            .ok_or_else(|| BridgeError::tool("request_not_found", "access request not found"))?;
        if request.status != "pending" {
            return Err(BridgeError::tool(
                "request_already_decided",
                format!("request is already {}", request.status),
            ));
        }
        request.status = "denied".to_string();
        request.decided_at = Some(Utc::now().to_rfc3339());
        let audit_event = json!({
            "action": "access_denied",
            "requestId": &request.id,
            "path": &request.path,
            "mode": request.mode
        });
        let result = request.clone();
        write_json_atomic(&self.request_file, &*requests)?;
        drop(requests);
        self.audit(audit_event).await;
        Ok(result)
    }

    pub async fn revoke_grant(&self, id: &str) -> BridgeResult<AccessGrant> {
        let mut grants = self.grants.lock().await;
        let grant = grants
            .grants
            .iter_mut()
            .find(|grant| grant.id == id && grant_active(grant))
            .ok_or_else(|| BridgeError::tool("grant_not_found", "access grant not found"))?;
        if grant.source == "bootstrap" {
            return Err(BridgeError::tool(
                "bootstrap_grant",
                "bootstrap grants cannot be revoked",
            ));
        }
        grant.enabled = false;
        grant.updated_at = Utc::now().to_rfc3339();
        let audit_event = json!({
            "action": "access_revoked",
            "grantId": &grant.id,
            "path": &grant.path,
            "mode": grant.mode
        });
        let result = grant.clone();
        write_json_atomic(&self.grant_file, &*grants)?;
        drop(grants);
        self.refresh_resolver().await;
        self.audit(audit_event).await;
        Ok(result)
    }

    fn create_grant(
        &self,
        name: Option<&str>,
        path: PathBuf,
        mode: RootMode,
        source: &str,
        expires_minutes: Option<u64>,
    ) -> AccessGrant {
        let now = Utc::now();
        let base_name = name
            .map(clean_name)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .map(clean_name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| "LocalFiles".to_string());
        AccessGrant {
            id: format!(
                "{}-{}",
                base_name,
                &Uuid::new_v4().simple().to_string()[..6]
            ),
            name: base_name,
            path,
            mode,
            enabled: true,
            source: source.to_string(),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            expires_at: expires_minutes
                .map(|minutes| (now + chrono::Duration::minutes(minutes as i64)).to_rfc3339()),
        }
    }

    async fn refresh_resolver(&self) {
        let grants = self
            .grants
            .lock()
            .await
            .grants
            .iter()
            .filter(|grant| grant_active(grant))
            .map(|grant| Grant {
                id: grant.id.clone(),
                name: grant.name.clone(),
                path: grant.path.clone(),
                mode: grant.mode,
                enabled: grant.enabled,
                source: grant.source.clone(),
            })
            .collect();
        self.resolver.replace_grants(grants);
    }

    async fn save_grants(&self) -> BridgeResult<()> {
        let grants = self.grants.lock().await;
        write_json_atomic(&self.grant_file, &*grants)
    }

    async fn save_requests(&self) -> BridgeResult<()> {
        let requests = self.requests.lock().await;
        write_json_atomic(&self.request_file, &*requests)
    }

    async fn audit(&self, event: Value) {
        let mut object = match event {
            Value::Object(object) => object,
            _ => return,
        };
        object.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );
        let Ok(line) = serde_json::to_string(&object).map(|line| format!("{line}\n")) else {
            return;
        };
        let path = self.audit_file.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            use std::io::Write;
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
                .write_all(line.as_bytes())
        })
        .await;
    }
}

fn parse_mode(value: Option<&str>) -> RootMode {
    if value == Some("read") {
        RootMode::Read
    } else {
        RootMode::ReadWrite
    }
}

fn mode_rank(mode: RootMode) -> u8 {
    match mode {
        RootMode::Read => 1,
        RootMode::ReadWrite => 2,
    }
}

fn grant_active(grant: &AccessGrant) -> bool {
    if !grant.enabled {
        return false;
    }
    match grant.expires_at.as_deref() {
        // 未设置过期时间:长期有效。
        None => true,
        // 已设置但解析失败:数据异常时 fail-closed,视为已过期,
        // 而不是永久有效。
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|expires| expires > Utc::now())
            .unwrap_or(false),
    }
}

fn clean_name(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            result.push(character);
            separator = false;
        } else if !separator && !result.is_empty() {
            result.push('-');
            separator = true;
        }
        if result.len() >= 64 {
            break;
        }
    }
    result.trim_matches('-').to_string()
}

fn normalize_local_path(value: &str, device: &DeviceIdentity) -> BridgeResult<PathBuf> {
    let mut raw = value.trim().to_string();
    if let Some(rest) = raw.strip_prefix("device://") {
        let (requested, path) = rest.split_once('/').ok_or_else(|| {
            BridgeError::tool("invalid_local_path", "device path is missing a path")
        })?;
        if ![&device.id, &device.name, &device.hostname]
            .iter()
            .any(|item| item.eq_ignore_ascii_case(requested))
        {
            return Err(BridgeError::tool(
                "wrong_device",
                format!(
                    "path targets device {requested}, but this bridge is {}",
                    device.id
                ),
            ));
        }
        raw = path.to_string();
    }
    let bytes = raw.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "only absolute local Windows drive paths are allowed",
        ));
    }
    if raw.contains('\0') || raw.starts_with(r"\\.\") || raw.starts_with(r"\\?\") {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "device paths are not allowed",
        ));
    }
    if raw[2..].contains(':') {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "alternate data streams are not allowed",
        ));
    }
    Ok(PathBuf::from(raw))
}

fn validate_chat_authorization(path: &Path, quote: &str) -> BridgeResult<()> {
    let quote = quote.trim();
    if !(8..=2000).contains(&quote.len()) {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the exact current user authorization message is required",
        ));
    }
    let lower = quote.to_ascii_lowercase();
    // 否定表述("不要授权"/"don't allow")不得被当作显式授权。
    if hanako_bridge_core::path::quote_contains_negation(quote) {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the user message must not contain a negation and must explicitly authorize access",
        ));
    }
    // 英文授权词用边界匹配:disallow/deny 内含的 allow/deny 子串不再被
    // 误判为授权词(结构性消除词表穷举无法覆盖的否定-授权重叠);
    // 中文授权词用 contains(中文无空格,边界匹配会误伤"允许访问")。
    let authorized_zh = [
        "\u{6388}\u{6743}", // 授权
        "\u{5141}\u{8bb8}", // 允许
        "\u{540c}\u{610f}", // 同意
        "\u{6279}\u{51c6}", // 批准
        "\u{51c6}\u{8bb8}", // 准许
    ]
    .iter()
    .any(|word| lower.contains(word));
    let authorized_en = [
        "authorize",
        "authorized",
        "authorizing",
        "allow",
        "allowed",
        "allowing",
        "approve",
        "approved",
        "approves",
        "permission",
        "permit",
        "permits",
        "permitted",
    ]
    .iter()
    .any(|word| hanako_bridge_core::path::quote_contains_token(&lower, word));
    let authorized = authorized_zh || authorized_en;
    if !authorized {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the user message must explicitly authorize access",
        ));
    }
    // 边界匹配:quote 中必须出现完整路径 token,防止 `C:\data` 匹配 `C:\data2`。
    if !hanako_bridge_core::path::quote_contains_path(quote, &path.to_string_lossy()) {
        return Err(BridgeError::tool(
            "authorization_path_not_confirmed",
            "the authorization message must contain the exact absolute path",
        ));
    }
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let Ok(path) = hanako_bridge_core::path::normalize_absolute_local(path) else {
        return false;
    };
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
