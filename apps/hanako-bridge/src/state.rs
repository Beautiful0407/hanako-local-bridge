use std::{
    collections::{HashMap, VecDeque},
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use anyhow::Context;
use chrono::Utc;
use hanako_bridge_core::{DeviceIdentity, RuntimeConfig, path::PathResolver};
use rand::{TryRngCore, rngs::OsRng};
use serde_json::{Value, json};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::{access::AccessController, cloud::CloudConnector, execution::ExecutionController};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileFingerprint {
    pub is_dir: bool,
    pub len: u64,
    pub modified_ms: u128,
}

#[derive(Clone, Debug)]
pub struct WatchRecord {
    pub id: String,
    pub public_path: String,
    pub real_path: PathBuf,
    pub grant_id: String,
    pub grant_relative: PathBuf,
    pub recursive: bool,
    pub created_at: String,
    pub sequence: u64,
    pub events: VecDeque<Value>,
    pub snapshot: HashMap<String, FileFingerprint>,
    pub closed: bool,
}

pub struct AppState {
    pub runtime: RuntimeConfig,
    pub device: DeviceIdentity,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub full_trust: bool,
    pub approval_port: u16,
    pub allow_chat_authorization: bool,
    pub resolver: Arc<PathResolver>,
    pub access: Arc<AccessController>,
    pub execution: Arc<ExecutionController>,
    pub cloud: OnceLock<Arc<CloudConnector>>,
    approval_token: Vec<u8>,
    path_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    audit_lock: Mutex<()>,
    pub watches: Mutex<HashMap<String, WatchRecord>>,
}

impl AppState {
    pub async fn new(
        runtime: RuntimeConfig,
        device: DeviceIdentity,
        data_dir: PathBuf,
        log_dir: PathBuf,
        full_trust: bool,
        approval_port: u16,
        allow_chat_authorization: bool,
    ) -> anyhow::Result<Self> {
        let approval_token = load_or_create_token(&data_dir).await?;
        let resolver = Arc::new(PathResolver::new(
            &runtime.config.filesystem.roots,
            full_trust,
            &device.id,
            vec![device.name.clone(), device.hostname.clone()],
        ));
        let access = Arc::new(
            AccessController::new(
                data_dir.clone(),
                log_dir.clone(),
                runtime.config.filesystem.roots.clone(),
                Arc::clone(&resolver),
                full_trust,
                allow_chat_authorization,
                runtime.config.filesystem.chat_grant_minutes,
                device.clone(),
                approval_port,
            )
            .await?,
        );
        let execution = Arc::new(
            ExecutionController::new(
                runtime.install_dir.clone(),
                data_dir.clone(),
                log_dir.clone(),
                full_trust,
                allow_chat_authorization,
                runtime.config.filesystem.chat_grant_minutes,
                device.clone(),
                approval_port,
            )
            .await?,
        );
        execution.start_recovered_monitors().await;
        Ok(Self {
            runtime,
            device,
            data_dir,
            log_dir,
            full_trust,
            approval_port,
            allow_chat_authorization,
            resolver,
            access,
            execution,
            cloud: OnceLock::new(),
            approval_token,
            path_locks: Mutex::new(HashMap::new()),
            audit_lock: Mutex::new(()),
            watches: Mutex::new(HashMap::new()),
        })
    }

    pub fn token_matches(&self, supplied: &[u8]) -> bool {
        if supplied.len() != self.approval_token.len() {
            return false;
        }
        supplied
            .iter()
            .zip(&self.approval_token)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
    }

    pub fn approval_token(&self) -> String {
        String::from_utf8_lossy(&self.approval_token).into_owned()
    }

    pub fn capabilities(&self) -> Value {
        json!({
            "read": true,
            "imageRead": true,
            "imageMimeTypes": ["image/png", "image/jpeg", "image/gif", "image/webp"],
            "maxImageBytes": 8 * 1024 * 1024,
            "write": true,
            "lineRead": true,
            "paginatedList": true,
            "boundedSearch": true,
            "fileWatch": true,
            "deviceIdentity": true,
            "devicePaths": true,
            "appendText": true,
            "exactTextPatch": true,
            "textEncodings": ["utf8", "utf16le", "utf16be"],
            "powershell": true,
            "python": true,
            "asynchronousExecution": true,
            "fullFileAccess": self.full_trust,
            "absoluteWindowsPaths": self.full_trust,
            "approvalRequired": !self.full_trust,
            "localApproval": !self.full_trust,
            "chatAuthorization": !self.full_trust && self.allow_chat_authorization,
            "chatGrantMinutes": self.runtime.config.filesystem.chat_grant_minutes
        })
    }

    pub async fn cloud_identity(&self) -> Value {
        if let Some(cloud) = self.cloud.get() {
            cloud.client_identity().await
        } else {
            json!({
                "status": if self.runtime.config.cloud.enabled { "offline" } else { "disabled" },
                "claimToken": null,
                "publicKeyFingerprint": null,
                "cloudUrl": self.runtime.config.cloud.url,
                "lastConnectedAt": null,
                "lastSeenAt": null,
                "lastError": "cloud connector is not initialized"
            })
        }
    }

    pub async fn audit_mcp(&self, mut event: Value) {
        let Some(object) = event.as_object_mut() else {
            return;
        };
        object.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );
        let Ok(mut line) = serde_json::to_string(&event) else {
            return;
        };
        line.push('\n');
        let path = self.log_dir.join("mcp-audit.jsonl");
        let _guard = self.audit_lock.lock().await;
        let _ =
            tokio::task::spawn_blocking(move || append_rotating(&path, &line, 10 * 1024 * 1024))
                .await;
    }

    pub async fn lock_path(&self, path: &Path) -> OwnedMutexGuard<()> {
        let key = path
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let lock = {
            let mut locks = self.path_locks.lock().await;
            Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
        };
        lock.lock_owned().await
    }
}

async fn load_or_create_token(data_dir: &Path) -> anyhow::Result<Vec<u8>> {
    let path = data_dir.join("approval-token.txt");
    if let Ok(value) = tokio::fs::read_to_string(&path).await {
        let token = value.trim().as_bytes().to_vec();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .context("cannot generate approval token")?;
    let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    tokio::fs::write(&path, format!("{token}\n")).await?;
    Ok(token.into_bytes())
}

fn append_rotating(path: &Path, line: &str, max_bytes: u64) -> std::io::Result<()> {
    if path.exists() && fs_len(path)?.saturating_add(line.len() as u64) > max_bytes {
        for index in (1..=5).rev() {
            let source = path.with_extension(format!("jsonl.{index}"));
            let destination = path.with_extension(format!("jsonl.{}", index + 1));
            if source.exists() {
                let _ = std::fs::remove_file(&destination);
                let _ = std::fs::rename(source, destination);
            }
        }
        let first_backup = path.with_extension("jsonl.1");
        let _ = std::fs::remove_file(&first_backup);
        let _ = std::fs::rename(path, first_backup);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

fn fs_len(path: &Path) -> std::io::Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}
