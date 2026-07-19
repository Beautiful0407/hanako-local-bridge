use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{BridgeError, BridgeResult, device::clean_device_id};

pub const OFFICIAL_CLOUD_URL: &str = "wss://154-201-69-202.sslip.io/local-bridge/connect";
pub const LEGACY_CLOUD_URL: &str = "ws://154.201.69.202/local-bridge/connect";
pub const OFFICIAL_UPDATE_MANIFEST: &str =
    "https://154-201-69-202.sslip.io/local-bridge/releases/update-manifest.json";
pub const OFFICIAL_ALPHA_UPDATE_MANIFEST: &str =
    "https://154-201-69-202.sslip.io/local-bridge/releases/alpha/update-manifest.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConfig {
    pub schema_version: u32,
    pub device: DeviceConfig,
    pub filesystem: FilesystemConfig,
    pub storage: StorageConfig,
    pub cloud: CloudConfig,
    pub tunnel: TunnelConfig,
    pub service: ServiceConfig,
    pub update: UpdateConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemConfig {
    pub host: String,
    pub port: u16,
    pub approval_port: u16,
    pub trust_mode: String,
    pub allow_chat_authorization: bool,
    pub chat_grant_minutes: u64,
    pub roots: Vec<RootConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RootConfig {
    pub name: String,
    pub path: PathBuf,
    pub mode: RootMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootMode {
    Read,
    ReadWrite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConfig {
    pub enabled: bool,
    pub url: String,
    pub reconnect_min_seconds: u64,
    pub reconnect_max_seconds: u64,
    pub heartbeat_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    pub enabled: bool,
    pub server: String,
    pub user: String,
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub identity_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    pub task_prefix: String,
    pub restart_delay_seconds: u64,
    pub tunnel_retry_min_seconds: u64,
    pub tunnel_retry_max_seconds: u64,
    pub tunnel_health_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateConfig {
    pub manifest: String,
    pub channel: String,
}

fn uses_official_update_feed(update: &UpdateConfig) -> bool {
    let manifest = update.manifest.trim();
    manifest.is_empty()
        || manifest == OFFICIAL_UPDATE_MANIFEST
        || manifest == OFFICIAL_ALPHA_UPDATE_MANIFEST
}

pub fn effective_update_channel<'a>(update: &'a UpdateConfig, current_version: &str) -> &'a str {
    let configured = update.channel.trim();
    if uses_official_update_feed(update)
        && Version::parse(current_version).is_ok_and(|version| !version.pre.is_empty())
        && (configured.is_empty() || configured.eq_ignore_ascii_case("stable"))
    {
        "alpha"
    } else if configured.is_empty() {
        "stable"
    } else {
        configured
    }
}

pub fn effective_update_manifest<'a>(update: &'a UpdateConfig, current_version: &str) -> &'a str {
    if !uses_official_update_feed(update) {
        return update.manifest.trim();
    }
    if effective_update_channel(update, current_version).eq_ignore_ascii_case("alpha") {
        OFFICIAL_ALPHA_UPDATE_MANIFEST
    } else {
        OFFICIAL_UPDATE_MANIFEST
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub install_dir: PathBuf,
    pub config_path: PathBuf,
    pub exists: bool,
    pub config: BridgeConfig,
}

fn windows_home() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn computer_name() -> String {
    env::var("COMPUTERNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            hostname::get()
                .ok()
                .and_then(|value| value.into_string().ok())
        })
        .unwrap_or_else(|| "Windows Device".to_string())
}

fn default_root_path() -> PathBuf {
    let home = windows_home();
    let workspace = home.join("Desktop").join("OH-WorkSpace");
    if workspace.exists() { workspace } else { home }
}

impl BridgeConfig {
    pub fn defaults(install_dir: &Path) -> Self {
        let hostname = computer_name();
        let device_id = {
            let cleaned = clean_device_id(&hostname);
            if cleaned.is_empty() {
                "windows-device".to_string()
            } else {
                cleaned
            }
        };
        let root_path = default_root_path();
        let root_name = root_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("LocalFiles")
            .to_string();
        Self {
            schema_version: 1,
            device: DeviceConfig {
                id: device_id,
                name: hostname,
            },
            filesystem: FilesystemConfig {
                host: "127.0.0.1".to_string(),
                port: 8787,
                approval_port: 8788,
                trust_mode: "full".to_string(),
                allow_chat_authorization: false,
                chat_grant_minutes: 120,
                roots: vec![
                    RootConfig {
                        name: root_name,
                        path: root_path,
                        mode: RootMode::ReadWrite,
                    },
                    RootConfig {
                        name: "HanakoLocalBridge".to_string(),
                        path: install_dir.to_path_buf(),
                        mode: RootMode::Read,
                    },
                ],
            },
            storage: StorageConfig {
                data_dir: install_dir.join("data"),
                log_dir: install_dir.join("logs"),
            },
            cloud: CloudConfig {
                enabled: true,
                url: OFFICIAL_CLOUD_URL.to_string(),
                reconnect_min_seconds: 3,
                reconnect_max_seconds: 60,
                heartbeat_seconds: 25,
            },
            tunnel: TunnelConfig {
                enabled: false,
                server: "154.201.69.202".to_string(),
                user: "root".to_string(),
                local_host: "127.0.0.1".to_string(),
                local_port: 8787,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 18787,
                identity_file: PathBuf::new(),
            },
            service: ServiceConfig {
                task_prefix: "Hanako Local FS".to_string(),
                restart_delay_seconds: 3,
                tunnel_retry_min_seconds: 5,
                tunnel_retry_max_seconds: 60,
                tunnel_health_seconds: 30,
            },
            update: UpdateConfig {
                manifest: OFFICIAL_UPDATE_MANIFEST.to_string(),
                channel: "stable".to_string(),
            },
        }
    }
}

pub fn merge_json(base: &mut Value, override_value: Value) {
    match (base, override_value) {
        (Value::Object(base_map), Value::Object(override_map)) => {
            for (key, value) in override_map {
                match base_map.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base_slot, value) => *base_slot = value,
    }
}

fn environment_map() -> BTreeMap<String, String> {
    env::vars()
        .map(|(key, value)| (key.to_ascii_uppercase(), value))
        .collect()
}

pub fn expand_environment(
    input: &str,
    install_dir: &Path,
    variables: &BTreeMap<String, String>,
) -> String {
    let mut output = input.replace("%INSTALLDIR%", install_dir.to_string_lossy().as_ref());
    let pattern = regex::Regex::new(r"%([^%]+)%").expect("valid environment pattern");
    output = pattern
        .replace_all(&output, |captures: &regex::Captures<'_>| {
            variables
                .get(&captures[1].to_ascii_uppercase())
                .cloned()
                .unwrap_or_else(|| captures[0].to_string())
        })
        .to_string();
    output
}

fn resolve_configured_path(
    input: &str,
    install_dir: &Path,
    variables: &BTreeMap<String, String>,
) -> PathBuf {
    let expanded = PathBuf::from(expand_environment(input, install_dir, variables));
    if expanded.is_absolute() {
        expanded
    } else {
        install_dir.join(expanded)
    }
}

fn normalize_paths(value: &mut Value, install_dir: &Path) {
    let variables = environment_map();
    if let Some(filesystem) = value.get_mut("filesystem").and_then(Value::as_object_mut)
        && let Some(Value::Array(roots)) = filesystem.get_mut("roots")
    {
        let mut seen = std::collections::HashSet::new();
        roots.retain_mut(|root| {
            let Some(root) = root.as_object_mut() else {
                return false;
            };
            let Some(path) = root.get("path").and_then(Value::as_str) else {
                return false;
            };
            let resolved = resolve_configured_path(path, install_dir, &variables);
            let default_name = resolved
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("LocalFiles");
            let name = root
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(default_name)
                .trim()
                .to_string();
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
                return false;
            }
            root.insert("name".to_string(), Value::String(name));
            root.insert(
                "path".to_string(),
                Value::String(resolved.to_string_lossy().to_string()),
            );
            let mode = if root.get("mode").and_then(Value::as_str) == Some("read") {
                "read"
            } else {
                "read_write"
            };
            root.insert("mode".to_string(), Value::String(mode.to_string()));
            true
        });
    }
    if let Some(storage) = value.get_mut("storage").and_then(Value::as_object_mut) {
        for key in ["dataDir", "logDir"] {
            if let Some(Value::String(path)) = storage.get_mut(key) {
                *path = resolve_configured_path(path, install_dir, &variables)
                    .to_string_lossy()
                    .to_string();
            }
        }
    }
    if let Some(tunnel) = value.get_mut("tunnel").and_then(Value::as_object_mut)
        && let Some(Value::String(path)) = tunnel.get_mut("identityFile")
        && !path.trim().is_empty()
    {
        *path = resolve_configured_path(path, install_dir, &variables)
            .to_string_lossy()
            .to_string();
    }
}

fn apply_legacy_migrations(value: &mut Value, source: &Value) {
    let source_has_cloud = source.get("cloud").is_some_and(Value::is_object);
    if !source_has_cloud {
        let server = value
            .pointer("/tunnel/server")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let cloud_url = if !server.is_empty() && server != "154.201.69.202" {
            format!("ws://{server}/local-bridge/connect")
        } else {
            OFFICIAL_CLOUD_URL.to_string()
        };
        if let Some(cloud) = value.get_mut("cloud").and_then(Value::as_object_mut) {
            cloud.insert("enabled".to_string(), Value::Bool(true));
            cloud.insert("url".to_string(), Value::String(cloud_url));
        }
        if let Some(tunnel) = value.get_mut("tunnel").and_then(Value::as_object_mut) {
            tunnel.insert("enabled".to_string(), Value::Bool(false));
        }
    }
    if value.pointer("/cloud/url").and_then(Value::as_str) == Some(LEGACY_CLOUD_URL)
        && let Some(cloud) = value.get_mut("cloud").and_then(Value::as_object_mut)
    {
        cloud.insert(
            "url".to_string(),
            Value::String(OFFICIAL_CLOUD_URL.to_string()),
        );
    }
    let manifest = value
        .pointer("/update/manifest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let legacy_manifest = manifest
        .replace('/', "\\")
        .to_ascii_lowercase()
        .ends_with("\\desktop\\hanako-local-fs-mcp-bridge\\release\\update-manifest.json");
    if (manifest.trim().is_empty() || legacy_manifest)
        && let Some(update) = value.get_mut("update").and_then(Value::as_object_mut)
    {
        update.insert(
            "manifest".to_string(),
            Value::String(OFFICIAL_UPDATE_MANIFEST.to_string()),
        );
    }
}

impl RuntimeConfig {
    pub fn load(install_dir: impl AsRef<Path>, config_path: Option<&Path>) -> BridgeResult<Self> {
        let install_dir = install_dir.as_ref().to_path_buf();
        let config_path = config_path
            .map(Path::to_path_buf)
            .or_else(|| env::var_os("HANA_LOCAL_BRIDGE_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| install_dir.join("config.json"));
        let exists = config_path.exists();
        let defaults = BridgeConfig::defaults(&install_dir);
        let mut merged = serde_json::to_value(defaults).expect("config is serializable");
        let source = match fs::read(&config_path) {
            Ok(bytes) => {
                serde_json::from_slice::<Value>(&bytes).map_err(|source| BridgeError::Json {
                    path: config_path.display().to_string(),
                    source,
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
            Err(source) => {
                return Err(BridgeError::Read {
                    path: config_path.display().to_string(),
                    source,
                });
            }
        };
        merge_json(&mut merged, source.clone());
        apply_legacy_migrations(&mut merged, &source);
        normalize_paths(&mut merged, &install_dir);
        let config = serde_json::from_value(merged).map_err(|source| BridgeError::Json {
            path: config_path.display().to_string(),
            source,
        })?;
        Ok(Self {
            install_dir,
            config_path,
            exists,
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn expands_paths_and_migrates_legacy_cloud_url() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let config_path = dir.path().join("config.json");
        fs::write(
            &config_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 1,
                "device": { "id": "Configured Device", "name": "Configured name" },
                "filesystem": {
                    "port": 29123,
                    "roots": [
                        { "name": "Root", "path": "root", "mode": "read_write" },
                        { "name": "Install", "path": "%INSTALLDIR%", "mode": "read" }
                    ]
                },
                "storage": { "dataDir": "state/data", "logDir": "state/logs" },
                "cloud": {
                    "enabled": true,
                    "url": LEGACY_CLOUD_URL
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let runtime = RuntimeConfig::load(dir.path(), Some(&config_path)).unwrap();

        assert!(runtime.exists);
        assert_eq!(runtime.config.filesystem.port, 29123);
        assert_eq!(runtime.config.filesystem.approval_port, 8788);
        assert_eq!(runtime.config.filesystem.roots[0].path, root);
        assert_eq!(runtime.config.filesystem.roots[1].path, dir.path());
        assert_eq!(runtime.config.cloud.url, OFFICIAL_CLOUD_URL);
        assert_eq!(runtime.config.update.manifest, OFFICIAL_UPDATE_MANIFEST);
    }

    #[test]
    fn leaves_unknown_environment_variables_untouched() {
        let variables = BTreeMap::from([("KNOWN".to_string(), "value".to_string())]);
        assert_eq!(
            expand_environment("%KNOWN%/%MISSING%", Path::new("C:/app"), &variables),
            "value/%MISSING%"
        );
    }

    #[test]
    fn official_update_manifest_tracks_the_effective_release_channel() {
        let mut update = UpdateConfig {
            manifest: OFFICIAL_UPDATE_MANIFEST.to_string(),
            channel: "stable".to_string(),
        };

        assert_eq!(
            effective_update_manifest(&update, "2.0.0"),
            OFFICIAL_UPDATE_MANIFEST
        );
        assert_eq!(effective_update_channel(&update, "2.0.0"), "stable");
        assert_eq!(
            effective_update_manifest(&update, "2.0.0-alpha.7"),
            OFFICIAL_ALPHA_UPDATE_MANIFEST
        );
        assert_eq!(effective_update_channel(&update, "2.0.0-alpha.7"), "alpha");

        update.channel = "alpha".to_string();
        assert_eq!(
            effective_update_manifest(&update, "1.4.9"),
            OFFICIAL_ALPHA_UPDATE_MANIFEST
        );
        assert_eq!(effective_update_channel(&update, "1.4.9"), "alpha");
    }

    #[test]
    fn custom_update_manifest_is_never_rewritten() {
        let update = UpdateConfig {
            manifest: "https://updates.example.test/custom.json".to_string(),
            channel: "stable".to_string(),
        };

        assert_eq!(
            effective_update_manifest(&update, "2.0.0-alpha.7"),
            "https://updates.example.test/custom.json"
        );
        assert_eq!(effective_update_channel(&update, "2.0.0-alpha.7"), "stable");
    }
}
