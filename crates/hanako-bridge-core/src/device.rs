use std::{
    env,
    path::{Path, PathBuf},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{BridgeResult, store};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDeviceIdentity {
    #[serde(default = "schema_version")]
    schema_version: u32,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub platform: String,
    pub updated_at: String,
}

fn schema_version() -> u32 {
    1
}

pub fn clean_device_id(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.trim().to_ascii_lowercase().chars() {
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

impl DeviceIdentity {
    pub fn load(data_dir: &Path, configured_id: &str, configured_name: &str) -> BridgeResult<Self> {
        let path = data_dir.join("device.json");
        let stored: StoredDeviceIdentity = store::load_json(&path, StoredDeviceIdentity::default)?;
        let hostname = hostname::get()
            .ok()
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "Windows Device".to_string());
        let id = [
            env::var("LOCAL_AGENT_DEVICE_ID").unwrap_or_default(),
            configured_id.to_string(),
            stored.id,
            env::var("COMPUTERNAME").unwrap_or_default(),
            hostname.clone(),
        ]
        .into_iter()
        .map(|value| clean_device_id(&value))
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "windows-device".to_string());
        let name = [
            env::var("LOCAL_AGENT_DEVICE_NAME").unwrap_or_default(),
            configured_name.to_string(),
            stored.name,
            env::var("COMPUTERNAME").unwrap_or_default(),
            hostname.clone(),
            id.clone(),
        ]
        .into_iter()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| id.clone());
        let identity = Self {
            schema_version: 1,
            id,
            name,
            hostname,
            platform: "win32".to_string(),
            updated_at: Utc::now().to_rfc3339(),
        };
        store::write_json_atomic(&path, &identity)?;
        Ok(identity)
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("device.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_device_ids_like_the_node_bridge() {
        assert_eq!(clean_device_id(" My Device / 01 "), "my-device-01");
        assert_eq!(clean_device_id("___"), "___");
    }
}
