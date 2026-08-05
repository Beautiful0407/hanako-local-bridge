use std::{
    collections::HashMap,
    env, fs,
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::Arc,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};
use hanako_bridge_core::{
    BridgeError, BridgeResult, DeviceIdentity,
    store::{load_json, write_json_atomic},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    process::Command,
    sync::{Mutex, RwLock},
};
use uuid::Uuid;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSpec {
    pub runtime: String,
    pub script_path: PathBuf,
    pub script_sha256: String,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_seconds: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionAuthorization {
    pub id: String,
    #[serde(flatten)]
    pub spec: ExecutionSpec,
    pub source: String,
    pub scope: String,
    pub uses_remaining: Option<i64>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_quote: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub id: String,
    #[serde(flatten)]
    pub spec: ExecutionSpec,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_scope: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizationStore {
    schema_version: u32,
    authorizations: Vec<ExecutionAuthorization>,
}

impl Default for AuthorizationStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            authorizations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestStore {
    schema_version: u32,
    requests: Vec<ExecutionRequest>,
}

impl Default for RequestStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            requests: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub id: String,
    pub authorization_id: String,
    pub runtime: String,
    pub script_path: PathBuf,
    pub script_sha256: String,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_seconds: u64,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub error: Option<String>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub pid: Option<u32>,
    pub recovered: bool,
    pub device_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunnerSpec {
    schema_version: u32,
    job_id: String,
    command: PathBuf,
    arguments: Vec<String>,
    cwd: PathBuf,
    timeout_seconds: u64,
    stdout_file: PathBuf,
    stderr_file: PathBuf,
    state_file: PathBuf,
    result_file: PathBuf,
    environment: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunnerState {
    schema_version: u32,
    job_id: String,
    runner_pid: u32,
    child_pid: Option<u32>,
    started_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunnerResult {
    schema_version: u32,
    job_id: String,
    runner_pid: u32,
    child_pid: Option<u32>,
    started_at: String,
    finished_at: String,
    status: String,
    exit_code: Option<i32>,
    signal: Option<String>,
    error: Option<String>,
    timed_out: bool,
    cancelled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub available: bool,
    pub command: Option<PathBuf>,
    pub prefix_arguments: Vec<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub checked_at: u128,
    pub device: DeviceIdentity,
    pub powershell: RuntimeInfo,
    pub python: RuntimeInfo,
}

pub struct ExecutionController {
    project_dir: PathBuf,
    data_dir: PathBuf,
    log_dir: PathBuf,
    job_dir: PathBuf,
    authorization_file: PathBuf,
    requests_file: PathBuf,
    audit_file: PathBuf,
    full_trust: bool,
    allow_chat_authorization: bool,
    chat_grant_minutes: u64,
    max_concurrent_jobs: usize,
    max_output_bytes: usize,
    device: DeviceIdentity,
    approval_url: String,
    authorizations: Mutex<AuthorizationStore>,
    requests: Mutex<RequestStore>,
    jobs: RwLock<HashMap<String, JobSummary>>,
    runtime_cache: Mutex<Option<RuntimeStatus>>,
}

impl ExecutionController {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        log_dir: PathBuf,
        full_trust: bool,
        allow_chat_authorization: bool,
        chat_grant_minutes: u64,
        device: DeviceIdentity,
        approval_port: u16,
    ) -> anyhow::Result<Self> {
        let job_dir = log_dir.join("jobs");
        tokio::fs::create_dir_all(&data_dir).await?;
        tokio::fs::create_dir_all(&log_dir).await?;
        tokio::fs::create_dir_all(&job_dir).await?;
        let authorization_file = data_dir.join("execution-authorizations.json");
        let requests_file = data_dir.join("execution-requests.json");
        let mut authorizations = load_json(&authorization_file, AuthorizationStore::default)?;
        let mut requests = load_json(&requests_file, RequestStore::default)?;
        authorizations
            .authorizations
            .retain(|item| !item.id.is_empty());
        requests.requests.retain(|item| !item.id.is_empty());
        if full_trust {
            let now = Utc::now().to_rfc3339();
            for request in &mut requests.requests {
                if request.status == "pending" {
                    request.status = "bypassed_full_trust".to_string();
                    request.decided_at = Some(now.clone());
                }
            }
        }
        write_json_atomic(&authorization_file, &authorizations)?;
        write_json_atomic(&requests_file, &requests)?;
        let controller = Self {
            project_dir,
            data_dir,
            log_dir: log_dir.clone(),
            job_dir,
            authorization_file,
            requests_file,
            audit_file: log_dir.join("execution-audit.jsonl"),
            full_trust,
            allow_chat_authorization,
            chat_grant_minutes: chat_grant_minutes.clamp(5, 1440),
            max_concurrent_jobs: env_usize("LOCAL_EXEC_MAX_CONCURRENT_JOBS", 2, 1, 8),
            max_output_bytes: env_usize(
                "LOCAL_EXEC_MAX_OUTPUT_BYTES",
                1024 * 1024,
                64 * 1024,
                8 * 1024 * 1024,
            ),
            device,
            approval_url: format!("http://127.0.0.1:{approval_port}/"),
            authorizations: Mutex::new(authorizations),
            requests: Mutex::new(requests),
            jobs: RwLock::new(HashMap::new()),
            runtime_cache: Mutex::new(None),
        };
        controller.recover_jobs().await?;
        Ok(controller)
    }

    pub async fn pending_count(&self) -> usize {
        self.requests
            .lock()
            .await
            .requests
            .iter()
            .filter(|item| item.status == "pending")
            .count()
    }

    pub async fn list_requests(&self) -> Vec<ExecutionRequest> {
        let mut requests = self.requests.lock().await.requests.clone();
        requests.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        requests
    }

    pub async fn list_authorizations(&self) -> Vec<Value> {
        self.authorizations
            .lock()
            .await
            .authorizations
            .iter()
            .filter(|item| authorization_active(item))
            .map(|item| self.public_authorization(item))
            .collect()
    }

    pub async fn request_status(&self, request_id: &str) -> BridgeResult<Value> {
        let request = self
            .requests
            .lock()
            .await
            .requests
            .iter()
            .find(|item| item.id == request_id)
            .cloned()
            .ok_or_else(|| BridgeError::tool("request_not_found", "execution request not found"))?;
        let authorization = if let Some(authorization_id) = &request.authorization_id {
            self.authorizations
                .lock()
                .await
                .authorizations
                .iter()
                .find(|item| item.id == *authorization_id)
                .map(|item| self.public_authorization(item))
        } else {
            None
        };
        Ok(json!({
            "request": request,
            "authorization": authorization,
            "approvalUrl": if request.status == "pending" { Some(self.approval_url.clone()) } else { None }
        }))
    }

    pub async fn request_run(&self, input: &Value) -> BridgeResult<Value> {
        let spec = self.normalize_spec(input).await?;
        if self.full_trust {
            let authorization =
                self.create_authorization(spec, "full_trust", "once", Some(1), 30, None);
            self.authorizations
                .lock()
                .await
                .authorizations
                .push(authorization.clone());
            self.save_authorizations().await?;
            self.audit(json!({
                "action": "full_trust_execution_authorized",
                "authorizationId": authorization.id,
                "scriptPath": authorization.spec.script_path,
                "scriptSha256": authorization.spec.script_sha256,
                "success": true
            }))
            .await;
            return Ok(json!({
                "status": "authorized",
                "trustMode": "full",
                "approvalRequired": false,
                "authorization": self.public_authorization(&authorization)
            }));
        }

        if let Some(existing) = self
            .authorizations
            .lock()
            .await
            .authorizations
            .iter()
            .find(|item| authorization_active(item) && specs_equal(&item.spec, &spec))
            .cloned()
        {
            return Ok(json!({
                "status": "authorized",
                "authorization": self.public_authorization(&existing)
            }));
        }

        if let Some(quote) = input.get("userAuthorizationQuote").and_then(Value::as_str) {
            if !self.allow_chat_authorization {
                return Err(BridgeError::tool(
                    "chat_authorization_disabled",
                    "chat execution authorization is disabled",
                ));
            }
            validate_chat_authorization(&spec, quote)?;
            let authorization = self.create_authorization(
                spec,
                "chat_authorization",
                "once",
                Some(1),
                self.chat_grant_minutes,
                Some(quote.to_string()),
            );
            self.authorizations
                .lock()
                .await
                .authorizations
                .push(authorization.clone());
            self.save_authorizations().await?;
            self.audit(json!({
                "action": "chat_execution_authorized",
                "authorizationId": authorization.id,
                "scriptPath": authorization.spec.script_path,
                "success": true
            }))
            .await;
            return Ok(json!({
                "status": "authorized",
                "authorization": self.public_authorization(&authorization)
            }));
        }

        if let Some(existing) = self
            .requests
            .lock()
            .await
            .requests
            .iter()
            .find(|item| item.status == "pending" && specs_equal(&item.spec, &spec))
            .cloned()
        {
            return Ok(json!({
                "status": "pending",
                "request": existing,
                "approvalUrl": self.approval_url
            }));
        }

        let request = ExecutionRequest {
            id: Uuid::new_v4().to_string(),
            spec,
            status: "pending".to_string(),
            created_at: Utc::now().to_rfc3339(),
            decided_at: None,
            authorization_id: None,
            approved_scope: None,
        };
        self.requests.lock().await.requests.push(request.clone());
        self.save_requests().await?;
        self.audit(json!({
            "action": "execution_requested",
            "requestId": request.id,
            "scriptPath": request.spec.script_path,
            "success": true
        }))
        .await;
        Ok(json!({
            "status": "pending",
            "request": request,
            "approvalUrl": self.approval_url
        }))
    }

    pub async fn approve_request(&self, id: &str, scope: &str) -> BridgeResult<Value> {
        let mut requests = self.requests.lock().await;
        let request = requests
            .requests
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| BridgeError::tool("request_not_found", "execution request not found"))?;
        if request.status != "pending" {
            return Err(BridgeError::tool(
                "request_already_decided",
                format!("request is already {}", request.status),
            ));
        }
        let trusted = scope == "trusted";
        let authorization = self.create_authorization(
            request.spec.clone(),
            "local_approval",
            if trusted { "trusted" } else { "once" },
            if trusted { None } else { Some(1) },
            if trusted { 365 * 24 * 60 } else { 30 },
            None,
        );
        request.status = "approved".to_string();
        request.decided_at = Some(Utc::now().to_rfc3339());
        request.authorization_id = Some(authorization.id.clone());
        request.approved_scope = Some(authorization.scope.clone());
        let request_clone = request.clone();
        write_json_atomic(&self.requests_file, &*requests)?;
        drop(requests);
        self.authorizations
            .lock()
            .await
            .authorizations
            .push(authorization.clone());
        self.save_authorizations().await?;
        self.audit(json!({
            "action": "execution_approved",
            "requestId": request_clone.id,
            "authorizationId": authorization.id,
            "scope": authorization.scope,
            "success": true
        }))
        .await;
        Ok(self.public_authorization(&authorization))
    }

    pub async fn deny_request(&self, id: &str) -> BridgeResult<ExecutionRequest> {
        let mut requests = self.requests.lock().await;
        let request = requests
            .requests
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| BridgeError::tool("request_not_found", "execution request not found"))?;
        if request.status != "pending" {
            return Err(BridgeError::tool(
                "request_already_decided",
                format!("request is already {}", request.status),
            ));
        }
        request.status = "denied".to_string();
        request.decided_at = Some(Utc::now().to_rfc3339());
        let result = request.clone();
        write_json_atomic(&self.requests_file, &*requests)?;
        Ok(result)
    }

    pub async fn revoke_authorization(&self, id: &str) -> BridgeResult<Value> {
        let mut store = self.authorizations.lock().await;
        let item = store
            .authorizations
            .iter_mut()
            .find(|item| item.id == id && authorization_active(item))
            .ok_or_else(|| {
                BridgeError::tool(
                    "authorization_not_found",
                    "execution authorization not found",
                )
            })?;
        item.enabled = false;
        item.updated_at = Utc::now().to_rfc3339();
        let result = self.public_authorization(item);
        write_json_atomic(&self.authorization_file, &*store)?;
        Ok(result)
    }

    // PIDs that terminate must never touch: this process, its running job
    // workers (same exe, spawned with --job-runner), and by-name the manager
    // and maintenance executables. Job workers cannot be told apart from the
    // bridge by name, so protection is PID-based.
    async fn protected_pids(&self) -> std::collections::HashSet<u32> {
        let mut protected = std::collections::HashSet::new();
        protected.insert(std::process::id());
        for job in self.jobs.read().await.values() {
            if job.status == "running"
                && let Some(pid) = job.pid
            {
                protected.insert(pid);
            }
        }
        for proc in list_processes_raw() {
            let lower = proc.name.to_ascii_lowercase();
            if lower == "hanako-manager.exe" || lower == "hanako-maintenance.exe" {
                protected.insert(proc.pid);
            }
        }
        protected
    }

    pub async fn list_processes(&self, arguments: &Value) -> BridgeResult<Value> {
        let filter = arguments
            .get("name")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(200)
            .clamp(1, 5000);
        let mut all = list_processes_raw();
        if let Some(needle) = filter.as_ref() {
            all.retain(|proc| proc.name.to_ascii_lowercase().contains(needle));
        }
        let total = all.len();
        let truncated = total > limit;
        all.truncate(limit);
        Ok(json!({
            "processes": all,
            "total": total,
            "truncated": truncated
        }))
    }

    pub async fn terminate(&self, arguments: &Value) -> BridgeResult<Value> {
        let tree = arguments
            .get("tree")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let confirm = arguments
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let explicit_pid = arguments
            .get("pid")
            .and_then(Value::as_u64)
            .map(|v| v as u32);
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());

        if explicit_pid.is_none() && name.is_none() {
            return Err(BridgeError::tool(
                "target_required",
                "terminate requires either a pid or a name",
            ));
        }

        let protected = self.protected_pids().await;

        // Resolve the candidate PIDs. For a by-name request, snapshot the
        // matching PIDs now; each PID's image name is re-verified at kill time
        // to guard against PID reuse between listing and killing.
        let snapshot = list_processes_raw();
        let mut matched: Vec<u32> = Vec::new();
        if let Some(pid) = explicit_pid {
            matched.push(pid);
        }
        if let Some(needle) = name.as_ref() {
            for proc in &snapshot {
                if proc.name.to_ascii_lowercase().contains(needle) {
                    matched.push(proc.pid);
                }
            }
        }
        matched.sort_unstable();
        matched.dedup();

        let mut protected_hit: Vec<u32> = Vec::new();
        let mut targets: Vec<u32> = Vec::new();
        for pid in &matched {
            if protected.contains(pid) {
                protected_hit.push(*pid);
            } else {
                targets.push(*pid);
            }
        }

        // A by-name request that resolves to more than one PID must be
        // confirmed to avoid an agent killing a whole browser or app family
        // from a vague instruction. A single explicit pid needs no confirm.
        let needs_confirm = name.is_some() && targets.len() > 1 && explicit_pid.is_none();
        if needs_confirm && !confirm {
            return Ok(json!({
                "requiresConfirmation": true,
                "matched": matched,
                "protected": protected_hit,
                "wouldTerminate": targets,
                "message": "multiple processes matched; call again with confirm:true (or a specific pid) to terminate"
            }));
        }

        let expected_name = name.clone();
        let mut terminated: Vec<u32> = Vec::new();
        let mut failed: Vec<Value> = Vec::new();
        let mut not_found = false;
        for pid in targets {
            // Re-verify the image still matches before killing (PID reuse guard).
            if let Some(needle) = expected_name.as_ref() {
                let still_matches = list_processes_raw()
                    .into_iter()
                    .any(|proc| proc.pid == pid && proc.name.to_ascii_lowercase().contains(needle));
                if !still_matches {
                    not_found = true;
                    continue;
                }
            }
            match taskkill_capture(pid, tree) {
                TaskkillOutcome::Terminated => terminated.push(pid),
                TaskkillOutcome::NotFound => not_found = true,
                TaskkillOutcome::Failed(reason) => {
                    failed.push(json!({ "pid": pid, "reason": reason }))
                }
            }
        }

        self.audit(json!({
            "action": "terminate",
            "pid": explicit_pid,
            "name": name,
            "tree": tree,
            "terminated": terminated,
            "failed": failed.len(),
            "protected": protected_hit,
        }))
        .await;

        Ok(json!({
            "terminated": terminated,
            "failed": failed,
            "protected": protected_hit,
            "matched": matched,
            "notFound": not_found
        }))
    }

    pub async fn detect_runtimes(&self, refresh: bool) -> RuntimeStatus {
        let mut cache = self.runtime_cache.lock().await;
        let now = unix_millis();
        if !refresh
            && let Some(existing) = cache.as_ref()
            && now.saturating_sub(existing.checked_at) < 30_000
        {
            return existing.clone();
        }
        let windows = env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let windows_powershell = PathBuf::from(windows)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let powershell = resolve_command(
            env::var("LOCAL_EXEC_POWERSHELL_PATH")
                .ok()
                .as_deref()
                .filter(|value| !value.is_empty()),
        )
        .or_else(|| windows_powershell.is_file().then_some(windows_powershell))
        .or_else(|| resolve_command(Some("pwsh.exe")))
        .or_else(|| resolve_command(Some("powershell.exe")));
        let powershell_version = powershell.as_ref().and_then(|command| {
            probe_version(
                command,
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "$PSVersionTable.PSVersion.ToString()",
                ],
            )
        });

        let mut python_prefix = Vec::new();
        let python = resolve_command(
            env::var("LOCAL_EXEC_PYTHON_PATH")
                .ok()
                .as_deref()
                .filter(|value| !value.is_empty()),
        )
        .or_else(|| {
            let py = resolve_command(Some("py.exe"));
            if py.is_some() {
                python_prefix.push("-3".to_string());
            }
            py
        })
        .or_else(|| resolve_command(Some("python.exe")))
        .or_else(|| resolve_command(Some("python3.exe")));
        let mut python_version_args = python_prefix.clone();
        python_version_args.push("--version".to_string());
        let python_version = python
            .as_ref()
            .and_then(|command| probe_version_owned(command, &python_version_args));
        let status = RuntimeStatus {
            checked_at: now,
            device: self.device.clone(),
            powershell: RuntimeInfo {
                available: powershell.is_some() && powershell_version.is_some(),
                command: powershell,
                prefix_arguments: Vec::new(),
                version: powershell_version,
            },
            python: RuntimeInfo {
                available: python.is_some() && python_version.is_some(),
                command: python,
                prefix_arguments: python_prefix,
                version: python_version,
            },
        };
        *cache = Some(status.clone());
        status
    }

    pub async fn run_authorization(self: &Arc<Self>, id: &str) -> BridgeResult<JobSummary> {
        let active_jobs = self
            .jobs
            .read()
            .await
            .values()
            .filter(|job| matches!(job.status.as_str(), "starting" | "running"))
            .count();
        if active_jobs >= self.max_concurrent_jobs {
            return Err(BridgeError::tool(
                "execution_busy",
                "the local execution concurrency limit has been reached",
            ));
        }
        let authorization = {
            let mut store = self.authorizations.lock().await;
            let item = store
                .authorizations
                .iter_mut()
                .find(|item| item.id == id && authorization_active(item))
                .ok_or_else(|| {
                    BridgeError::tool(
                        "authorization_not_found",
                        "execution authorization not found or expired",
                    )
                })?;
            let current_hash = sha256_file(&item.spec.script_path).await?;
            let expected_hash = item.spec.script_sha256.clone();
            if current_hash != expected_hash {
                item.enabled = false;
                item.updated_at = Utc::now().to_rfc3339();
                write_json_atomic(&self.authorization_file, &*store)?;
                return Err(BridgeError::mismatch(
                    "script_sha256_mismatch",
                    "the script changed after authorization; submit the task again",
                    expected_hash,
                    current_hash,
                ));
            }
            if let Some(remaining) = item.uses_remaining.as_mut() {
                *remaining -= 1;
                if *remaining <= 0 {
                    item.enabled = false;
                }
            }
            item.updated_at = Utc::now().to_rfc3339();
            let result = item.clone();
            write_json_atomic(&self.authorization_file, &*store)?;
            result
        };
        let runtimes = self.detect_runtimes(true).await;
        let runtime = match authorization.spec.runtime.as_str() {
            "powershell" => runtimes.powershell,
            "python" => runtimes.python,
            _ => unreachable!(),
        };
        if !runtime.available {
            return Err(BridgeError::tool(
                "runtime_unavailable",
                format!("{} runtime is not available", authorization.spec.runtime),
            ));
        }
        let command = runtime.command.ok_or_else(|| {
            BridgeError::tool("runtime_unavailable", "runtime command is unavailable")
        })?;
        let job_id = Uuid::new_v4().to_string();
        let stdout_file = self.job_dir.join(format!("{job_id}.stdout.log"));
        let stderr_file = self.job_dir.join(format!("{job_id}.stderr.log"));
        tokio::fs::write(&stdout_file, b"")
            .await
            .map_err(|error| BridgeError::tool("job_start_failed", error.to_string()))?;
        tokio::fs::write(&stderr_file, b"")
            .await
            .map_err(|error| BridgeError::tool("job_start_failed", error.to_string()))?;
        let mut executable_arguments = runtime.prefix_arguments;
        if authorization.spec.runtime == "powershell" {
            executable_arguments.extend([
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
            ]);
        }
        executable_arguments.push(authorization.spec.script_path.to_string_lossy().to_string());
        executable_arguments.extend(authorization.spec.arguments.clone());
        let spec_file = self.job_dir.join(format!("{job_id}.spec.json"));
        let state_file = self.job_dir.join(format!("{job_id}.runner.json"));
        let result_file = self.job_dir.join(format!("{job_id}.result.json"));
        let runner_spec = RunnerSpec {
            schema_version: 1,
            job_id: job_id.clone(),
            command,
            arguments: executable_arguments,
            cwd: authorization.spec.cwd.clone(),
            timeout_seconds: authorization.spec.timeout_seconds,
            stdout_file,
            stderr_file,
            state_file,
            result_file,
            environment: HashMap::from([
                ("PYTHONUTF8".to_string(), "1".to_string()),
                ("PYTHONIOENCODING".to_string(), "utf-8".to_string()),
            ]),
        };
        write_json_atomic(&spec_file, &runner_spec)?;
        let now = Utc::now().to_rfc3339();
        let mut job = JobSummary {
            id: job_id.clone(),
            authorization_id: authorization.id,
            runtime: authorization.spec.runtime,
            script_path: authorization.spec.script_path,
            script_sha256: authorization.spec.script_sha256,
            arguments: authorization.spec.arguments,
            cwd: authorization.spec.cwd,
            timeout_seconds: authorization.spec.timeout_seconds,
            status: "starting".to_string(),
            created_at: now.clone(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            signal: None,
            error: None,
            timed_out: false,
            cancelled: false,
            stdout_truncated: false,
            stderr_truncated: false,
            pid: None,
            recovered: false,
            device_id: self.device.id.clone(),
        };
        self.persist_job(&job)?;
        let current_exe = env::current_exe()
            .map_err(|error| BridgeError::tool("job_start_failed", error.to_string()))?;
        let mut command = StdCommand::new(current_exe);
        command
            .arg("--job-runner")
            .arg(&spec_file)
            .current_dir(&self.project_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        let child = command
            .spawn()
            .map_err(|error| BridgeError::tool("job_start_failed", error.to_string()))?;
        job.pid = Some(child.id());
        job.status = "running".to_string();
        job.started_at = Some(Utc::now().to_rfc3339());
        self.persist_job(&job)?;
        self.jobs.write().await.insert(job_id.clone(), job.clone());
        self.spawn_monitor(job_id);
        self.audit(json!({
            "action": "execution_started",
            "jobId": job.id,
            "authorizationId": job.authorization_id,
            "scriptPath": job.script_path,
            "success": true
        }))
        .await;
        Ok(job)
    }

    pub async fn execute(self: &Arc<Self>, input: &Value) -> BridgeResult<Value> {
        let requested = self.request_run(input).await?;
        if requested.get("status").and_then(Value::as_str) != Some("authorized") {
            return Ok(requested);
        }
        let authorization_id = requested
            .pointer("/authorization/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BridgeError::tool("authorization_not_found", "authorization is missing")
            })?;
        let started = self.run_authorization(authorization_id).await?;
        let job = self.wait_for_job(&started.id).await?;
        let output = self.read_job_output(&started.id, input).await?;
        Ok(json!({
            "status": job.status,
            "authorization": requested["authorization"],
            "job": job,
            "stdout": output["stdout"],
            "stderr": output["stderr"]
        }))
    }

    pub async fn get_job(&self, id: &str) -> BridgeResult<JobSummary> {
        if let Some(job) = self.jobs.read().await.get(id).cloned() {
            return Ok(job);
        }
        load_json(&self.job_dir.join(format!("{id}.json")), || {
            None::<JobSummary>
        })?
        .ok_or_else(|| BridgeError::tool("job_not_found", "execution job not found"))
    }

    pub async fn wait_for_job(&self, id: &str) -> BridgeResult<JobSummary> {
        let initial = self.get_job(id).await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(initial.timeout_seconds + 10);
        loop {
            let job = self.get_job(id).await?;
            if !matches!(job.status.as_str(), "starting" | "running") {
                return Ok(job);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(BridgeError::tool(
                    "job_wait_timeout",
                    "timed out waiting for the local execution job result",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn read_job_output(&self, id: &str, input: &Value) -> BridgeResult<Value> {
        let job = self.get_job(id).await?;
        let max_chars = input
            .get("maxChars")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(256 * 1024)
            .clamp(1024, 2 * 1024 * 1024);
        let stdout = read_tail(&self.job_dir.join(format!("{id}.stdout.log")), max_chars).await;
        let stderr = read_tail(&self.job_dir.join(format!("{id}.stderr.log")), max_chars).await;
        Ok(json!({
            "job": job,
            "stdout": stdout,
            "stderr": stderr,
            "returnedTail": true
        }))
    }

    pub async fn cancel_job(&self, id: &str) -> BridgeResult<JobSummary> {
        let mut job = self.get_job(id).await?;
        if !matches!(job.status.as_str(), "starting" | "running") {
            return Ok(job);
        }
        job.cancelled = true;
        if let Some(pid) = job.pid {
            kill_process_tree(pid);
        }
        job.status = "cancelled".to_string();
        job.finished_at = Some(Utc::now().to_rfc3339());
        self.persist_job(&job)?;
        self.jobs.write().await.insert(id.to_string(), job.clone());
        self.audit(json!({
            "action": "execution_cancel_requested",
            "jobId": id,
            "success": true
        }))
        .await;
        Ok(job)
    }

    pub async fn start_recovered_monitors(self: &Arc<Self>) {
        let active = self
            .jobs
            .read()
            .await
            .values()
            .filter(|job| {
                matches!(job.status.as_str(), "starting" | "running")
                    && job.pid.is_some_and(process_alive)
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        for job_id in active {
            self.spawn_monitor(job_id);
        }
    }

    fn create_authorization(
        &self,
        spec: ExecutionSpec,
        source: &str,
        scope: &str,
        uses_remaining: Option<i64>,
        minutes: u64,
        authorization_quote: Option<String>,
    ) -> ExecutionAuthorization {
        let now = Utc::now();
        ExecutionAuthorization {
            id: Uuid::new_v4().to_string(),
            spec,
            source: source.to_string(),
            scope: scope.to_string(),
            uses_remaining,
            enabled: true,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            expires_at: (scope == "once")
                .then(|| (now + chrono::Duration::minutes(minutes as i64)).to_rfc3339()),
            authorization_quote,
        }
    }

    fn public_authorization(&self, item: &ExecutionAuthorization) -> Value {
        json!({
            "id": item.id,
            "runtime": item.spec.runtime,
            "scriptPath": item.spec.script_path,
            "scriptSha256": item.spec.script_sha256,
            "arguments": item.spec.arguments,
            "cwd": item.spec.cwd,
            "timeoutSeconds": item.spec.timeout_seconds,
            "reason": item.spec.reason,
            "source": item.source,
            "scope": item.scope,
            "usesRemaining": item.uses_remaining,
            "expiresAt": item.expires_at,
            "deviceId": self.device.id
        })
    }

    async fn normalize_spec(&self, input: &Value) -> BridgeResult<ExecutionSpec> {
        let runtime = input
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !matches!(runtime.as_str(), "powershell" | "python") {
            return Err(BridgeError::tool(
                "unsupported_runtime",
                "runtime must be powershell or python",
            ));
        }
        let script_path = local_drive_path(
            input
                .get("scriptPath")
                .and_then(Value::as_str)
                .unwrap_or(""),
            "scriptPath",
            &self.device,
        )?;
        let metadata = tokio::fs::metadata(&script_path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BridgeError::tool("script_not_found", "script file does not exist")
            } else {
                BridgeError::tool("script_stat_failed", error.to_string())
            }
        })?;
        if !metadata.is_file() {
            return Err(BridgeError::tool(
                "script_file_required",
                "scriptPath must point to a file",
            ));
        }
        let extension = script_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if (runtime == "powershell" && extension != "ps1")
            || (runtime == "python" && extension != "py")
        {
            return Err(BridgeError::tool(
                "script_extension_mismatch",
                "script extension does not match the selected runtime",
            ));
        }
        if !self.full_trust
            && (is_inside(&script_path, &self.data_dir) || is_inside(&script_path, &self.log_dir))
        {
            return Err(BridgeError::tool(
                "bridge_control_path",
                "bridge control and audit files cannot be executed",
            ));
        }
        let cwd = if let Some(value) = input.get("cwd").and_then(Value::as_str) {
            local_drive_path(value, "cwd", &self.device)?
        } else {
            script_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(r"C:\"))
        };
        let cwd_metadata = tokio::fs::metadata(&cwd).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BridgeError::tool("cwd_not_found", "working directory does not exist")
            } else {
                BridgeError::tool("cwd_stat_failed", error.to_string())
            }
        })?;
        if !cwd_metadata.is_dir() {
            return Err(BridgeError::tool(
                "cwd_directory_required",
                "cwd must be a directory",
            ));
        }
        let arguments = normalize_arguments(input.get("arguments"))?;
        let timeout_seconds = input
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(1, 1800);
        Ok(ExecutionSpec {
            runtime,
            script_path: script_path.clone(),
            script_sha256: sha256_file(&script_path).await?,
            arguments,
            cwd,
            timeout_seconds,
            reason: input
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(500)
                .collect(),
        })
    }

    async fn save_authorizations(&self) -> BridgeResult<()> {
        let store = self.authorizations.lock().await;
        write_json_atomic(&self.authorization_file, &*store)
    }

    async fn save_requests(&self) -> BridgeResult<()> {
        let store = self.requests.lock().await;
        write_json_atomic(&self.requests_file, &*store)
    }

    fn persist_job(&self, job: &JobSummary) -> BridgeResult<()> {
        write_json_atomic(&self.job_dir.join(format!("{}.json", job.id)), job)
    }

    fn spawn_monitor(self: &Arc<Self>, job_id: String) {
        let controller = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Ok(job) = controller.get_job(&job_id).await else {
                    break;
                };
                if job.finished_at.is_some() {
                    break;
                }
                let result_file = controller.job_dir.join(format!("{job_id}.result.json"));
                if let Ok(Some(result)) = load_json(&result_file, || None::<RunnerResult>) {
                    let _ = controller.finish_job(&job_id, result).await;
                    break;
                }
                if job.pid.is_some_and(|pid| !process_alive(pid)) {
                    tokio::time::sleep(Duration::from_millis(750)).await;
                    if let Ok(Some(result)) = load_json(&result_file, || None::<RunnerResult>) {
                        let _ = controller.finish_job(&job_id, result).await;
                        break;
                    }
                    let result = RunnerResult {
                        schema_version: 1,
                        job_id: job_id.clone(),
                        runner_pid: job.pid.unwrap_or_default(),
                        child_pid: None,
                        started_at: job.started_at.clone().unwrap_or_default(),
                        finished_at: Utc::now().to_rfc3339(),
                        status: "failed".to_string(),
                        exit_code: None,
                        signal: None,
                        error: Some("execution runner exited without writing a result".to_string()),
                        timed_out: false,
                        cancelled: false,
                    };
                    let _ = controller.finish_job(&job_id, result).await;
                    break;
                }
            }
        });
    }

    async fn finish_job(&self, id: &str, result: RunnerResult) -> BridgeResult<()> {
        let mut job = self.get_job(id).await?;
        if job.finished_at.is_some() {
            return Ok(());
        }
        job.status = result.status;
        job.exit_code = result.exit_code;
        job.signal = result.signal;
        job.error = result.error;
        job.timed_out = result.timed_out;
        job.cancelled = result.cancelled || job.cancelled;
        job.finished_at = Some(result.finished_at);
        job.stdout_truncated = trim_file_tail(
            &self.job_dir.join(format!("{id}.stdout.log")),
            self.max_output_bytes,
        )?;
        job.stderr_truncated = trim_file_tail(
            &self.job_dir.join(format!("{id}.stderr.log")),
            self.max_output_bytes,
        )?;
        self.persist_job(&job)?;
        self.jobs.write().await.insert(id.to_string(), job.clone());
        self.audit(json!({
            "action": "execution_finished",
            "jobId": id,
            "status": job.status,
            "exitCode": job.exit_code,
            "timedOut": job.timed_out,
            "cancelled": job.cancelled,
            "recovered": job.recovered,
            "success": job.status == "completed"
        }))
        .await;
        Ok(())
    }

    async fn recover_jobs(&self) -> BridgeResult<()> {
        let entries = fs::read_dir(&self.job_dir)
            .map_err(|error| BridgeError::tool("job_recovery_failed", error.to_string()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !file_name.ends_with(".json")
                || file_name.ends_with(".runner.json")
                || file_name.ends_with(".result.json")
                || file_name.ends_with(".spec.json")
            {
                continue;
            }
            let Some(mut job) = load_json(&path, || None::<JobSummary>)? else {
                continue;
            };
            if !matches!(job.status.as_str(), "starting" | "running") {
                continue;
            }
            let runner_state: Option<RunnerState> = load_json(
                &self.job_dir.join(format!("{}.runner.json", job.id)),
                || None,
            )?;
            if job.pid.is_none() {
                job.pid = runner_state.map(|state| state.runner_pid);
            }
            job.recovered = true;
            self.jobs.write().await.insert(job.id.clone(), job.clone());
            let result: Option<RunnerResult> = load_json(
                &self.job_dir.join(format!("{}.result.json", job.id)),
                || None,
            )?;
            if let Some(result) = result {
                self.finish_job(&job.id, result).await?;
            } else if job.pid.is_some_and(process_alive) {
                // start_recovered_monitors attaches monitors after the controller is inside Arc.
            } else {
                let result = RunnerResult {
                    schema_version: 1,
                    job_id: job.id.clone(),
                    runner_pid: job.pid.unwrap_or_default(),
                    child_pid: None,
                    started_at: job.started_at.clone().unwrap_or_default(),
                    finished_at: Utc::now().to_rfc3339(),
                    status: "failed".to_string(),
                    exit_code: None,
                    signal: None,
                    error: Some(
                        "bridge restarted after the execution runner had already stopped"
                            .to_string(),
                    ),
                    timed_out: false,
                    cancelled: false,
                };
                self.finish_job(&job.id, result).await?;
            }
        }
        Ok(())
    }

    async fn audit(&self, event: Value) {
        let mut record = match event {
            Value::Object(value) => value,
            _ => return,
        };
        record.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );
        let line = match serde_json::to_string(&record) {
            Ok(line) => format!("{line}\n"),
            Err(_) => return,
        };
        let path = self.audit_file.clone();
        let _ =
            tokio::task::spawn_blocking(move || append_rotating(&path, &line, 10 * 1024 * 1024))
                .await;
    }
}

pub async fn run_worker_if_requested() -> anyhow::Result<bool> {
    let mut arguments = env::args_os();
    let _executable = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--job-runner")) {
        return Ok(false);
    }
    let spec_path = arguments
        .next()
        .ok_or_else(|| anyhow::anyhow!("job runner requires a spec file"))?;
    run_worker(Path::new(&spec_path)).await?;
    Ok(true)
}

async fn run_worker(spec_path: &Path) -> anyhow::Result<()> {
    let spec: RunnerSpec = load_json(spec_path, || None::<RunnerSpec>)?
        .ok_or_else(|| anyhow::anyhow!("job runner spec is missing or invalid"))?;
    let started_at = Utc::now().to_rfc3339();
    write_json_atomic(
        &spec.state_file,
        &RunnerState {
            schema_version: 1,
            job_id: spec.job_id.clone(),
            runner_pid: std::process::id(),
            child_pid: None,
            started_at: started_at.clone(),
        },
    )?;
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spec.stdout_file)?;
    let stderr = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spec.stderr_file)?;
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.arguments)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .envs(&spec.environment);
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            write_runner_failure(&spec, &started_at, error.to_string())?;
            return Ok(());
        }
    };
    let child_pid = child.id();
    write_json_atomic(
        &spec.state_file,
        &RunnerState {
            schema_version: 1,
            job_id: spec.job_id.clone(),
            runner_pid: std::process::id(),
            child_pid,
            started_at: started_at.clone(),
        },
    )?;
    let timeout = tokio::time::sleep(Duration::from_secs(spec.timeout_seconds.max(1)));
    tokio::pin!(timeout);
    let (status, timed_out) = tokio::select! {
        result = child.wait() => (result, false),
        () = &mut timeout => {
            if let Some(pid) = child_pid {
                kill_process_tree(pid);
            }
            (child.wait().await, true)
        }
    };
    let (job_status, exit_code, error) = match status {
        Ok(status) if timed_out => ("timed_out", status.code(), None),
        Ok(status) if status.success() => ("completed", status.code(), None),
        Ok(status) => ("failed", status.code(), None),
        Err(error) => ("failed", None, Some(error.to_string())),
    };
    write_json_atomic(
        &spec.result_file,
        &RunnerResult {
            schema_version: 1,
            job_id: spec.job_id,
            runner_pid: std::process::id(),
            child_pid,
            started_at,
            finished_at: Utc::now().to_rfc3339(),
            status: job_status.to_string(),
            exit_code,
            signal: None,
            error,
            timed_out,
            cancelled: false,
        },
    )?;
    Ok(())
}

fn write_runner_failure(spec: &RunnerSpec, started_at: &str, error: String) -> BridgeResult<()> {
    write_json_atomic(
        &spec.result_file,
        &RunnerResult {
            schema_version: 1,
            job_id: spec.job_id.clone(),
            runner_pid: std::process::id(),
            child_pid: None,
            started_at: started_at.to_string(),
            finished_at: Utc::now().to_rfc3339(),
            status: "failed".to_string(),
            exit_code: None,
            signal: None,
            error: Some(error),
            timed_out: false,
            cancelled: false,
        },
    )
}

fn normalize_arguments(value: Option<&Value>) -> BridgeResult<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| BridgeError::tool("invalid_arguments", "arguments must be an array"))?;
    if array.len() > 64 {
        return Err(BridgeError::tool(
            "too_many_arguments",
            "no more than 64 arguments are allowed",
        ));
    }
    let mut total = 0usize;
    let mut result = Vec::with_capacity(array.len());
    for item in array {
        let text = item
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| item.to_string());
        if text.contains('\0') || text.len() > 4096 {
            return Err(BridgeError::tool(
                "invalid_argument",
                "an argument is invalid or too long",
            ));
        }
        total += text.len();
        if total > 32768 {
            return Err(BridgeError::tool(
                "arguments_too_long",
                "combined arguments are too long",
            ));
        }
        result.push(text);
    }
    Ok(result)
}

fn local_drive_path(value: &str, label: &str, device: &DeviceIdentity) -> BridgeResult<PathBuf> {
    let mut raw = value.trim().to_string();
    if let Some(rest) = raw.strip_prefix("device://") {
        let (requested, path) = rest.split_once('/').ok_or_else(|| {
            BridgeError::tool("invalid_local_path", "device path is missing a path")
        })?;
        let requested = requested.to_ascii_lowercase();
        if ![&device.id, &device.name, &device.hostname]
            .iter()
            .any(|item| item.to_ascii_lowercase() == requested)
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
            format!("{label} must be an absolute local Windows drive path"),
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

fn validate_chat_authorization(spec: &ExecutionSpec, quote: &str) -> BridgeResult<()> {
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
            "the user message must not contain a negation and must explicitly authorize execution",
        ));
    }
    // 英文授权词用边界匹配(disallow 等否定-授权重叠不再被误判为授权词),
    // 中文授权词用 contains(中文无空格)。
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
        "permit",
        "permits",
        "permitted",
        "permission",
    ]
    .iter()
    .any(|word| hanako_bridge_core::path::quote_contains_token(&lower, word));
    let has_authorization = authorized_zh || authorized_en;
    let has_execution = [
        "execute",
        "run",
        "launch",
        "\u{6267}\u{884c}",
        "\u{8fd0}\u{884c}",
        "\u{542f}\u{52a8}",
    ]
    .iter()
    .any(|word| lower.contains(word));
    if !has_authorization || !has_execution {
        return Err(BridgeError::tool(
            "explicit_execution_authorization_required",
            "the user message must explicitly authorize execution",
        ));
    }
    // 边界匹配:quote 中必须出现完整脚本路径 token,防止 `C:\data` 匹配 `C:\data2`。
    if !hanako_bridge_core::path::quote_contains_path(quote, &spec.script_path.to_string_lossy()) {
        return Err(BridgeError::tool(
            "authorization_path_not_confirmed",
            "the authorization message must contain the exact absolute script path",
        ));
    }
    for argument in &spec.arguments {
        // 参数区分大小写且要求边界匹配,短参数如 `-f` 不再匹配 `config-file` 内部。
        if !argument.is_empty() && !hanako_bridge_core::path::quote_contains_token(quote, argument)
        {
            return Err(BridgeError::tool(
                "authorization_arguments_not_confirmed",
                format!("the authorization message must contain the exact argument: {argument}"),
            ));
        }
    }
    Ok(())
}

fn authorization_active(item: &ExecutionAuthorization) -> bool {
    if !item.enabled || item.uses_remaining.is_some_and(|remaining| remaining <= 0) {
        return false;
    }
    match item.expires_at.as_deref() {
        // 未设置过期时间:长期有效。
        None => true,
        // 已设置但解析失败:数据异常时 fail-closed,视为已过期。
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|expires| expires > Utc::now())
            .unwrap_or(false),
    }
}

fn specs_equal(left: &ExecutionSpec, right: &ExecutionSpec) -> bool {
    left.runtime == right.runtime
        && paths_equal(&left.script_path, &right.script_path)
        && left.script_sha256 == right.script_sha256
        && paths_equal(&left.cwd, &right.cwd)
        && left.timeout_seconds == right.timeout_seconds
        && left.arguments == right.arguments
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

async fn sha256_file(path: &Path) -> BridgeResult<String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn resolve_command(command: Option<&str>) -> Option<PathBuf> {
    let command = command?.trim();
    if command.is_empty() {
        return None;
    }
    if command.contains(['\\', '/']) {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    let output = StdCommand::new("where.exe")
        .arg(command)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

fn probe_version(command: &Path, arguments: &[&str]) -> Option<String> {
    probe_version_owned(
        command,
        &arguments
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>(),
    )
}

fn probe_version_owned(command: &Path, arguments: &[String]) -> Option<String> {
    let output = StdCommand::new(command)
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    String::from_utf8_lossy(text)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn process_alive(pid: u32) -> bool {
    let output = StdCommand::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
}

fn kill_process_tree(pid: u32) {
    let _ = StdCommand::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcInfo {
    pid: u32,
    name: String,
    session_id: Option<u32>,
}

// Parse `tasklist.exe /FO CSV /NH` output. Each row is quoted CSV:
// "Image Name","PID","Session Name","Session#","Mem Usage"
fn parse_tasklist_csv(text: &str) -> Vec<ProcInfo> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields = parse_csv_row(line);
        if fields.len() < 2 {
            continue;
        }
        let Ok(pid) = fields[1].trim().parse::<u32>() else {
            continue;
        };
        let session_id = fields
            .get(3)
            .and_then(|value| value.trim().parse::<u32>().ok());
        rows.push(ProcInfo {
            pid,
            name: fields[0].clone(),
            session_id,
        });
    }
    rows
}

// Minimal CSV row parser for tasklist output: fields are wrapped in double
// quotes and separated by commas; embedded quotes are not expected in image
// names, but commas can appear so we split on quote boundaries, not commas.
fn parse_csv_row(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

fn list_processes_raw() -> Vec<ProcInfo> {
    let output = StdCommand::new("tasklist.exe")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_tasklist_csv(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

#[derive(Debug)]
enum TaskkillOutcome {
    Terminated,
    NotFound,
    Failed(String),
}

// Classify taskkill result from stderr/stdout text rather than exit codes,
// which vary between "not found" and "access denied".
fn classify_taskkill(stdout: &str, stderr: &str, success: bool) -> TaskkillOutcome {
    if success {
        return TaskkillOutcome::Terminated;
    }
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if combined.contains("not found")
        || combined.contains("not running")
        || combined.contains("could not find")
        || combined.contains("没有找到")
        || combined.contains("找不到")
    {
        return TaskkillOutcome::NotFound;
    }
    if combined.contains("access is denied") || combined.contains("拒绝访问") {
        return TaskkillOutcome::Failed(
            "access denied (target may run at a higher integrity level or require elevation)"
                .to_string(),
        );
    }
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("taskkill failed")
        .to_string();
    TaskkillOutcome::Failed(reason)
}

fn taskkill_capture(pid: u32, tree: bool) -> TaskkillOutcome {
    let mut command = StdCommand::new("taskkill.exe");
    command.arg("/PID").arg(pid.to_string());
    if tree {
        command.arg("/T");
    }
    command.arg("/F").creation_flags(CREATE_NO_WINDOW);
    match command.output() {
        Ok(output) => classify_taskkill(
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
            output.status.success(),
        ),
        Err(error) => TaskkillOutcome::Failed(error.to_string()),
    }
}

async fn read_tail(path: &Path, max_chars: usize) -> String {
    tokio::fs::read_to_string(path)
        .await
        .unwrap_or_default()
        .chars()
        .rev()
        .take(max_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn trim_file_tail(path: &Path, max_bytes: usize) -> BridgeResult<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(BridgeError::tool("log_trim_failed", error.to_string())),
    };
    if metadata.len() <= max_bytes as u64 {
        return Ok(false);
    }
    use std::io::{Read, Seek};
    let mut file = fs::File::open(path)
        .map_err(|error| BridgeError::tool("log_trim_failed", error.to_string()))?;
    file.seek(std::io::SeekFrom::End(-(max_bytes as i64)))
        .map_err(|error| BridgeError::tool("log_trim_failed", error.to_string()))?;
    let mut bytes = Vec::with_capacity(max_bytes);
    file.read_to_end(&mut bytes)
        .map_err(|error| BridgeError::tool("log_trim_failed", error.to_string()))?;
    fs::write(path, bytes)
        .map_err(|error| BridgeError::tool("log_trim_failed", error.to_string()))?;
    Ok(true)
}

fn append_rotating(path: &Path, line: &str, max_bytes: u64) -> std::io::Result<()> {
    if fs::metadata(path)
        .map(|metadata| metadata.len() + line.len() as u64 > max_bytes)
        .unwrap_or(false)
    {
        let backup = PathBuf::from(format!("{}.1", path.display()));
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(path, backup);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

fn env_usize(name: &str, fallback: usize, min: usize, max: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
        .clamp(min, max)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |value| value.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_tasklist_rows_with_spaces_and_missing_fields() {
        let sample = "\"Tabbit Browser.exe\",\"6988\",\"Console\",\"1\",\"123,456 K\"\n\
                      \"svchost.exe\",\"1024\",\"Services\",\"0\",\"8,000 K\"\n\
                      \r\n\
                      \"weird.exe\",\"notanumber\",\"Console\",\"1\",\"0 K\"";
        let rows = parse_tasklist_csv(sample);
        assert_eq!(rows.len(), 2, "the row with a non-numeric PID is skipped");
        assert_eq!(rows[0].name, "Tabbit Browser.exe");
        assert_eq!(rows[0].pid, 6988);
        assert_eq!(rows[0].session_id, Some(1));
        assert_eq!(rows[1].name, "svchost.exe");
        assert_eq!(rows[1].pid, 1024);
    }

    #[test]
    fn csv_row_keeps_commas_inside_quotes() {
        let fields = parse_csv_row("\"a,b\",\"12\",\"Console\",\"1\",\"1,024 K\"");
        assert_eq!(fields[0], "a,b");
        assert_eq!(fields[1], "12");
        assert_eq!(fields[4], "1,024 K");
    }

    #[test]
    fn classifies_taskkill_not_found() {
        let out = classify_taskkill("", "ERROR: The process \"1234\" not found.", false);
        assert!(matches!(out, TaskkillOutcome::NotFound));
    }

    #[test]
    fn classifies_taskkill_access_denied() {
        let out = classify_taskkill("", "ERROR: Access is denied.", false);
        match out {
            TaskkillOutcome::Failed(reason) => assert!(reason.contains("access denied")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn classifies_taskkill_success() {
        let out = classify_taskkill("SUCCESS: ...", "", true);
        assert!(matches!(out, TaskkillOutcome::Terminated));
    }

    #[test]
    fn classifies_unknown_failure_with_stderr_reason() {
        let out = classify_taskkill("", "ERROR: something odd happened", false);
        match out {
            TaskkillOutcome::Failed(reason) => {
                assert!(reason.contains("something odd"))
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
