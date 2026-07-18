const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const projectDir = path.resolve(__dirname, "..");
const serverFile = path.join(projectDir, "server.cjs");
const routerFile = path.join(projectDir, "cloud", "device-router.cjs");

async function waitForHealth(url, child, output) {
  const deadline = Date.now() + 15000;
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

function startProcess(file, env) {
  const output = { stdout: "", stderr: "" };
  const child = spawn(process.execPath, [file], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
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
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-device-router-test-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  await fsp.mkdir(root, { recursive: true });
  await fsp.mkdir(data, { recursive: true });
  const mcpToken = "router-test-token";
  await fsp.writeFile(path.join(data, "approval-token.txt"), `${mcpToken}\n`, "utf8");

  const devicePort = 35000 + Math.floor(Math.random() * 500);
  const routerPort = devicePort + 500;
  const configFile = path.join(temp, "devices.json");
  const cacheFile = path.join(temp, "tools-cache.json");
  const queueFile = path.join(temp, "offline-queue.json");
  await fsp.writeFile(
    configFile,
    `${JSON.stringify(
      {
        schemaVersion: 1,
        defaultDeviceId: "router-test",
        devices: [
          {
            id: "router-test",
            name: "Router Test Device",
            url: `http://127.0.0.1:${devicePort}/mcp`,
            healthUrl: `http://127.0.0.1:${devicePort}/health`,
            mcpToken,
            enabled: true,
          },
        ],
      },
      null,
      2,
    )}\n`,
    "utf8",
  );

  let device;
  let router;
  const deviceEnv = {
    ...process.env,
    LOCAL_AGENT_TRUST_MODE: "full",
    LOCAL_AGENT_DEVICE_ID: "router-test",
    LOCAL_AGENT_DEVICE_NAME: "Router Test Device",
    LOCAL_FS_MCP_ROOT: root,
    LOCAL_FS_MCP_ROOT_NAME: "RouterRoot",
    LOCAL_FS_MCP_HOST: "127.0.0.1",
    LOCAL_FS_MCP_PORT: String(devicePort),
    LOCAL_FS_MCP_APPROVAL_PORT: String(devicePort + 1),
    LOCAL_FS_MCP_DATA_DIR: data,
    LOCAL_FS_MCP_LOG_DIR: logs,
    LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION: "0",
  };
  try {
    device = startProcess(serverFile, deviceEnv);
    await waitForHealth(`http://127.0.0.1:${devicePort}/health`, device.child, device.output);

    router = startProcess(routerFile, {
      ...process.env,
      HANA_DEVICE_ROUTER_CONFIG: configFile,
      HANA_DEVICE_ROUTER_CACHE: cacheFile,
      HANA_DEVICE_ROUTER_QUEUE: queueFile,
      HANA_DEVICE_ROUTER_HOST: "127.0.0.1",
      HANA_DEVICE_ROUTER_PORT: String(routerPort),
      HANA_DEVICE_HEALTH_INTERVAL_MS: "2000",
    });
    const routerBase = `http://127.0.0.1:${routerPort}`;
    const health = await waitForHealth(`${routerBase}/health`, router.child, router.output);
    assert.equal(health.ok, true);
    assert.equal(health.devices[0].id, "router-test");
    assert.equal(health.devices[0].online, true);
    assert.equal(health.devices[0].mcpToken, undefined);

    const registration = await fetch(`${routerBase}/devices/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        id: "second-device",
        name: "Second Device",
        remotePort: devicePort,
        mcpToken: "second-device-token",
      }),
    }).then((response) => response.json());
    assert.equal(registration.deviceId, "second-device");
    assert.notEqual(registration.remotePort, devicePort);
    const registeredConfig = JSON.parse(await fsp.readFile(configFile, "utf8"));
    assert.ok(registeredConfig.devices.some((item) => (
      item.id === "second-device"
      && item.url === `http://127.0.0.1:${registration.remotePort}/mcp`
      && item.mcpToken === "second-device-token"
    )));
    const websocketRegistration = await fetch(`${routerBase}/devices/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        id: "wss-device",
        name: "WSS Device",
        url: "http://127.0.0.1:14500/internal/local-bridge/devices/wss-device/mcp",
        healthUrl: "http://127.0.0.1:14500/internal/local-bridge/devices/wss-device/health",
      }),
    }).then((response) => response.json());
    assert.equal(websocketRegistration.deviceId, "wss-device");
    const configWithWss = JSON.parse(await fsp.readFile(configFile, "utf8"));
    assert.ok(configWithWss.devices.some((item) => (
      item.id === "wss-device"
      && item.url === "http://127.0.0.1:14500/internal/local-bridge/devices/wss-device/mcp"
    )));
    const missingSelection = await rpc(routerBase, 0, "tools/call", {
      name: "local_fs.read_text",
      arguments: {
        path: path.join(root, "routed.txt"),
      },
    });
    assert.equal(missingSelection.error.data.code, "device_required");

    const listed = await rpc(routerBase, 1, "tools/list");
    assert.equal(listed.result.tools.length, 34);
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_device.devices"));
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_device.queue"));
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_device.cancel_queued"));
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_fs.read_lines"));
    assert.ok(listed.result.tools.some((tool) => tool.name === "local_fs.read_image"));

    const devices = textResult(
      await rpc(routerBase, 2, "tools/call", {
        name: "local_device.devices",
        arguments: { refresh: true },
      }),
    );
    assert.equal(devices.devices[0].online, true);
    assert.equal(devices.devices[0].pathPrefix, "device://router-test/");

    const deviceRoot = `device://router-test/${root.replace(/\\/g, "/")}`;
    const routedWrite = await rpc(routerBase, 3, "tools/call", {
      name: "local_fs.write_text",
      arguments: {
        path: `${deviceRoot}/routed.txt`,
        text: "through router",
      },
    });
    assert.ok(routedWrite.result, JSON.stringify(routedWrite));
    assert.equal(await fsp.readFile(path.join(root, "routed.txt"), "utf8"), "through router");

    const absoluteWrite = await rpc(routerBase, 4, "tools/call", {
      name: "local_fs.write_text",
      arguments: {
        deviceId: "router-test",
        path: path.join(root, "selected.txt"),
        text: "selected device",
      },
    });
    assert.ok(absoluteWrite.result, JSON.stringify(absoluteWrite));
    assert.equal(await fsp.readFile(path.join(root, "selected.txt"), "utf8"), "selected device");

    const missingDevice = await rpc(routerBase, 5, "tools/call", {
      name: "local_fs.read_text",
      arguments: {
        path: `device://missing-device/${root.replace(/\\/g, "/")}/routed.txt`,
      },
    });
    assert.equal(missingDevice.error.data.code, "device_not_found");

    await stopProcess(device);
    device = null;
    const offlineDevices = textResult(
      await rpc(routerBase, 6, "tools/call", {
        name: "local_device.devices",
        arguments: { refresh: true },
      }),
    );
    assert.equal(offlineDevices.devices[0].online, false);
    const queued = textResult(
      await rpc(routerBase, 7, "tools/call", {
        name: "local_fs.write_text",
        arguments: {
          path: `${deviceRoot}/queued.txt`,
          text: "written after reconnect",
          queueIfOffline: true,
        },
      }),
    );
    assert.equal(queued.status, "queued");
    assert.equal(queued.queue.status, "queued");

    device = startProcess(serverFile, deviceEnv);
    await waitForHealth(`http://127.0.0.1:${devicePort}/health`, device.child, device.output);
    let queuedStatus = null;
    const queueDeadline = Date.now() + 20000;
    while (Date.now() < queueDeadline) {
      await rpc(routerBase, 8, "tools/call", {
        name: "local_device.devices",
        arguments: { refresh: true },
      });
      const queue = textResult(
        await rpc(routerBase, 9, "tools/call", {
          name: "local_device.queue",
          arguments: { queueId: queued.queue.id },
        }),
      );
      queuedStatus = queue.items[0];
      if (queuedStatus?.status === "completed") break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    assert.equal(queuedStatus?.status, "completed");
    assert.equal(await fsp.readFile(path.join(root, "queued.txt"), "utf8"), "written after reconnect");
  } finally {
    await stopProcess(router).catch(() => {});
    await stopProcess(device).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("device router tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
