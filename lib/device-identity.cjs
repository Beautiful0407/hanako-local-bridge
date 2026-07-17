const os = require("os");
const path = require("path");

const { loadJson, writeJsonAtomic } = require("./json-store.cjs");

function cleanDeviceId(value) {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

function loadDeviceIdentity(options = {}) {
  const dataDir = path.resolve(options.dataDir);
  const file = path.join(dataDir, "device.json");
  const existing = loadJson(file, { schemaVersion: 1 });
  const hostname = os.hostname();
  const id =
    cleanDeviceId(process.env.LOCAL_AGENT_DEVICE_ID) ||
    cleanDeviceId(options.id) ||
    cleanDeviceId(existing.id) ||
    cleanDeviceId(process.env.COMPUTERNAME) ||
    cleanDeviceId(hostname) ||
    "windows-device";
  const name =
    String(
      process.env.LOCAL_AGENT_DEVICE_NAME ||
        options.name ||
        existing.name ||
        process.env.COMPUTERNAME ||
        hostname ||
        id,
    ).trim() ||
    id;
  const identity = {
    schemaVersion: 1,
    id,
    name,
    hostname,
    platform: process.platform,
    updatedAt: new Date().toISOString(),
  };
  writeJsonAtomic(file, identity);
  return identity;
}

module.exports = {
  cleanDeviceId,
  loadDeviceIdentity,
};
