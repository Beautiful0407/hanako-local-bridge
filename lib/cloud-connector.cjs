const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const STATE_FILE = "cloud-identity.json";
const PROTOCOL_VERSION = 1;

function nowIso() {
  return new Date().toISOString();
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("base64url");
}

function randomToken() {
  return crypto.randomBytes(32).toString("base64url");
}

function atomicWriteJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const temp = `${filePath}.${process.pid}.tmp`;
  fs.writeFileSync(temp, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  fs.renameSync(temp, filePath);
  try {
    fs.chmodSync(filePath, 0o600);
  } catch {}
}

function createIdentity(deviceId) {
  const pair = crypto.generateKeyPairSync("ed25519");
  const publicKey = pair.publicKey.export({ type: "spki", format: "pem" }).toString();
  const privateKey = pair.privateKey.export({ type: "pkcs8", format: "pem" }).toString();
  const createdAt = nowIso();
  return {
    schemaVersion: 1,
    deviceId,
    publicKey,
    privateKey,
    publicKeyFingerprint: sha256(publicKey),
    claimToken: randomToken(),
    credential: "",
    createdAt,
    updatedAt: createdAt,
  };
}

function normalizeIdentity(value, deviceId) {
  if (!value || typeof value !== "object" || value.schemaVersion !== 1) {
    return createIdentity(deviceId);
  }
  if (!value.publicKey || !value.privateKey) return createIdentity(deviceId);
  return {
    schemaVersion: 1,
    deviceId,
    publicKey: String(value.publicKey),
    privateKey: String(value.privateKey),
    publicKeyFingerprint: String(value.publicKeyFingerprint || sha256(String(value.publicKey))),
    claimToken: String(value.claimToken || (value.credential ? "" : randomToken())),
    credential: String(value.credential || ""),
    createdAt: String(value.createdAt || nowIso()),
    updatedAt: String(value.updatedAt || nowIso()),
  };
}

function loadIdentity(dataDir, deviceId) {
  const filePath = path.join(dataDir, STATE_FILE);
  let parsed = null;
  try {
    parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (err) {
    if (err?.code !== "ENOENT") {
      const backup = `${filePath}.corrupt-${Date.now()}`;
      try {
        fs.renameSync(filePath, backup);
      } catch {}
    }
  }
  const identity = normalizeIdentity(parsed, deviceId);
  atomicWriteJson(filePath, identity);
  return { filePath, identity };
}

function parseMessage(data) {
  try {
    if (typeof data === "string") return JSON.parse(data);
    if (data instanceof ArrayBuffer) return JSON.parse(Buffer.from(data).toString("utf8"));
    if (ArrayBuffer.isView(data)) {
      return JSON.parse(Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("utf8"));
    }
    return JSON.parse(String(data));
  } catch {
    return null;
  }
}

class CloudConnector {
  constructor({
    config,
    dataDir,
    device,
    version,
    handleRpc,
    capabilities,
    WebSocketImpl = globalThis.WebSocket,
    log = () => {},
  }) {
    this.config = config || {};
    this.dataDir = dataDir;
    this.device = device;
    this.version = version;
    this.handleRpc = handleRpc;
    this.capabilities = capabilities || {};
    this.WebSocketImpl = WebSocketImpl;
    this.log = log;
    this.state = loadIdentity(dataDir, device.id);
    this.socket = null;
    this.stopped = true;
    this.reconnectTimer = null;
    this.heartbeatTimer = null;
    this.retrySeconds = Math.max(2, Number(this.config.reconnectMinSeconds) || 3);
    this.status = this.config.enabled === false ? "disabled" : "offline";
    this.lastConnectedAt = null;
    this.lastSeenAt = null;
    this.lastError = "";
  }

  start() {
    if (this.config.enabled === false || this.stopped === false) return;
    if (typeof this.WebSocketImpl !== "function") {
      this.status = "error";
      this.lastError = "WebSocket runtime unavailable";
      this.log(this.lastError);
      return;
    }
    this.stopped = false;
    this.connect();
  }

  stop() {
    this.stopped = true;
    clearTimeout(this.reconnectTimer);
    clearInterval(this.heartbeatTimer);
    this.reconnectTimer = null;
    this.heartbeatTimer = null;
    try {
      this.socket?.close();
    } catch {}
    this.socket = null;
    this.status = this.config.enabled === false ? "disabled" : "offline";
  }

  connect() {
    if (this.stopped || this.config.enabled === false) return;
    const url = String(this.config.url || "").trim();
    if (!/^wss?:\/\//i.test(url)) {
      this.status = "error";
      this.lastError = "cloud.url must use ws:// or wss://";
      this.scheduleReconnect();
      return;
    }

    this.status = "connecting";
    this.lastError = "";
    let socket;
    try {
      socket = new this.WebSocketImpl(url);
    } catch (err) {
      this.lastError = err.message || String(err);
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;

    socket.addEventListener("open", () => {
      if (this.socket !== socket) return;
      this.status = "authenticating";
      this.lastConnectedAt = nowIso();
      this.sendHello();
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = setInterval(() => {
        this.send({ type: "heartbeat", sentAt: nowIso() });
      }, Math.max(10, Number(this.config.heartbeatSeconds) || 25) * 1000);
      this.heartbeatTimer.unref?.();
    });

    socket.addEventListener("message", (event) => {
      const message = parseMessage(event.data);
      if (!message) return;
      this.lastSeenAt = nowIso();
      this.handleMessage(message).catch((err) => {
        this.lastError = err.message || String(err);
        this.log(`cloud message failed: ${this.lastError}`);
      });
    });

    socket.addEventListener("error", () => {
      if (this.socket !== socket) return;
      this.lastError = "cloud websocket error";
    });

    socket.addEventListener("close", () => {
      if (this.socket !== socket) return;
      this.socket = null;
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
      if (!this.stopped) this.scheduleReconnect();
    });
  }

  sendHello() {
    const identity = this.state.identity;
    const nonce = crypto.randomBytes(24).toString("base64url");
    const signature = crypto.sign(
      null,
      Buffer.from(nonce, "utf8"),
      crypto.createPrivateKey(identity.privateKey),
    ).toString("base64url");
    this.send({
      type: "hello",
      protocolVersion: PROTOCOL_VERSION,
      device: {
        id: this.device.id,
        name: this.device.name,
        hostname: this.device.hostname,
        platform: process.platform,
        version: this.version,
      },
      capabilities: this.capabilities,
      publicKey: identity.publicKey,
      publicKeyFingerprint: identity.publicKeyFingerprint,
      proof: { nonce, signature },
      claimToken: identity.credential ? "" : identity.claimToken,
      credential: identity.credential,
    });
  }

  async handleMessage(message) {
    if (message.type === "hello_ack") {
      this.status = message.status === "active" ? "active" : "pending_claim";
      this.retrySeconds = Math.max(2, Number(this.config.reconnectMinSeconds) || 3);
      return;
    }
    if (message.type === "approved" && message.credential) {
      this.state.identity.credential = String(message.credential);
      this.state.identity.claimToken = "";
      this.state.identity.updatedAt = nowIso();
      atomicWriteJson(this.state.filePath, this.state.identity);
      this.status = "active";
      return;
    }
    if (message.type === "revoked") {
      this.state.identity.credential = "";
      this.state.identity.claimToken = randomToken();
      this.state.identity.updatedAt = nowIso();
      atomicWriteJson(this.state.filePath, this.state.identity);
      this.status = "pending_claim";
      this.sendHello();
      return;
    }
    if (message.type === "ping") {
      this.send({ type: "pong", sentAt: nowIso() });
      return;
    }
    if (message.type === "rpc_request") {
      const requestId = String(message.requestId || "");
      if (!requestId) return;
      try {
        const response = await this.handleRpc(message.payload);
        this.send({ type: "rpc_response", requestId, response });
      } catch (err) {
        this.send({
          type: "rpc_response",
          requestId,
          error: {
            code: err.code || "local_rpc_failed",
            message: err.message || String(err),
          },
        });
      }
    }
  }

  send(message) {
    const socket = this.socket;
    if (!socket || socket.readyState !== this.WebSocketImpl.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  scheduleReconnect() {
    if (this.stopped) return;
    this.status = "offline";
    clearTimeout(this.reconnectTimer);
    const delay = this.retrySeconds;
    this.retrySeconds = Math.min(
      Math.max(delay, Number(this.config.reconnectMaxSeconds) || 60),
      Math.max(delay * 2, Number(this.config.reconnectMinSeconds) || 3),
    );
    this.reconnectTimer = setTimeout(() => this.connect(), delay * 1000);
    this.reconnectTimer.unref?.();
  }

  clientIdentity() {
    const identity = this.state.identity;
    return {
      status: this.status,
      claimToken: identity.credential ? null : identity.claimToken,
      publicKeyFingerprint: identity.publicKeyFingerprint,
      cloudUrl: String(this.config.url || ""),
      lastConnectedAt: this.lastConnectedAt,
      lastSeenAt: this.lastSeenAt,
      lastError: this.lastError || null,
    };
  }
}

module.exports = {
  CloudConnector,
  createIdentity,
  loadIdentity,
  parseMessage,
  sha256,
};
