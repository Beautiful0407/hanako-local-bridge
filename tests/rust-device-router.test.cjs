const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const projectDir = path.resolve(__dirname, "..");
const bridgeExe = path.join(projectDir, "target", "debug", "hanako-bridge.exe");
const routerExe = path.join(projectDir, "target", "debug", "hanako-device-router.exe");

async function waitForHealth(url, child, output) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`process exited early with code ${child.exitCode}\n${output.stderr}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return response.json();
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${url}\n${output.stderr}`);
}

function startProcess(file, args, env) {
  const output = { stdout: "", stderr: "" };
  const child = spawn(file, args, {
    env,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdout.on("data", (chunk) => {
    output.stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    output.stderr += chunk;
  });
  return { child, output };
}

async function stopProcess(instance) {
  if (!instance || instance.child.exitCode !== null) return;
  instance.child.kill();
  await Promise.race([
    new Promise((resolve) => instance.child.once("exit", resolve)),
    new Promise((_, reject) => setTimeout(() => reject(new Error("process did not stop")), 10000)),
  ]);
}

async function rpc(base, id, method, params = {}) {
  const response = await fetch(`${base}/mcp`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
  });
  assert.equal(response.status, 200);
  return response.json();
}

function textResult(response) {
  assert.ok(response.result, JSON.stringify(response));
  return JSON.parse(response.result.content[0].text);
}

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hanako-rust-router-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const bridgeConfig = path.join(temp, "bridge-config.json");
  const routerConfig = path.join(temp, "devices.json");
  const cache = path.join(temp, "tools-cache.json");
  const queue = path.join(temp, "offline-queue.json");
  const devicePort = 36000 + Math.floor(Math.random() * 500);
  const routerPort = devicePort + 500;
  await fsp.mkdir(root, { recursive: true });
  await fsp.mkdir(data, { recursive: true });
  await fsp.mkdir(logs, { recursive: true });
  await fsp.writeFile(
    bridgeConfig,
    `${JSON.stringify({
      schemaVersion: 1,
      device: { id: "router-test", name: "Router Test Device" },
      filesystem: {
        host: "127.0.0.1",
        port: devicePort,
        approvalPort: devicePort + 1,
        trustMode: "full",
        allowChatAuthorization: false,
        chatGrantMinutes: 120,
        roots: [{ name: "RouterRoot", path: root, mode: "read_write" }],
      },
      storage: { dataDir: data, logDir: logs },
      cloud: {
        enabled: false,
        url: "wss://your-server.example.com/local-bridge/connect",
        reconnectMinSeconds: 3,
        reconnectMaxSeconds: 60,
        heartbeatSeconds: 25,
      },
      tunnel: {
        enabled: false,
        server: "127.0.0.1",
        user: "root",
        localHost: "127.0.0.1",
        localPort: devicePort,
        remoteHost: "127.0.0.1",
        remotePort: devicePort,
        identityFile: "",
      },
      service: {
        taskPrefix: "Rust Router Test",
        restartDelaySeconds: 1,
        tunnelRetryMinSeconds: 1,
        tunnelRetryMaxSeconds: 2,
        tunnelHealthSeconds: 2,
      },
      update: { manifest: "", channel: "alpha" },
    }, null, 2)}\n`,
    "utf8",
  );
  await fsp.writeFile(
    routerConfig,
    `${JSON.stringify({
      schemaVersion: 1,
      defaultDeviceId: "router-test",
      devices: [{
        id: "router-test",
        name: "Router Test Device",
        url: `http://127.0.0.1:${devicePort}/mcp`,
        healthUrl: `http://127.0.0.1:${devicePort}/health`,
        mcpToken: "",
        enabled: true,
      }],
    }, null, 2)}\n`,
    "utf8",
  );

  let device;
  let router;
  try {
    device = startProcess(bridgeExe, ["--service"], {
      ...process.env,
      HANA_LOCAL_BRIDGE_CONFIG: bridgeConfig,
      LOCAL_AGENT_TRUST_MODE: "full",
      LOCAL_AGENT_DEVICE_ID: "router-test",
      LOCAL_AGENT_DEVICE_NAME: "Router Test Device",
    });
    await waitForHealth(`http://127.0.0.1:${devicePort}/health`, device.child, device.output);
    const mcpToken = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();
    const configuredRouter = JSON.parse(await fsp.readFile(routerConfig, "utf8"));
    configuredRouter.devices[0].mcpToken = mcpToken;
    await fsp.writeFile(routerConfig, `${JSON.stringify(configuredRouter, null, 2)}\n`, "utf8");

    router = startProcess(routerExe, [], {
      ...process.env,
      HANA_DEVICE_ROUTER_CONFIG: routerConfig,
      HANA_DEVICE_ROUTER_CACHE: cache,
      HANA_DEVICE_ROUTER_QUEUE: queue,
      HANA_DEVICE_ROUTER_HOST: "127.0.0.1",
      HANA_DEVICE_ROUTER_PORT: String(routerPort),
      HANA_DEVICE_HEALTH_INTERVAL_MS: "2000",
    });
    const routerBase = `http://127.0.0.1:${routerPort}`;
    const health = await waitForHealth(`${routerBase}/health`, router.child, router.output);
    assert.equal(health.ok, true);
    assert.equal(health.devices[0].online, true);

    const listed = await rpc(routerBase, 1, "tools/list");
    assert.equal(listed.result.tools.length, 36);
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_fs.read_image"));

    const deviceRoot = `device://router-test/${root.replace(/\\/g, "/")}`;
    const write = await rpc(routerBase, 2, "tools/call", {
      name: "local_fs.write_text",
      arguments: { path: `${deviceRoot}/routed.txt`, text: "through rust router" },
    });
    assert.ok(write.result, JSON.stringify(write));
    assert.equal(
      await fsp.readFile(path.join(root, "routed.txt"), "utf8"),
      "through rust router",
    );

    await stopProcess(device);
    device = null;
    const offline = textResult(await rpc(routerBase, 3, "tools/call", {
      name: "local_device.devices",
      arguments: { refresh: true },
    }));
    assert.equal(offline.devices[0].online, false);
    const queued = textResult(await rpc(routerBase, 4, "tools/call", {
      name: "local_fs.write_text",
      arguments: {
        path: `${deviceRoot}/queued.txt`,
        text: "after reconnect",
        queueIfOffline: true,
      },
    }));
    assert.equal(queued.status, "queued");

    device = startProcess(bridgeExe, ["--service"], {
      ...process.env,
      HANA_LOCAL_BRIDGE_CONFIG: bridgeConfig,
      LOCAL_AGENT_TRUST_MODE: "full",
      LOCAL_AGENT_DEVICE_ID: "router-test",
      LOCAL_AGENT_DEVICE_NAME: "Router Test Device",
    });
    await waitForHealth(`http://127.0.0.1:${devicePort}/health`, device.child, device.output);
    const deadline = Date.now() + 20000;
    let queueItem;
    while (Date.now() < deadline) {
      const queueResult = textResult(await rpc(routerBase, 5, "tools/call", {
        name: "local_device.queue",
        arguments: { queueId: queued.queue.id },
      }));
      queueItem = queueResult.items[0];
      if (queueItem?.status === "completed") break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    assert.equal(queueItem?.status, "completed");
    assert.equal(await fsp.readFile(path.join(root, "queued.txt"), "utf8"), "after reconnect");
  } finally {
    await stopProcess(router).catch(() => {});
    await stopProcess(device).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }
  console.log("Rust device router integration tests passed");
}

run().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
