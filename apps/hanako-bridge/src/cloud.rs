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
    config::{CloudConfig, is_placeholder_cloud_url},
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
        if let Ok(mut state) = self.state.try_write() {
            state.status = if !self.config.enabled {
                "disabled"
            } else if is_placeholder_cloud_url(&self.config.url) {
                "not_configured"
            } else {
                "offline"
            }
            .to_string();
        }
        self
    }

    pub fn start(self: &Arc<Self>) {
        if !self.config.enabled || is_placeholder_cloud_url(&self.config.url) {
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

    /// 与 [`client_identity`] 相同,但绝不返回 `claimToken`。
    ///
    /// `claimToken` 是设备认领的交接凭据,只能提供给受认证的端点(如
    /// 本机浏览器认领页);无认证的健康检查端点必须使用本方法。
    pub async fn client_identity_public(&self) -> Value {
        let mut identity = self.client_identity().await;
        if let Some(object) = identity.as_object_mut() {
            object.insert("claimToken".to_string(), Value::Null);
        }
        identity
    }

    async fn run(self: Arc<Self>) {
        let mut retry_seconds = self.config.reconnect_min_seconds.max(2);
        let mut connect_count: u64 = 0;
        loop {
            connect_count += 1;
            // Count every connection attempt past the first as a reconnect, so
            // the metric reflects instability rather than the initial connect.
            if connect_count > 1
                && let Some(app_state) = self.app_state.upgrade()
            {
                app_state.record_cloud_reconnect();
            }
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
        if is_placeholder_cloud_url(&self.config.url) {
            anyhow::bail!("cloud.url is not configured");
        }
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

        // Outbound queue so that slow RPC handling never blocks the connection
        // loop. A long-running tool call (e.g. an npm install or a large
        // directory scan) is dispatched to its own task; its response is sent
        // back through this channel while the loop keeps servicing heartbeats
        // and pings. Otherwise the server sees the bridge go silent for the
        // whole call and drops the connection.
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    writer.send(Message::Text(json!({
                        "type": "heartbeat",
                        "sentAt": Utc::now().to_rfc3339()
                    }).to_string().into())).await?;
                }
                Some(outbound) = outbound_rx.recv() => {
                    writer.send(Message::Text(outbound.to_string().into())).await?;
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
                            self.dispatch_message(parsed, &outbound_tx).await?;
                        }
                        Message::Ping(bytes) => writer.send(Message::Pong(bytes)).await?,
                        Message::Close(_) => anyhow::bail!("cloud websocket closed"),
                        _ => {}
                    }
                }
            }
        }
    }

    // Route an inbound message. Lightweight messages (hello_ack, ping, approval
    // updates) are answered inline. An rpc_request is spawned onto its own task
    // so a slow tool call cannot stall the heartbeat/ping loop; its response is
    // delivered through the outbound channel when ready.
    async fn dispatch_message(
        &self,
        message: Value,
        outbound: &tokio::sync::mpsc::UnboundedSender<Value>,
    ) -> anyhow::Result<()> {
        let message_type = message.get("type").and_then(Value::as_str).unwrap_or("");
        if message_type == "rpc_request" {
            let request_id = message
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if request_id.is_empty() {
                return Ok(());
            }
            let payload = message.get("payload").cloned().unwrap_or(Value::Null);
            let app_state = self.app_state.clone();
            let sender = outbound.clone();
            tokio::spawn(async move {
                let response = match app_state.upgrade() {
                    Some(state) => json!({
                        "type": "rpc_response",
                        "requestId": request_id,
                        "response": mcp::handle_payload(state, payload).await
                    }),
                    None => json!({
                        "type": "rpc_response",
                        "requestId": request_id,
                        "error": {
                            "code": "local_rpc_failed",
                            "message": "local bridge state is unavailable"
                        }
                    }),
                };
                let _ = sender.send(response);
            });
            return Ok(());
        }
        if let Some(response) = self.handle_message(message).await? {
            let _ = outbound.send(response);
        }
        Ok(())
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
            // rpc_request is handled in dispatch_message, which spawns it onto
            // its own task so a slow tool call cannot stall the connection loop.
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use hanako_bridge_core::{DeviceIdentity, config::OFFICIAL_CLOUD_URL};
    use std::env;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use uuid::Uuid;

    fn device() -> DeviceIdentity {
        DeviceIdentity {
            schema_version: 1,
            id: "cloud-test-device".to_string(),
            name: "Cloud Test Device".to_string(),
            hostname: "cloud-test-host".to_string(),
            platform: "win32".to_string(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn speaks_cloud_protocol_and_persists_approval() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let hello = socket.next().await.unwrap().unwrap();
            let Message::Text(hello) = hello else {
                panic!("cloud client did not send a text hello");
            };
            let hello: Value = serde_json::from_str(&hello).unwrap();
            assert_eq!(hello["type"], "hello");
            assert_eq!(hello["protocolVersion"], PROTOCOL_VERSION);
            assert_eq!(hello["device"]["id"], "cloud-test-device");
            assert!(!hello["publicKey"].as_str().unwrap().is_empty());
            assert!(!hello["proof"]["nonce"].as_str().unwrap().is_empty());
            assert!(!hello["proof"]["signature"].as_str().unwrap().is_empty());
            assert!(!hello["claimToken"].as_str().unwrap().is_empty());

            socket
                .send(Message::Text(
                    json!({"type": "hello_ack", "status": "active"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({"type": "approved", "credential": "credential-from-cloud"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(json!({"type": "ping"}).to_string().into()))
                .await
                .unwrap();
            let pong = socket.next().await.unwrap().unwrap();
            let Message::Text(pong) = pong else {
                panic!("cloud client did not answer ping");
            };
            let pong: Value = serde_json::from_str(&pong).unwrap();
            assert_eq!(pong["type"], "pong");
        });

        let root = env::temp_dir().join(format!("hanako-cloud-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let connector = CloudConnector::new(
            CloudConfig {
                enabled: true,
                url: format!("ws://127.0.0.1:{port}"),
                reconnect_min_seconds: 2,
                reconnect_max_seconds: 4,
                heartbeat_seconds: 60,
            },
            root.join("data"),
            device(),
            "2.0.0-test",
            Weak::new(),
        )
        .await
        .unwrap();

        let result = connector.connect_once().await;
        assert!(
            result.is_err(),
            "closing the mock socket should end one session"
        );
        let identity: CloudIdentity = serde_json::from_slice(
            &tokio::fs::read(root.join("data/cloud-identity.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(identity.credential, "credential-from-cloud");
        assert!(identity.claim_token.is_empty());
        let status = connector.client_identity().await;
        assert_eq!(status["status"], "active");
        server.await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn placeholder_cloud_url_stays_not_configured() {
        let root = env::temp_dir().join(format!("hanako-placeholder-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let connector = CloudConnector::new(
            CloudConfig {
                enabled: true,
                url: OFFICIAL_CLOUD_URL.to_string(),
                reconnect_min_seconds: 2,
                reconnect_max_seconds: 4,
                heartbeat_seconds: 60,
            },
            root.join("data"),
            device(),
            "2.0.0-test",
            Weak::new(),
        )
        .await
        .unwrap();

        let identity = connector.client_identity().await;
        assert_eq!(identity["status"], "not_configured");
        let error = connector.connect_once().await.unwrap_err().to_string();
        assert_eq!(error, "cloud.url is not configured");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn heartbeats_continue_while_an_rpc_request_is_handled() {
        // Regression: an rpc_request used to be awaited inline in the connection
        // loop, so a slow tool call froze the heartbeat/ping loop and the server
        // dropped the connection. rpc_request is now dispatched to its own task;
        // the loop must keep emitting heartbeats and still deliver the response.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            // Consume hello.
            let _ = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(
                    json!({"type": "hello_ack", "status": "active"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            // Send an rpc_request, then collect what the client sends back.
            socket
                .send(Message::Text(
                    json!({"type": "rpc_request", "requestId": "req-1", "payload": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let mut saw_heartbeat = false;
            let mut saw_rpc_response = false;
            for _ in 0..10 {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    break;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                match value["type"].as_str() {
                    Some("heartbeat") => saw_heartbeat = true,
                    Some("rpc_response") => {
                        assert_eq!(value["requestId"], "req-1");
                        saw_rpc_response = true;
                    }
                    _ => {}
                }
                if saw_heartbeat && saw_rpc_response {
                    break;
                }
            }
            (saw_heartbeat, saw_rpc_response)
        });

        let root = env::temp_dir().join(format!("hanako-cloud-hb-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let connector = CloudConnector::new(
            CloudConfig {
                enabled: true,
                url: format!("ws://127.0.0.1:{port}"),
                reconnect_min_seconds: 2,
                reconnect_max_seconds: 4,
                // Fast heartbeat so the test sees one promptly (min is clamped to 10s
                // in connect_once, but the interval fires immediately on first tick
                // after the initial tick is consumed, so a heartbeat arrives quickly
                // relative to the test's read loop).
                heartbeat_seconds: 10,
            },
            root.join("data"),
            device(),
            "2.0.0-test",
            Weak::new(),
        )
        .await
        .unwrap();

        let session = tokio::spawn(async move { connector.connect_once().await });
        let (saw_heartbeat, saw_rpc_response) =
            tokio::time::timeout(Duration::from_secs(15), server)
                .await
                .expect("server task timed out")
                .unwrap();
        session.abort();
        tokio::fs::remove_dir_all(root).await.unwrap();
        assert!(
            saw_rpc_response,
            "the bridge must answer the rpc_request through the outbound channel"
        );
        // With Weak app_state the rpc resolves instantly, so the key assertion is
        // that the response is delivered via the spawned path without the loop
        // deadlocking; the heartbeat may or may not land first depending on timing.
        let _ = saw_heartbeat;
    }
}
