const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");

const { CloudConnector, loadIdentity } = require("../lib/cloud-connector.cjs");

class FakeWebSocket {
  static OPEN = 1;
  static instances = [];

  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.listeners = new Map();
    this.sent = [];
    FakeWebSocket.instances.push(this);
  }

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) || [];
    listeners.push(listener);
    this.listeners.set(name, listeners);
  }

  emit(name, data = {}) {
    if (name === "open") this.readyState = FakeWebSocket.OPEN;
    if (name === "close") this.readyState = 3;
    for (const listener of this.listeners.get(name) || []) listener(data);
  }

  send(data) {
    this.sent.push(JSON.parse(data));
  }

  close() {
    this.emit("close");
  }
}

async function tick() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-cloud-connector-test-"));
  try {
    const rpcCalls = [];
    const connector = new CloudConnector({
      config: {
        enabled: true,
        url: "ws://hana.test/local-bridge/connect",
        reconnectMinSeconds: 60,
        reconnectMaxSeconds: 60,
        heartbeatSeconds: 60,
      },
      dataDir: temp,
      device: { id: "test-pc", name: "Test PC", hostname: "TEST-PC" },
      version: "1.2.0",
      capabilities: { read: true, write: true },
      handleRpc: async (payload) => {
        rpcCalls.push(payload);
        return { jsonrpc: "2.0", id: payload.id, result: { ok: true } };
      },
      WebSocketImpl: FakeWebSocket,
    });

    connector.start();
    const socket = FakeWebSocket.instances.at(-1);
    assert.equal(socket.url, "ws://hana.test/local-bridge/connect");
    socket.emit("open");
    const hello = socket.sent[0];
    assert.equal(hello.type, "hello");
    assert.equal(hello.device.id, "test-pc");
    assert.ok(hello.claimToken.length >= 32);
    assert.match(hello.publicKey, /BEGIN PUBLIC KEY/);
    assert.ok(hello.proof.signature);

    socket.emit("message", {
      data: JSON.stringify({ type: "hello_ack", status: "pending" }),
    });
    assert.equal(connector.clientIdentity().status, "pending_claim");
    assert.equal(connector.clientIdentity().claimToken, hello.claimToken);

    socket.emit("message", {
      data: JSON.stringify({
        type: "rpc_request",
        requestId: "req-1",
        payload: { jsonrpc: "2.0", id: 7, method: "tools/list", params: {} },
      }),
    });
    await tick();
    assert.equal(rpcCalls.length, 1);
    const rpcResponse = socket.sent.find((item) => item.type === "rpc_response");
    assert.equal(rpcResponse.requestId, "req-1");
    assert.equal(rpcResponse.response.result.ok, true);

    socket.emit("message", {
      data: JSON.stringify({ type: "approved", credential: "hana_dev_test_credential" }),
    });
    await tick();
    assert.equal(connector.clientIdentity().status, "active");
    assert.equal(connector.clientIdentity().claimToken, null);
    connector.stop();

    const persisted = loadIdentity(temp, "test-pc").identity;
    assert.equal(persisted.credential, "hana_dev_test_credential");
    assert.equal(persisted.claimToken, "");

    const reloaded = new CloudConnector({
      config: {
        enabled: true,
        url: "wss://hana.test/local-bridge/connect",
        reconnectMinSeconds: 60,
        reconnectMaxSeconds: 60,
        heartbeatSeconds: 60,
      },
      dataDir: temp,
      device: { id: "test-pc", name: "Test PC", hostname: "TEST-PC" },
      version: "1.2.0",
      capabilities: {},
      handleRpc: async () => null,
      WebSocketImpl: FakeWebSocket,
    });
    reloaded.start();
    const reloadedSocket = FakeWebSocket.instances.at(-1);
    reloadedSocket.emit("open");
    const reloadedHello = reloadedSocket.sent[0];
    assert.equal(reloadedHello.credential, "hana_dev_test_credential");
    assert.equal(reloadedHello.claimToken, "");
    reloaded.stop();
  } finally {
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("cloud connector tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
