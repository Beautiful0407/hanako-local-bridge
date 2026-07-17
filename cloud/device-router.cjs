const fs = require("fs");
const fsp = require("fs/promises");
const http = require("http");
const path = require("path");
const crypto = require("crypto");

const CONFIG_FILE = path.resolve(
  process.env.HANA_DEVICE_ROUTER_CONFIG || path.join(__dirname, "devices.json"),
);
const CACHE_FILE = path.resolve(
  process.env.HANA_DEVICE_ROUTER_CACHE || path.join(path.dirname(CONFIG_FILE), "tools-cache.json"),
);
const QUEUE_FILE = path.resolve(
  process.env.HANA_DEVICE_ROUTER_QUEUE || path.join(path.dirname(CONFIG_FILE), "offline-queue.json"),
);
const HOST = process.env.HANA_DEVICE_ROUTER_HOST || "127.0.0.1";
const PORT = Number(process.env.HANA_DEVICE_ROUTER_PORT || 18786);
const HEALTH_INTERVAL_MS = Math.max(2000, Number(process.env.HANA_DEVICE_HEALTH_INTERVAL_MS) || 10000);

function loadJson(file, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (err) {
    if (err?.code === "ENOENT") return fallback;
    throw err;
  }
}

function loadConfig() {
  const config = loadJson(CONFIG_FILE, null);
  if (!config || !Array.isArray(config.devices)) {
    throw new Error(`device router config is invalid: ${CONFIG_FILE}`);
  }
  return {
    schemaVersion: 1,
    defaultDeviceId: String(config.defaultDeviceId || "").toLowerCase(),
    devices: config.devices
      .filter((device) => device && device.id && device.url && device.enabled !== false)
      .map((device) => ({
        id: String(device.id).toLowerCase(),
        name: String(device.name || device.id),
        url: String(device.url),
        healthUrl: String(device.healthUrl || String(device.url).replace(/\/mcp\/?$/i, "/health")),
        enabled: true,
      })),
  };
}

function writeConfig(config) {
  const temp = `${CONFIG_FILE}.${process.pid}.tmp`;
  fs.mkdirSync(path.dirname(CONFIG_FILE), { recursive: true });
  fs.writeFileSync(temp, `${JSON.stringify(config, null, 2)}\n`, "utf8");
  fs.renameSync(temp, CONFIG_FILE);
}

function cleanDeviceId(value) {
  return String(value || "").trim().toLowerCase().replace(/[^a-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
}

function portFromDevice(device) {
  try {
    return Number(new URL(device.url).port);
  } catch {
    return 0;
  }
}

function normalizeLoopbackUrl(value, label) {
  if (!value) return "";
  let parsed;
  try {
    parsed = new URL(String(value));
  } catch {
    throw Object.assign(new Error(`${label} must be a valid URL`), { code: "device_url_invalid" });
  }
  const host = parsed.hostname.toLowerCase();
  if (parsed.protocol !== "http:" || !["127.0.0.1", "localhost", "::1", "[::1]"].includes(host)) {
    throw Object.assign(new Error(`${label} must use a loopback HTTP URL`), { code: "device_url_invalid" });
  }
  return parsed.toString();
}

function allocateRemotePort(config, deviceId, requestedPort) {
  const used = new Set(
    (config.devices || [])
      .filter((device) => cleanDeviceId(device.id) !== deviceId)
      .map(portFromDevice)
      .filter((port) => Number.isInteger(port) && port > 0),
  );
  const requested = Number(requestedPort);
  if (Number.isInteger(requested) && requested >= 1024 && requested <= 65535 && !used.has(requested)) {
    return requested;
  }
  const min = Math.max(1024, Number(process.env.HANA_DEVICE_REMOTE_PORT_MIN) || 18787);
  const max = Math.min(65535, Number(process.env.HANA_DEVICE_REMOTE_PORT_MAX) || 19999);
  for (let port = min; port <= max; port += 1) {
    if (!used.has(port)) return port;
  }
  throw Object.assign(new Error("no remote tunnel ports are available"), {
    code: "remote_port_unavailable",
  });
}

function writeJson(res, status, data, headers = {}) {
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-store",
    ...headers,
  });
  res.end(data === null ? "" : JSON.stringify(data));
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
      if (body.length > 16 * 1024 * 1024) {
        reject(Object.assign(new Error("request body is too large"), { code: "request_too_large" }));
        req.destroy();
      }
    });
    req.on("end", () => resolve(body));
    req.on("error", reject);
  });
}

function jsonRpc(id, result) {
  return { jsonrpc: "2.0", id, result };
}

function jsonRpcError(id, err) {
  return {
    jsonrpc: "2.0",
    id,
    error: {
      code: -32000,
      message: err.message || String(err),
      data: {
        code: err.code || "device_router_error",
        deviceId: err.deviceId || null,
        requestedDevices: err.requestedDevices || null,
      },
    },
  };
}

function contentJson(value) {
  return { content: [{ type: "text", text: JSON.stringify(value, null, 2) }] };
}

function extractDeviceIds(value, target) {
  if (typeof value === "string") {
    const match = /^device:\/\/([^/]+)\//i.exec(value.trim());
    if (match) target.add(decodeURIComponent(match[1]).toLowerCase());
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) extractDeviceIds(item, target);
    return;
  }
  if (value && typeof value === "object") {
    for (const item of Object.values(value)) extractDeviceIds(item, target);
  }
}

function adaptTool(tool) {
  const adapted = structuredClone(tool);
  adapted.description = String(adapted.description || "")
    .replace(/device:\/\/[a-z0-9._-]+\//gi, "device://<deviceId>/")
    .concat(" Route to a specific computer with deviceId or a device://<deviceId>/C:/... path.");
  adapted.inputSchema ||= { type: "object", properties: {} };
  adapted.inputSchema.properties ||= {};
  adapted.inputSchema.properties.deviceId = {
    type: "string",
    description: "Target device ID. Optional when a device:// path is present or only one device is configured.",
  };
  adapted.inputSchema.properties.queueIfOffline = {
    type: "boolean",
    description: "When true, persist this call and run it automatically after the target device reconnects.",
  };
  return adapted;
}

const deviceTools = [
  {
    name: "local_device.devices",
    title: "List local Windows devices",
    description: "List configured Windows bridge devices, online state, version, latency, and device:// path prefix.",
    inputSchema: {
      type: "object",
      properties: {
        refresh: { type: "boolean" },
      },
    },
  },
  {
    name: "local_device.queue",
    title: "List offline device queue",
    description: "List queued, running, completed, failed, or cancelled calls for local Windows devices.",
    inputSchema: {
      type: "object",
      properties: {
        queueId: { type: "string" },
        deviceId: { type: "string" },
        status: { type: "string" },
        limit: { type: "number" },
      },
    },
  },
  {
    name: "local_device.cancel_queued",
    title: "Cancel an offline queued call",
    description: "Cancel a queued call before it starts running on the target Windows device.",
    inputSchema: {
      type: "object",
      properties: { queueId: { type: "string" } },
      required: ["queueId"],
    },
  },
];

class DeviceRouter {
  constructor() {
    this.config = loadConfig();
    this.status = new Map();
    this.toolCache = loadJson(CACHE_FILE, { schemaVersion: 1, tools: [] });
    this.queue = loadJson(QUEUE_FILE, { schemaVersion: 1, items: [] });
    if (!Array.isArray(this.queue.items)) this.queue.items = [];
    this.queueProcessing = false;
  }

  reloadConfig() {
    this.config = loadConfig();
  }

  getDevice(id) {
    return this.config.devices.find((device) => device.id === String(id || "").toLowerCase()) || null;
  }

  registerDevice(input = {}) {
    const id = cleanDeviceId(input.id);
    if (!id) {
      throw Object.assign(new Error("device id is required"), { code: "device_id_required" });
    }
    const raw = loadJson(CONFIG_FILE, {
      schemaVersion: 1,
      defaultDeviceId: "",
      devices: [],
    });
    if (!Array.isArray(raw.devices)) raw.devices = [];
    const explicitUrl = normalizeLoopbackUrl(input.url, "device url");
    const explicitHealthUrl = normalizeLoopbackUrl(input.healthUrl, "device healthUrl");
    const remotePort = explicitUrl ? portFromDevice({ url: explicitUrl }) : allocateRemotePort(raw, id, input.remotePort);
    const name = String(input.name || id).trim().slice(0, 120) || id;
    const device = {
      id,
      name,
      url: explicitUrl || `http://127.0.0.1:${remotePort}/mcp`,
      healthUrl: explicitHealthUrl || (
        explicitUrl
          ? explicitUrl.replace(/\/mcp\/?$/i, "/health")
          : `http://127.0.0.1:${remotePort}/health`
      ),
      enabled: true,
    };
    const index = raw.devices.findIndex((item) => cleanDeviceId(item?.id) === id);
    if (index >= 0) raw.devices[index] = device;
    else raw.devices.push(device);
    if (!raw.defaultDeviceId) raw.defaultDeviceId = id;
    raw.schemaVersion = 1;
    writeConfig(raw);
    this.reloadConfig();
    this.status.delete(id);
    return {
      ok: true,
      deviceId: id,
      deviceName: name,
      remotePort,
      default: raw.defaultDeviceId === id,
    };
  }

  async refreshDevice(device) {
    const started = Date.now();
    try {
      const response = await fetch(device.healthUrl, {
        signal: AbortSignal.timeout(5000),
      });
      if (!response.ok) throw new Error(`health returned HTTP ${response.status}`);
      const health = await response.json();
      const next = {
        online: health?.ok === true,
        checkedAt: new Date().toISOString(),
        lastSeenAt: health?.ok === true ? new Date().toISOString() : null,
        latencyMs: Date.now() - started,
        version: health?.version || null,
        trustMode: health?.trustMode || null,
        device: health?.device || { id: device.id, name: device.name },
        capabilities: health?.capabilities || {},
        error: "",
      };
      const previous = this.status.get(device.id);
      if (!next.lastSeenAt && previous?.lastSeenAt) next.lastSeenAt = previous.lastSeenAt;
      this.status.set(device.id, next);
      return next;
    } catch (err) {
      const previous = this.status.get(device.id);
      const next = {
        online: false,
        checkedAt: new Date().toISOString(),
        lastSeenAt: previous?.lastSeenAt || null,
        latencyMs: Date.now() - started,
        version: previous?.version || null,
        trustMode: previous?.trustMode || null,
        device: previous?.device || { id: device.id, name: device.name },
        capabilities: previous?.capabilities || {},
        error: err.message || String(err),
      };
      this.status.set(device.id, next);
      return next;
    }
  }

  async refreshAll() {
    this.reloadConfig();
    await Promise.all(this.config.devices.map((device) => this.refreshDevice(device)));
    this.processQueue().catch(() => {});
    return this.publicDevices();
  }

  publicDevices() {
    return this.config.devices.map((device) => {
      const status = this.status.get(device.id) || {};
      return {
        id: device.id,
        name: device.name,
        default: device.id === this.config.defaultDeviceId,
        online: status.online === true,
        checkedAt: status.checkedAt || null,
        lastSeenAt: status.lastSeenAt || null,
        latencyMs: status.latencyMs ?? null,
        version: status.version || null,
        trustMode: status.trustMode || null,
        hostname: status.device?.hostname || null,
        capabilities: status.capabilities || {},
        error: status.error || "",
        pathPrefix: `device://${device.id}/`,
      };
    });
  }

  async callDevice(device, message) {
    const response = await fetch(device.url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(message),
      signal: AbortSignal.timeout(35000),
    });
    if (!response.ok) {
      throw Object.assign(new Error(`device ${device.id} returned HTTP ${response.status}`), {
        code: "device_request_failed",
        deviceId: device.id,
      });
    }
    return response.json();
  }

  async refreshTools() {
    for (const device of this.config.devices) {
      const status = await this.refreshDevice(device);
      if (!status.online) continue;
      const response = await this.callDevice(device, {
        jsonrpc: "2.0",
        id: `tools-${Date.now()}`,
        method: "tools/list",
        params: {},
      });
      if (Array.isArray(response?.result?.tools)) {
        this.toolCache = {
          schemaVersion: 1,
          updatedAt: new Date().toISOString(),
          sourceDeviceId: device.id,
          tools: response.result.tools.map(adaptTool),
        };
        await fsp.writeFile(CACHE_FILE, `${JSON.stringify(this.toolCache, null, 2)}\n`, "utf8");
        break;
      }
    }
    return [...deviceTools, ...(this.toolCache.tools || [])];
  }

  async persistQueue() {
    const temp = `${QUEUE_FILE}.${process.pid}.tmp`;
    await fsp.writeFile(temp, `${JSON.stringify(this.queue, null, 2)}\n`, "utf8");
    await fsp.rename(temp, QUEUE_FILE);
  }

  publicQueueItem(item) {
    return {
      id: item.id,
      deviceId: item.deviceId,
      tool: item.tool,
      status: item.status,
      createdAt: item.createdAt,
      updatedAt: item.updatedAt,
      startedAt: item.startedAt || null,
      finishedAt: item.finishedAt || null,
      attempts: item.attempts || 0,
      error: item.error || "",
      response: item.response || null,
    };
  }

  async queueCall(deviceId, tool, args) {
    const now = new Date().toISOString();
    const queuedArgs = structuredClone(args);
    delete queuedArgs.deviceId;
    delete queuedArgs.queueIfOffline;
    const item = {
      id: `queue_${crypto.randomUUID()}`,
      deviceId,
      tool,
      arguments: queuedArgs,
      status: "queued",
      createdAt: now,
      updatedAt: now,
      startedAt: null,
      finishedAt: null,
      attempts: 0,
      error: "",
      response: null,
    };
    this.queue.items.push(item);
    if (this.queue.items.length > 1000) {
      const removable = this.queue.items.filter((entry) =>
        ["completed", "failed", "cancelled"].includes(entry.status));
      while (this.queue.items.length > 1000 && removable.length) {
        const candidate = removable.shift();
        this.queue.items.splice(this.queue.items.indexOf(candidate), 1);
      }
    }
    await this.persistQueue();
    return this.publicQueueItem(item);
  }

  async processQueue() {
    if (this.queueProcessing) return;
    this.queueProcessing = true;
    try {
      for (const item of this.queue.items) {
        if (item.status !== "queued") continue;
        const device = this.getDevice(item.deviceId);
        const status = device ? this.status.get(device.id) : null;
        if (!device || !status?.online) continue;
        item.status = "running";
        item.startedAt = new Date().toISOString();
        item.updatedAt = item.startedAt;
        item.attempts = (item.attempts || 0) + 1;
        await this.persistQueue();
        try {
          const response = await this.callDevice(device, {
            jsonrpc: "2.0",
            id: item.id,
            method: "tools/call",
            params: { name: item.tool, arguments: item.arguments },
          });
          item.response = response;
          item.status = response?.error ? "failed" : "completed";
          item.error = response?.error?.message || "";
          item.finishedAt = new Date().toISOString();
          item.updatedAt = item.finishedAt;
        } catch (err) {
          item.status = err?.code === "device_request_failed" ? "queued" : "failed";
          item.error = err.message || String(err);
          item.updatedAt = new Date().toISOString();
          if (item.status === "failed") item.finishedAt = item.updatedAt;
          await this.refreshDevice(device);
        }
        await this.persistQueue();
      }
    } finally {
      this.queueProcessing = false;
    }
  }

  async selectDevice(args = {}) {
    const requested = new Set();
    if (args.deviceId) requested.add(String(args.deviceId).toLowerCase());
    extractDeviceIds(args, requested);
    if (requested.size > 1) {
      throw Object.assign(new Error("one tool call cannot target multiple devices"), {
        code: "cross_device_operation_not_supported",
        requestedDevices: [...requested],
      });
    }
    let deviceId = [...requested][0] || null;
    if (!deviceId && this.config.devices.length === 1) {
      deviceId = this.config.devices[0].id || this.config.defaultDeviceId;
    }
    if (!deviceId) {
      throw Object.assign(new Error("deviceId is required when multiple devices are configured"), {
        code: "device_required",
      });
    }
    const device = this.getDevice(deviceId);
    if (!device) {
      throw Object.assign(new Error(`device is not configured: ${deviceId}`), {
        code: "device_not_found",
        deviceId,
      });
    }
    const current = this.status.get(device.id);
    const stale = !current?.checkedAt || Date.now() - Date.parse(current.checkedAt) > HEALTH_INTERVAL_MS;
    const status = stale ? await this.refreshDevice(device) : current;
    if (!status?.online) {
      throw Object.assign(new Error(`device is offline: ${device.id}`), {
        code: "device_offline",
        deviceId: device.id,
      });
    }
    return device;
  }

  async handle(message) {
    const id = message?.id ?? null;
    try {
      if (message?.method === "initialize") {
        return jsonRpc(id, {
          protocolVersion: "2025-03-26",
          capabilities: { tools: { listChanged: true } },
          serverInfo: { name: "hanako-local-device-router", version: "0.8.0" },
        });
      }
      if (message?.method === "notifications/initialized") return null;
      if (message?.method === "ping") return jsonRpc(id, {});
      if (message?.method === "tools/list") {
        return jsonRpc(id, { tools: await this.refreshTools() });
      }
      if (message?.method === "tools/call") {
        const name = String(message.params?.name || "");
        const args = { ...(message.params?.arguments || {}) };
        if (name === "local_device.devices") {
          if (args.refresh === true) await this.refreshAll();
          return jsonRpc(id, contentJson({ devices: this.publicDevices() }));
        }
        if (name === "local_device.queue") {
          const limit = Math.min(500, Math.max(1, Number(args.limit) || 100));
          const items = this.queue.items
            .filter((item) => !args.queueId || item.id === args.queueId)
            .filter((item) => !args.deviceId || item.deviceId === String(args.deviceId).toLowerCase())
            .filter((item) => !args.status || item.status === args.status)
            .slice(-limit)
            .reverse()
            .map((item) => this.publicQueueItem(item));
          return jsonRpc(id, contentJson({ items }));
        }
        if (name === "local_device.cancel_queued") {
          const item = this.queue.items.find((entry) => entry.id === String(args.queueId || ""));
          if (!item) {
            throw Object.assign(new Error("queued call not found"), { code: "queue_not_found" });
          }
          if (item.status !== "queued") {
            throw Object.assign(new Error(`queued call is already ${item.status}`), {
              code: "queue_not_cancellable",
            });
          }
          item.status = "cancelled";
          item.finishedAt = new Date().toISOString();
          item.updatedAt = item.finishedAt;
          await this.persistQueue();
          return jsonRpc(id, contentJson(this.publicQueueItem(item)));
        }

        const queueIfOffline = args.queueIfOffline === true;
        let device;
        try {
          device = await this.selectDevice(args);
        } catch (err) {
          if (queueIfOffline && err?.code === "device_offline" && err.deviceId) {
            const queued = await this.queueCall(err.deviceId, name, args);
            return jsonRpc(id, contentJson({ status: "queued", queue: queued }));
          }
          throw err;
        }
        delete args.deviceId;
        delete args.queueIfOffline;
        try {
          return await this.callDevice(device, {
            jsonrpc: "2.0",
            id,
            method: "tools/call",
            params: { name, arguments: args },
          });
        } catch (err) {
          if (queueIfOffline && err?.code === "device_request_failed") {
            await this.refreshDevice(device);
            const queued = await this.queueCall(device.id, name, args);
            return jsonRpc(id, contentJson({ status: "queued", queue: queued }));
          }
          throw err;
        }
      }
      throw Object.assign(new Error(`unsupported MCP method: ${message?.method}`), {
        code: "method_not_supported",
      });
    } catch (err) {
      return jsonRpcError(id, err);
    }
  }
}

async function main() {
  const router = new DeviceRouter();
  await router.refreshAll();
  await router.refreshTools();
  const interval = setInterval(() => {
    router.refreshAll().catch(() => {});
  }, HEALTH_INTERVAL_MS);
  interval.unref?.();

  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, `http://${req.headers.host || `${HOST}:${PORT}`}`);
      if (req.method === "GET" && url.pathname === "/health") {
        await router.refreshAll();
        return writeJson(res, 200, {
          ok: true,
          version: "0.8.0",
          devices: router.publicDevices(),
          toolCount: deviceTools.length + (router.toolCache.tools || []).length,
          queue: {
            queued: router.queue.items.filter((item) => item.status === "queued").length,
            running: router.queue.items.filter((item) => item.status === "running").length,
          },
        });
      }
      if (req.method === "POST" && url.pathname === "/devices/register") {
        const body = JSON.parse((await readBody(req)) || "{}");
        return writeJson(res, 200, router.registerDevice(body));
      }
      if (req.method !== "POST" || url.pathname !== "/mcp") {
        return writeJson(res, 404, { error: "not found" });
      }
      const payload = JSON.parse((await readBody(req)) || "{}");
      if (Array.isArray(payload)) {
        const responses = (await Promise.all(payload.map((message) => router.handle(message)))).filter(Boolean);
        return writeJson(res, 200, responses);
      }
      const response = await router.handle(payload);
      if (response === null) return writeJson(res, 202, null);
      return writeJson(res, 200, response, { "MCP-Session-Id": "hana-device-router" });
    } catch (err) {
      return writeJson(res, 500, { error: err.message || String(err) });
    }
  });

  await new Promise((resolve) => server.listen(PORT, HOST, resolve));
  console.log(`[device-router] v0.8.0 listening on http://${HOST}:${PORT}/mcp`);
  for (const device of router.publicDevices()) {
    console.log(`[device-router] ${device.id}: ${device.online ? "online" : "offline"}`);
  }
}

main().catch((err) => {
  console.error(`[device-router] failed: ${err.stack || err}`);
  process.exit(1);
});
