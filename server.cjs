const http = require("http");
const fsp = require("fs/promises");
const path = require("path");

const { AccessController } = require("./lib/access-control.cjs");
const { createApprovalServer } = require("./lib/approval-server.cjs");
const { CloudConnector } = require("./lib/cloud-connector.cjs");
const { loadDeviceIdentity } = require("./lib/device-identity.cjs");
const { ExecutionController } = require("./lib/execution-control.cjs");
const {
  envBoolean,
  envNumber,
  envString,
  loadRuntimeConfig,
} = require("./lib/runtime-config.cjs");
const {
  createExecutionToolDefinitions,
  createExecutionToolRunner,
} = require("./lib/execution-tools.cjs");
const { createToolDefinitions, createToolRunner } = require("./lib/tools.cjs");
const { version: SERVER_VERSION } = require("./package.json");

const PROJECT_DIR = __dirname;
const RUNTIME = loadRuntimeConfig({ projectDir: PROJECT_DIR });
const CONFIG = RUNTIME.config;
const HOST = envString(process.env, "LOCAL_FS_MCP_HOST", CONFIG.filesystem.host);
const PORT = envNumber(process.env, "LOCAL_FS_MCP_PORT", CONFIG.filesystem.port);
const APPROVAL_HOST = "127.0.0.1";
const APPROVAL_PORT = envNumber(
  process.env,
  "LOCAL_FS_MCP_APPROVAL_PORT",
  CONFIG.filesystem.approvalPort,
);
const DATA_DIR = path.resolve(envString(process.env, "LOCAL_FS_MCP_DATA_DIR", CONFIG.storage.dataDir));
const LOG_DIR = path.resolve(envString(process.env, "LOCAL_FS_MCP_LOG_DIR", CONFIG.storage.logDir));
const CONFIGURED_ROOTS = CONFIG.filesystem.roots;
const DEFAULT_ROOT = CONFIGURED_ROOTS[0];
const DEFAULT_ROOT_NAME = envString(process.env, "LOCAL_FS_MCP_ROOT_NAME", DEFAULT_ROOT.name);
const DEFAULT_ROOT_PATH = path.resolve(
  envString(process.env, "LOCAL_FS_MCP_ROOT", DEFAULT_ROOT.path),
);
const MAX_TEXT_BYTES = Number(process.env.LOCAL_FS_MCP_MAX_TEXT_BYTES || 1024 * 1024);
const MAX_CHUNK_BYTES = Number(process.env.LOCAL_FS_MCP_MAX_CHUNK_BYTES || 1024 * 1024);
const MAX_IMAGE_BYTES = Number(process.env.LOCAL_FS_MCP_MAX_IMAGE_BYTES || 8 * 1024 * 1024);
const MAX_WRITE_BYTES = Number(process.env.LOCAL_FS_MCP_MAX_WRITE_BYTES || 4 * 1024 * 1024);
const MAX_SEARCH_RESULTS = Number(process.env.LOCAL_FS_MCP_MAX_SEARCH_RESULTS || 100);
const TRUST_MODE = envString(process.env, "LOCAL_AGENT_TRUST_MODE", CONFIG.filesystem.trustMode)
  .trim()
  .toLowerCase();
const FULL_TRUST = TRUST_MODE === "full";
const ALLOW_CHAT_AUTHORIZATION = envBoolean(
  process.env,
  "LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION",
  CONFIG.filesystem.allowChatAuthorization,
);
const CHAT_GRANT_MINUTES = envNumber(
  process.env,
  "LOCAL_FS_MCP_CHAT_GRANT_MINUTES",
  CONFIG.filesystem.chatGrantMinutes,
);
const MAX_CONCURRENT_JOBS = Number(process.env.LOCAL_EXEC_MAX_CONCURRENT_JOBS || 2);
const MAX_EXEC_OUTPUT_BYTES = Number(process.env.LOCAL_EXEC_MAX_OUTPUT_BYTES || 1024 * 1024);
const MAX_AUDIT_BYTES = Number(process.env.LOCAL_AGENT_AUDIT_MAX_BYTES || 10 * 1024 * 1024);
const DEVICE = loadDeviceIdentity({
  dataDir: DATA_DIR,
  id: CONFIG.device.id,
  name: CONFIG.device.name,
});
const CLOUD_CONFIG = {
  ...CONFIG.cloud,
  enabled: envBoolean(process.env, "HANA_CLOUD_BRIDGE_ENABLED", CONFIG.cloud.enabled),
  url: envString(process.env, "HANA_CLOUD_BRIDGE_URL", CONFIG.cloud.url),
};
const LOCAL_CAPABILITIES = {
  read: true,
  imageRead: true,
  imageMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  maxImageBytes: MAX_IMAGE_BYTES,
  write: true,
  lineRead: true,
  paginatedList: true,
  boundedSearch: true,
  fileWatch: true,
  deviceIdentity: true,
  devicePaths: true,
  appendText: true,
  exactTextPatch: true,
  textEncodings: ["utf8", "utf16le", "utf16be"],
  powershell: true,
  python: true,
  asynchronousExecution: true,
  fullFileAccess: FULL_TRUST,
  absoluteWindowsPaths: FULL_TRUST,
  approvalRequired: !FULL_TRUST,
  localApproval: !FULL_TRUST,
  chatAuthorization: !FULL_TRUST && ALLOW_CHAT_AUTHORIZATION,
  chatGrantMinutes: CHAT_GRANT_MINUTES,
};

function browserIdentityHosts() {
  const hosts = new Set();
  const values = [
    CONFIG.cloud?.url,
    CONFIG.tunnel?.server,
    ...(process.env.HANA_BROWSER_IDENTITY_HOSTS || "").split(","),
  ];
  for (const value of values) {
    const text = String(value || "").trim();
    if (!text) continue;
    try {
      const parsed = new URL(/^[a-z]+:\/\//i.test(text) ? text : `ssh://${text}`);
      if (parsed.hostname) hosts.add(parsed.hostname.toLowerCase());
    } catch {}
  }
  return [...hosts];
}

function loadBootstrapRoots() {
  const roots = process.env.LOCAL_FS_MCP_ROOT
    ? [{ name: DEFAULT_ROOT_NAME, path: DEFAULT_ROOT_PATH, mode: "read_write" }]
    : CONFIGURED_ROOTS.map((root) => ({ ...root }));
  const extraRaw = process.env.LOCAL_FS_MCP_ROOTS_JSON || "";
  if (extraRaw.trim()) {
    const parsed = JSON.parse(extraRaw);
    for (const item of Array.isArray(parsed) ? parsed : []) {
      if (!item || typeof item !== "object" || !item.name || !item.path) continue;
      roots.push({
        name: String(item.name),
        path: path.resolve(String(item.path)),
        mode: item.mode === "read_write" ? "read_write" : "read",
      });
    }
  }

  const unique = [];
  const seen = new Set();
  for (const root of roots) {
    const key = String(root.name).toLowerCase();
    if (!key || seen.has(key)) continue;
    seen.add(key);
    unique.push(root);
  }
  return unique;
}

function jsonRpc(id, result) {
  return { jsonrpc: "2.0", id, result };
}

function jsonRpcError(id, code, message, data) {
  return {
    jsonrpc: "2.0",
    id,
    error: {
      code,
      message,
      ...(data === undefined ? {} : { data }),
    },
  };
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function writeJson(res, status, data, headers = {}) {
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Headers": "Content-Type, Authorization, MCP-Protocol-Version, MCP-Session-Id",
    "Access-Control-Allow-Methods": "POST, GET, OPTIONS, DELETE",
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    ...headers,
  });
  res.end(data === null || data === undefined ? "" : JSON.stringify(data));
}

async function main() {
  const bootstrapRoots = loadBootstrapRoots();
  for (const root of bootstrapRoots) await fsp.access(root.path);

  const access = new AccessController({
    projectDir: PROJECT_DIR,
    dataDir: DATA_DIR,
    logDir: LOG_DIR,
    bootstrapRoots,
    approvalUrl: `http://${APPROVAL_HOST}:${APPROVAL_PORT}/`,
    fullTrust: FULL_TRUST,
    allowChatAuthorization: ALLOW_CHAT_AUTHORIZATION,
    chatGrantMinutes: CHAT_GRANT_MINUTES,
    maxAuditBytes: MAX_AUDIT_BYTES,
    deviceId: DEVICE.id,
    deviceName: DEVICE.name,
    deviceHostname: DEVICE.hostname,
  });
  await access.init();

  const execution = new ExecutionController({
    projectDir: PROJECT_DIR,
    dataDir: DATA_DIR,
    logDir: LOG_DIR,
    approvalUrl: `http://${APPROVAL_HOST}:${APPROVAL_PORT}/`,
    fullTrust: FULL_TRUST,
    allowChatAuthorization: ALLOW_CHAT_AUTHORIZATION,
    chatGrantMinutes: CHAT_GRANT_MINUTES,
    maxConcurrentJobs: MAX_CONCURRENT_JOBS,
    maxOutputBytes: MAX_EXEC_OUTPUT_BYTES,
    maxAuditBytes: MAX_AUDIT_BYTES,
    deviceId: DEVICE.id,
    deviceName: DEVICE.name,
    deviceHostname: DEVICE.hostname,
  });
  await execution.init();

  let tools = [
    ...createToolDefinitions(access.listGrants().map((grant) => grant.id), FULL_TRUST, DEVICE),
    ...createExecutionToolDefinitions(FULL_TRUST, DEVICE),
  ];
  const callTool = createToolRunner({
    access,
    maxTextBytes: MAX_TEXT_BYTES,
    maxChunkBytes: MAX_CHUNK_BYTES,
    maxImageBytes: MAX_IMAGE_BYTES,
    maxWriteBytes: MAX_WRITE_BYTES,
    maxSearchResults: MAX_SEARCH_RESULTS,
    device: DEVICE,
  });
  const callExecutionTool = createExecutionToolRunner({ execution });

  async function handleRpc(message) {
    const id = message?.id ?? null;
    try {
      if (message.method === "initialize") {
        return jsonRpc(id, {
          protocolVersion: message.params?.protocolVersion || "2025-06-18",
          capabilities: { tools: { listChanged: true } },
          serverInfo: {
            name: "hana-local-fs-mcp",
            title: "Hanako Local File Bridge",
            version: SERVER_VERSION,
          },
        });
      }
      if (message.method === "notifications/initialized") return null;
      if (message.method === "tools/list") {
        tools = [
          ...createToolDefinitions(access.listGrants().map((grant) => grant.id), FULL_TRUST, DEVICE),
          ...createExecutionToolDefinitions(FULL_TRUST, DEVICE),
        ];
        return jsonRpc(id, { tools });
      }
      if (message.method === "tools/call") {
        const startedAt = Date.now();
        try {
          const toolName = String(message.params?.name || "");
          const result = toolName.startsWith("local_exec.")
            ? await callExecutionTool(toolName, message.params?.arguments || {})
            : await callTool(toolName, message.params?.arguments || {});
          await access.audit({
            action: "tool_call",
            tool: toolName,
            durationMs: Date.now() - startedAt,
            success: true,
          });
          return jsonRpc(id, result);
        } catch (err) {
          await access.audit({
            action: "tool_call",
            tool: message.params?.name,
            durationMs: Date.now() - startedAt,
            success: false,
            error: err.message || String(err),
            code: err.code || "tool_error",
          });
          throw err;
        }
      }
      return jsonRpcError(id, -32601, `method not found: ${message.method}`);
    } catch (err) {
      return jsonRpcError(id, -32000, err.message || String(err), {
        code: err.code || "tool_error",
        approvalUrl: err.approvalUrl,
        grantId: err.grantId,
        expected: err.expected,
        actual: err.actual,
      });
    }
  }

  const cloudConnector = new CloudConnector({
    config: CLOUD_CONFIG,
    dataDir: DATA_DIR,
    device: DEVICE,
    version: SERVER_VERSION,
    handleRpc,
    capabilities: LOCAL_CAPABILITIES,
    log: (message) => console.log(`[local-fs-mcp] cloud: ${message}`),
  });

  const mcpServer = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, `http://${req.headers.host || `${HOST}:${PORT}`}`);
      if (req.method === "OPTIONS") return writeJson(res, 204, null);
      if (req.method === "GET" && url.pathname === "/health") {
        return writeJson(res, 200, {
          ok: true,
          version: SERVER_VERSION,
          configPath: RUNTIME.configPath,
          device: DEVICE,
          trustMode: FULL_TRUST ? "full" : "approval",
          approvalUrl: access.approvalUrl,
          roots: access.listGrants(),
          pendingRequests: access.listRequests().filter((request) => request.status === "pending").length,
          pendingExecutions: execution.listRequests().filter((request) => request.status === "pending").length,
          capabilities: LOCAL_CAPABILITIES,
          cloud: cloudConnector.clientIdentity(),
        });
      }
      if (req.method === "DELETE" && url.pathname === "/mcp") return writeJson(res, 200, { ok: true });
      if (req.method !== "POST" || url.pathname !== "/mcp") return writeJson(res, 404, { error: "not found" });

      const payload = JSON.parse((await readBody(req)) || "{}");
      if (Array.isArray(payload)) {
        const responses = (await Promise.all(payload.map(handleRpc))).filter(Boolean);
        return writeJson(res, 200, responses);
      }
      const response = await handleRpc(payload);
      if (response === null) return writeJson(res, 202, null);
      return writeJson(res, 200, response, { "MCP-Session-Id": "hana-local-fs" });
    } catch (err) {
      return writeJson(res, 500, { error: err.message || String(err) });
    }
  });

  const approvalServer = createApprovalServer({
    access,
    execution,
    host: APPROVAL_HOST,
    port: APPROVAL_PORT,
    device: DEVICE,
    version: SERVER_VERSION,
    allowedBrowserHosts: browserIdentityHosts(),
    cloudIdentity: () => cloudConnector.clientIdentity(),
  });

  await Promise.all([
    new Promise((resolve) => mcpServer.listen(PORT, HOST, resolve)),
    new Promise((resolve) => approvalServer.listen(APPROVAL_PORT, APPROVAL_HOST, resolve)),
  ]);

  console.log(`[local-fs-mcp] v${SERVER_VERSION} listening on http://${HOST}:${PORT}/mcp`);
  console.log(`[local-fs-mcp] trust mode: ${FULL_TRUST ? "full" : "approval"}`);
  console.log(`[local-fs-mcp] approval UI: http://${APPROVAL_HOST}:${APPROVAL_PORT}/`);
  cloudConnector.start();
  console.log(`[local-fs-mcp] cloud connector: ${CLOUD_CONFIG.enabled ? CLOUD_CONFIG.url : "disabled"}`);
  for (const root of access.listGrants()) {
    console.log(`[local-fs-mcp] authorized root ${root.id} (${root.mode}): ${root.path}`);
  }

  const stop = () => cloudConnector.stop();
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
}

main().catch((err) => {
  console.error(`[local-fs-mcp] failed: ${err.stack || err.message || err}`);
  process.exit(1);
});
