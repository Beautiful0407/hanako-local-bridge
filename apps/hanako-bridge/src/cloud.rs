use std::{
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};

use base64::Engine;
use chrono::Utc;
use ed25519_dalek::{
    Signer, SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use futures_util::{SinkExt, StreamExt};
use hanako_bridge_core::{
    BridgeResult, DeviceIdentity,
    config::CloudConfig,
    store::{load_json, write_json_atomic},
};
use pkcs8::LineEnding;
use rand::{TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::{mcp, state::AppState};

const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudIdentity {
    schema_version: u32,
    device_id: String,
    public_key: String,
    private_key: String,
    public_key_fingerprint: String,
    claim_token: String,
    credential: String,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug)]
struct CloudState {
    identity: CloudIdentity,
    status: String,
    last_connected_at: Option<String>,
    last_seen_at: Option<String>,
    last_error: Option<String>,
}

pub struct CloudConnector {
    config: CloudConfig,
    identity_path: PathBuf,
    device: DeviceIdentity,
    version: String,
    app_state: Weak<AppState>,
    state: RwLock<CloudState>,
}

impl CloudConnector {
    pub async fn new(
        config: CloudConfig,
        data_dir: PathBuf,
        device: DeviceIdentity,
        version: &str,
        app_state: Weak<AppState>,
    ) -> anyhow::Result<Self> {
        let identity_path = data_dir.join("cloud-identity.json");
        let identity = load_cloud_identity(&identity_path, &device.id)?;
        Ok(Self {
            config,
            identity_path,
            device,
            version: version.to_string(),
            app_state,
            state: RwLock::new(CloudState {
                identity,
                status: "offline".to_string(),
                last_connected_at: None,
                last_seen_at: None,
                last_error: None,
            }),
        }
        .with_initial_status())
    }

    fn with_initial_status(self) -> Self {
        if !self.config.enabled
            && let Ok(mut state) = self.state.try_write()
        {
            state.status = "disabled".to_string();
        }
        self
    }

    pub fn start(self: &Arc<Self>) {
        if !self.config.enabled {
            return;
        }
        let connector = Arc::clone(self);
        tokio::spawn(async move {
            connector.run().await;
        });
    }

    pub async fn client_identity(&self) -> Value {
        let state = self.state.read().await;
        json!({
            "status": state.status,
            "claimToken": if state.identity.credential.is_empty() {
                Value::String(state.identity.claim_token.clone())
            } else {
                Value::Null
            },
            "publicKeyFingerprint": state.identity.public_key_fingerprint,
            "cloudUrl": self.config.url,
            "lastConnectedAt": state.last_connected_at,
            "lastSeenAt": state.last_seen_at,
            "lastError": state.last_error
        })
    }

    async fn run(self: Arc<Self>) {
        let mut retry_seconds = self.config.reconnect_min_seconds.max(2);
        loop {
            {
                let mut state = self.state.write().await;
                state.status = "connecting".to_string();
                state.last_error = None;
            }
            match self.connect_once().await {
                Ok(()) => {
                    retry_seconds = self.config.reconnect_min_seconds.max(2);
                }
                Err(error) => {
                    let mut state = self.state.write().await;
                    state.status = "offline".to_string();
                    state.last_error = Some(error.to_string());
                }
            }
            tokio::time::sleep(Duration::from_secs(retry_seconds)).await;
            retry_seconds =
                (retry_seconds * 2).min(self.config.reconnect_max_seconds.max(retry_seconds));
        }
    }

    async fn connect_once(&self) -> anyhow::Result<()> {
        if !self.config.url.starts_with("ws://") && !self.config.url.starts_with("wss://") {
            anyhow::bail!("cloud.url must use ws:// or wss://");
        }
        let request = self.config.url.clone().into_client_request()?;
        let (socket, _) = connect_async(request).await?;
        let (mut writer, mut reader) = socket.split();
        {
            let mut state = self.state.write().await;
            state.status = "authenticating".to_string();
            state.last_connected_at = Some(Utc::now().to_rfc3339());
            state.last_error = None;
        }
        writer
            .send(Message::Text(
                self.hello_message().await?.to_string().into(),
            ))
            .await?;
        let mut heartbeat =
            tokio::time::interval(Duration::from_secs(self.config.heartbeat_seconds.max(10)));
        heartbeat.tick().await;

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    writer.send(Message::Text(json!({
                        "type": "heartbeat",
                        "sentAt": Utc::now().to_rfc3339()
                    }).to_string().into())).await?;
                }
                message = reader.next() => {
                    let Some(message) = message else {
                        anyhow::bail!("cloud websocket closed");
                    };
                    let message = message?;
                    match message {
                        Message::Text(text) => {
                            let parsed: Value = serde_json::from_str(&text)?;
                            {
                                let mut state = self.state.write().await;
                                state.last_seen_at = Some(Utc::now().to_rfc3339());
                            }
                            if let Some(response) = self.handle_message(parsed).await? {
                                writer.send(Message::Text(response.to_string().into())).await?;
                            }
                        }
                        Message::Ping(bytes) => writer.send(Message::Pong(bytes)).await?,
                        Message::Close(_) => anyhow::bail!("cloud websocket closed"),
                        _ => {}
                    }
                }
            }
        }
    }

    async fn hello_message(&self) -> anyhow::Result<Value> {
        let state = self.state.read().await;
        let identity = &state.identity;
        let signing_key = SigningKey::from_pkcs8_pem(&identity.private_key)?;
        let nonce = random_token(24)?;
        let signature = signing_key.sign(nonce.as_bytes());
        let capabilities = self
            .app_state
            .upgrade()
            .map_or_else(|| json!({}), |state| state.capabilities());
        Ok(json!({
            "type": "hello",
            "protocolVersion": PROTOCOL_VERSION,
            "device": {
                "id": self.device.id,
                "name": self.device.name,
                "hostname": self.device.hostname,
                "platform": "win32",
                "version": self.version
            },
            "capabilities": capabilities,
            "publicKey": identity.public_key,
            "publicKeyFingerprint": identity.public_key_fingerprint,
            "proof": {
                "nonce": nonce,
                "signature": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
            },
            "claimToken": if identity.credential.is_empty() { identity.claim_token.clone() } else { String::new() },
            "credential": identity.credential
        }))
    }

    async fn handle_message(&self, message: Value) -> anyhow::Result<Option<Value>> {
        match message.get("type").and_then(Value::as_str).unwrap_or("") {
            "hello_ack" => {
                let mut state = self.state.write().await;
                state.status = if message.get("status").and_then(Value::as_str) == Some("active") {
                    "active".to_string()
                } else {
                    "pending_claim".to_string()
                };
                Ok(None)
            }
            "approved" => {
                let credential = message
                    .get("credential")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if credential.is_empty() {
                    return Ok(None);
                }
                let mut state = self.state.write().await;
                state.identity.credential = credential.to_string();
                state.identity.claim_token.clear();
                state.identity.updated_at = Utc::now().to_rfc3339();
                write_json_atomic(&self.identity_path, &state.identity)?;
                state.status = "active".to_string();
                Ok(None)
            }
            "revoked" => {
                let mut state = self.state.write().await;
                state.identity.credential.clear();
                state.identity.claim_token = random_token(32)?;
                state.identity.updated_at = Utc::now().to_rfc3339();
                write_json_atomic(&self.identity_path, &state.identity)?;
                state.status = "pending_claim".to_string();
                drop(state);
                Ok(Some(self.hello_message().await?))
            }
            "ping" => Ok(Some(json!({
                "type": "pong",
                "sentAt": Utc::now().to_rfc3339()
            }))),
            "rpc_request" => {
                let request_id = message
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if request_id.is_empty() {
                    return Ok(None);
                }
                let Some(state) = self.app_state.upgrade() else {
                    return Ok(Some(json!({
                        "type": "rpc_response",
                        "requestId": request_id,
                        "error": {
                            "code": "local_rpc_failed",
                            "message": "local bridge state is unavailable"
                        }
                    })));
                };
                let response = mcp::handle_payload(
                    state,
                    message.get("payload").cloned().unwrap_or(Value::Null),
                )
                .await;
                Ok(Some(json!({
                    "type": "rpc_response",
                    "requestId": request_id,
                    "response": response
                })))
            }
            _ => Ok(None),
        }
    }
}

fn load_cloud_identity(path: &Path, device_id: &str) -> BridgeResult<CloudIdentity> {
    let stored = load_json(path, || Option::<CloudIdentity>::None)?;
    let identity = match stored {
        Some(identity)
            if identity.schema_version == 1
                && !identity.public_key.is_empty()
                && !identity.private_key.is_empty()
                && SigningKey::from_pkcs8_pem(&identity.private_key).is_ok() =>
        {
            normalize_identity(identity, device_id)?
        }
        _ => create_identity(device_id)?,
    };
    write_json_atomic(path, &identity)?;
    Ok(identity)
}

fn normalize_identity(mut identity: CloudIdentity, device_id: &str) -> BridgeResult<CloudIdentity> {
    identity.device_id = device_id.to_string();
    if identity.public_key_fingerprint.is_empty() {
        identity.public_key_fingerprint = fingerprint(&identity.public_key);
    }
    if identity.claim_token.is_empty() && identity.credential.is_empty() {
        identity.claim_token = random_token(32)?;
    }
    if identity.created_at.is_empty() {
        identity.created_at = Utc::now().to_rfc3339();
    }
    if identity.updated_at.is_empty() {
        identity.updated_at = Utc::now().to_rfc3339();
    }
    Ok(identity)
}

fn create_identity(device_id: &str) -> BridgeResult<CloudIdentity> {
    let mut seed = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut seed)
        .map_err(|error| anyhow::anyhow!("cannot generate Ed25519 key: {error}"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let private_key = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| anyhow::anyhow!("cannot encode Ed25519 private key: {error}"))?
        .to_string();
    let public_key = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|error| anyhow::anyhow!("cannot encode Ed25519 public key: {error}"))?;
    let now = Utc::now().to_rfc3339();
    Ok(CloudIdentity {
        schema_version: 1,
        device_id: device_id.to_string(),
        public_key_fingerprint: fingerprint(&public_key),
        public_key,
        private_key,
        claim_token: random_token(32)?,
        credential: String::new(),
        created_at: now.clone(),
        updated_at: now,
    })
}

fn fingerprint(public_key: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(public_key.as_bytes()))
}

fn random_token(bytes: usize) -> BridgeResult<String> {
    let mut value = vec![0u8; bytes];
    OsRng
        .try_fill_bytes(&mut value)
        .map_err(|error| anyhow::anyhow!("cannot generate secure random token: {error}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value))
}
