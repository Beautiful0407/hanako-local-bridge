const fs = require("fs");
const os = require("os");
const path = require("path");

const { cleanDeviceId } = require("./device-identity.cjs");
const OFFICIAL_CLOUD_URL = "wss://your-server.example.com/local-bridge/connect";
const LEGACY_CLOUD_URL = "ws://YOUR_SERVER_IP/local-bridge/connect";
const OFFICIAL_UPDATE_MANIFEST =
  "https://your-server.example.com/local-bridge/releases/update-manifest.json";

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function isObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function mergeDeep(base, override) {
  if (!isObject(override)) return clone(base);
  const merged = clone(base);
  for (const [key, value] of Object.entries(override)) {
    if (isObject(value) && isObject(merged[key])) {
      merged[key] = mergeDeep(merged[key], value);
    } else {
      merged[key] = clone(value);
    }
  }
  return merged;
}

function expandEnvironment(value, installDir) {
  return String(value || "")
    .replace(/%INSTALLDIR%/gi, installDir)
    .replace(/%([^%]+)%/g, (match, name) => process.env[name] ?? match);
}

function resolveConfiguredPath(value, installDir) {
  const expanded = expandEnvironment(value, installDir);
  return path.resolve(path.isAbsolute(expanded) ? expanded : path.join(installDir, expanded));
}

function defaultRootPath() {
  const home = process.env.USERPROFILE || os.homedir() || process.cwd();
  const workspace = path.join(home, "Desktop", "OH-WorkSpace");
  return fs.existsSync(workspace) ? workspace : home;
}

function createDefaultConfig(installDir) {
  const hostname = os.hostname() || process.env.COMPUTERNAME || "Windows Device";
  const deviceId = cleanDeviceId(process.env.COMPUTERNAME || hostname) || "windows-device";
  const rootPath = defaultRootPath();
  return {
    schemaVersion: 1,
    device: {
      id: deviceId,
      name: process.env.COMPUTERNAME || hostname || deviceId,
    },
    filesystem: {
      host: "127.0.0.1",
      port: 8787,
      approvalPort: 8788,
      trustMode: "full",
      allowChatAuthorization: false,
      chatGrantMinutes: 120,
      roots: [
        {
          name: path.basename(rootPath) || "LocalFiles",
          path: rootPath,
          mode: "read_write",
        },
        {
          name: "HanakoLocalBridge",
          path: installDir,
          mode: "read",
        },
      ],
    },
    storage: {
      dataDir: "data",
      logDir: "logs",
    },
    cloud: {
      enabled: true,
      url: OFFICIAL_CLOUD_URL,
      reconnectMinSeconds: 3,
      reconnectMaxSeconds: 60,
      heartbeatSeconds: 25,
    },
    tunnel: {
      enabled: false,
      server: "YOUR_SERVER_IP",
      user: "root",
      localHost: "127.0.0.1",
      localPort: 8787,
      remoteHost: "127.0.0.1",
      remotePort: 18787,
      identityFile: "",
    },
    service: {
      taskPrefix: "Hanako Local FS",
      restartDelaySeconds: 3,
      tunnelRetryMinSeconds: 5,
      tunnelRetryMaxSeconds: 60,
      tunnelHealthSeconds: 30,
    },
    update: {
      manifest: OFFICIAL_UPDATE_MANIFEST,
      channel: "stable",
    },
  };
}

function normalizeRoots(roots, installDir) {
  const normalized = [];
  const seen = new Set();
  for (const item of Array.isArray(roots) ? roots : []) {
    if (!item || typeof item !== "object" || !item.path) continue;
    const rootPath = resolveConfiguredPath(item.path, installDir);
    const name = String(item.name || path.basename(rootPath) || "LocalFiles").trim();
    const key = name.toLowerCase();
    if (!name || seen.has(key)) continue;
    seen.add(key);
    normalized.push({
      name,
      path: rootPath,
      mode: item.mode === "read" ? "read" : "read_write",
    });
  }
  return normalized;
}

function loadRuntimeConfig(options = {}) {
  const installDir = path.resolve(options.installDir || options.projectDir || path.resolve(__dirname, ".."));
  const configPath = path.resolve(
    options.configPath || process.env.HANA_LOCAL_BRIDGE_CONFIG || path.join(installDir, "config.json"),
  );
  const defaults = createDefaultConfig(installDir);
  let source = {};
  try {
    source = JSON.parse(fs.readFileSync(configPath, "utf8"));
  } catch (err) {
    if (err?.code !== "ENOENT") throw new Error(`cannot load bridge config ${configPath}: ${err.message}`);
  }

  const config = mergeDeep(defaults, source);
  if (!isObject(source.cloud)) {
    const server = String(config.tunnel?.server || "").trim();
    if (server && server !== "YOUR_SERVER_IP") {
      config.cloud.url = `ws://${server}/local-bridge/connect`;
    } else {
      config.cloud.url = OFFICIAL_CLOUD_URL;
    }
    config.cloud.enabled = true;
    config.tunnel.enabled = false;
  }
  if (String(config.cloud.url || "").trim() === LEGACY_CLOUD_URL) {
    config.cloud.url = OFFICIAL_CLOUD_URL;
  }
  const updateManifest = String(config.update?.manifest || "").trim();
  if (
    !updateManifest ||
    /\\Desktop\\Hanako-Local-FS-MCP-Bridge\\release\\update-manifest\.json$/i.test(updateManifest)
  ) {
    config.update.manifest = OFFICIAL_UPDATE_MANIFEST;
  }
  config.filesystem.roots = normalizeRoots(config.filesystem.roots, installDir);
  if (config.filesystem.roots.length === 0) {
    config.filesystem.roots = normalizeRoots(defaults.filesystem.roots, installDir);
  }
  config.storage.dataDir = resolveConfiguredPath(config.storage.dataDir, installDir);
  config.storage.logDir = resolveConfiguredPath(config.storage.logDir, installDir);
  config.tunnel.identityFile = config.tunnel.identityFile
    ? resolveConfiguredPath(config.tunnel.identityFile, installDir)
    : "";

  return {
    installDir,
    configPath,
    exists: fs.existsSync(configPath),
    config,
  };
}

function envString(env, name, fallback) {
  const value = env[name];
  return value === undefined || String(value).trim() === "" ? fallback : String(value);
}

function envNumber(env, name, fallback) {
  const value = Number(envString(env, name, fallback));
  return Number.isFinite(value) ? value : Number(fallback);
}

function envBoolean(env, name, fallback) {
  const value = env[name];
  if (value === undefined || String(value).trim() === "") return Boolean(fallback);
  return ["1", "true", "yes", "on"].includes(String(value).trim().toLowerCase());
}

module.exports = {
  createDefaultConfig,
  envBoolean,
  envNumber,
  envString,
  expandEnvironment,
  loadRuntimeConfig,
  mergeDeep,
  normalizeRoots,
  resolveConfiguredPath,
};
