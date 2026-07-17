const fs = require("fs");
const fsp = require("fs/promises");
const path = require("path");
const crypto = require("crypto");

const { loadJson, writeJsonAtomic } = require("./json-store.cjs");
const { appendLineRotating } = require("./log-utils.cjs");

const MODE_RANK = {
  read: 1,
  read_write: 2,
};

function isInside(filePath, rootPath) {
  const rel = path.relative(rootPath, filePath);
  return rel === "" || (!!rel && !rel.startsWith("..") && !path.isAbsolute(rel));
}

function cleanGrantName(value) {
  const cleaned = String(value || "")
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned.slice(0, 64);
}

function normalizeMode(value, fallback = "read") {
  return value === "read_write" ? "read_write" : value === "read" ? "read" : fallback;
}

function samePath(a, b) {
  return path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase();
}

function isGrantActive(grant) {
  if (!grant || grant.enabled === false) return false;
  if (!grant.expiresAt) return true;
  return Date.parse(grant.expiresAt) > Date.now();
}

function quoteIncludesPath(quote, requestedPath) {
  const normalizedQuote = String(quote || "").toLowerCase().replace(/\//g, "\\");
  const normalizedPath = path.resolve(requestedPath).toLowerCase().replace(/\//g, "\\");
  return normalizedQuote.includes(normalizedPath);
}

function localDrivePath(value) {
  const raw = String(value || "").trim();
  if (!/^[A-Za-z]:[\\/]/.test(raw)) {
    throw Object.assign(new Error("only absolute local Windows drive paths are allowed"), {
      code: "invalid_local_path",
    });
  }
  if (raw.includes("\0") || raw.startsWith("\\\\.\\") || raw.startsWith("\\\\?\\")) {
    throw Object.assign(new Error("device paths are not allowed"), { code: "invalid_local_path" });
  }
  const withoutDrive = raw.slice(2);
  if (withoutDrive.includes(":")) {
    throw Object.assign(new Error("alternate data streams are not allowed"), { code: "invalid_local_path" });
  }
  return path.resolve(raw);
}

function normalizeDevicePath(value, deviceId, aliases = []) {
  const raw = String(value || "").trim();
  const match = /^device:\/\/([^/]+)\/(.+)$/i.exec(raw);
  if (!match) return raw;
  const requestedDevice = decodeURIComponent(match[1]).toLowerCase();
  const accepted = new Set([deviceId, ...aliases].filter(Boolean).map((item) => String(item).toLowerCase()));
  if (!accepted.has(requestedDevice)) {
    throw Object.assign(new Error(`path targets device ${requestedDevice}, but this bridge is ${deviceId}`), {
      code: "wrong_device",
      requestedDevice,
      deviceId,
    });
  }
  return decodeURIComponent(match[2]);
}

function deviceUri(deviceId, absolutePath) {
  return `device://${deviceId}/${path.resolve(absolutePath).replace(/\\/g, "/")}`;
}

class AccessController {
  constructor(options) {
    this.projectDir = path.resolve(options.projectDir);
    this.dataDir = path.resolve(options.dataDir);
    this.logDir = path.resolve(options.logDir);
    this.bootstrapRoots = Array.isArray(options.bootstrapRoots) ? options.bootstrapRoots : [];
    this.approvalUrl = options.approvalUrl;
    this.fullTrust = options.fullTrust === true;
    this.allowChatAuthorization = options.allowChatAuthorization === true;
    this.chatGrantMinutes = Math.min(1440, Math.max(5, Number(options.chatGrantMinutes) || 120));
    this.deviceId = String(options.deviceId || "windows-device").toLowerCase();
    this.deviceName = String(options.deviceName || this.deviceId);
    this.deviceHostname = String(options.deviceHostname || this.deviceName);
    this.deviceAliases = [this.deviceName, this.deviceHostname];
    this.accessFile = path.join(this.dataDir, "access-control.json");
    this.pendingFile = path.join(this.dataDir, "pending-requests.json");
    this.tokenFile = path.join(this.dataDir, "approval-token.txt");
    this.auditFile = path.join(this.logDir, "access-audit.jsonl");
    this.maxAuditBytes = Math.max(64 * 1024, Number(options.maxAuditBytes) || 10 * 1024 * 1024);
    this.auditQueue = Promise.resolve();
    this.state = { schemaVersion: 1, grants: [] };
    this.pending = { schemaVersion: 1, requests: [] };
    this.approvalToken = "";
    this.fullTrustDriveGrants = [];
  }

  async init() {
    await fsp.mkdir(this.dataDir, { recursive: true });
    await fsp.mkdir(this.logDir, { recursive: true });

    this.state = loadJson(this.accessFile, { schemaVersion: 1, grants: [] });
    this.pending = loadJson(this.pendingFile, { schemaVersion: 1, requests: [] });
    if (!Array.isArray(this.state.grants)) this.state.grants = [];
    if (!Array.isArray(this.pending.requests)) this.pending.requests = [];

    for (const root of this.bootstrapRoots) {
      if (!root || !root.name || !root.path) continue;
      const rootPath = path.resolve(root.path);
      const existing = this.state.grants.find(
        (grant) => grant.source === "bootstrap" && (grant.id === root.name || samePath(grant.path, rootPath)),
      );
      const next = {
        id: cleanGrantName(root.name) || `root-${crypto.randomBytes(3).toString("hex")}`,
        name: String(root.name),
        path: rootPath,
        mode: normalizeMode(root.mode, "read"),
        enabled: true,
        source: "bootstrap",
        createdAt: existing?.createdAt || new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      };
      if (existing) Object.assign(existing, next);
      else this.state.grants.push(next);
    }

    this.state.grants = this.state.grants.filter((grant) => grant && grant.id && grant.path);
    this.pending.requests = this.pending.requests.filter((request) => request && request.id && request.path);
    this.fullTrustDriveGrants = this.fullTrust ? this.discoverFullTrustDriveGrants() : [];

    let bypassedRequests = 0;
    if (this.fullTrust) {
      const decidedAt = new Date().toISOString();
      for (const request of this.pending.requests) {
        if (request.status !== "pending") continue;
        request.status = "bypassed_full_trust";
        request.decidedAt = decidedAt;
        bypassedRequests += 1;
      }
    }

    try {
      this.approvalToken = String(await fsp.readFile(this.tokenFile, "utf8")).trim();
    } catch (err) {
      if (!err || err.code !== "ENOENT") throw err;
    }
    if (!this.approvalToken) {
      this.approvalToken = crypto.randomBytes(32).toString("hex");
      await fsp.writeFile(this.tokenFile, `${this.approvalToken}\n`, "utf8");
    }

    this.saveState();
    this.savePending();
    if (bypassedRequests > 0) {
      await this.audit({
        action: "pending_access_requests_bypassed",
        count: bypassedRequests,
        trustMode: "full",
        success: true,
      });
    }
  }

  saveState() {
    writeJsonAtomic(this.accessFile, this.state);
  }

  savePending() {
    writeJsonAtomic(this.pendingFile, this.pending);
  }

  listGrants() {
    const grants = this.fullTrust ? this.fullTrustDriveGrants : this.state.grants;
    const seen = new Set();
    return grants
      .filter((grant) => grant.enabled !== false)
      .filter((grant) => isGrantActive(grant))
      .filter((grant) => {
        const key = String(grant.id).toLowerCase();
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .map((grant) => ({
        id: grant.id,
        name: grant.name,
        path: grant.path,
        mode: grant.mode,
        source: grant.source,
        createdAt: grant.createdAt,
        expiresAt: grant.expiresAt || null,
        deviceId: this.deviceId,
        deviceUri: deviceUri(this.deviceId, grant.path),
      }));
  }

  listRequests() {
    return [...this.pending.requests].sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
  }

  findGrantById(id) {
    const lowered = String(id || "").toLowerCase();
    if (this.fullTrust) {
      const driveMatch = /^drive-([a-z])$/i.exec(String(id || ""));
      if (driveMatch) return this.fullTrustGrantForPath(`${driveMatch[1].toUpperCase()}:\\`);
    }
    return this.state.grants.find((grant) => isGrantActive(grant) && String(grant.id).toLowerCase() === lowered);
  }

  findCoveringGrant(absolutePath, requestedMode = "read") {
    const target = path.resolve(absolutePath);
    if (this.fullTrust) return this.fullTrustGrantForPath(target);
    return this.state.grants
      .filter((grant) => isGrantActive(grant) && MODE_RANK[grant.mode] >= MODE_RANK[requestedMode])
      .filter((grant) => isInside(target, path.resolve(grant.path)))
      .sort((a, b) => path.resolve(b.path).length - path.resolve(a.path).length)[0] || null;
  }

  async requestAccess(input = {}) {
    const requestedPath = localDrivePath(
      normalizeDevicePath(input.path, this.deviceId, this.deviceAliases),
    );
    if (this.fullTrust) {
      const grant = this.fullTrustGrantForPath(requestedPath);
      await this.audit({
        action: "full_trust_access_authorized",
        path: requestedPath,
        mode: "read_write",
        grantId: grant.id,
        success: true,
      });
      return {
        status: "authorized",
        trustMode: "full",
        approvalRequired: false,
        grant: this.publicGrant(grant),
      };
    }
    const stat = await fsp.stat(requestedPath).catch((err) => {
      if (err && err.code === "ENOENT") {
        throw Object.assign(new Error("requested folder does not exist"), { code: "folder_not_found" });
      }
      throw err;
    });
    if (!stat.isDirectory()) {
      throw Object.assign(new Error("access requests must target a folder"), { code: "folder_required" });
    }

    const mode = normalizeMode(input.mode, "read");
    const covering = this.findCoveringGrant(requestedPath, mode);
    if (covering) {
      return {
        status: "authorized",
        grant: this.publicGrant(covering),
      };
    }

    if (input.userAuthorizationQuote) {
      if (!this.allowChatAuthorization) {
        throw Object.assign(new Error("chat authorization is disabled"), {
          code: "chat_authorization_disabled",
          approvalUrl: this.approvalUrl,
        });
      }
      return {
        status: "authorized",
        grant: await this.grantFromChatAuthorization({
          path: requestedPath,
          mode,
          name: input.name,
          reason: input.reason,
          quote: input.userAuthorizationQuote,
        }),
      };
    }

    const duplicate = this.pending.requests.find(
      (request) => request.status === "pending" && samePath(request.path, requestedPath) && request.mode === mode,
    );
    if (duplicate) {
      return {
        status: "pending",
        request: duplicate,
        approvalUrl: this.approvalUrl,
      };
    }

    const request = {
      id: crypto.randomUUID(),
      path: requestedPath,
      mode,
      name: cleanGrantName(input.name) || "",
      reason: String(input.reason || "").trim().slice(0, 500),
      status: "pending",
      createdAt: new Date().toISOString(),
      decidedAt: null,
    };
    this.pending.requests.push(request);
    this.savePending();
    await this.audit({
      action: "access_requested",
      requestId: request.id,
      path: request.path,
      mode: request.mode,
      reason: request.reason,
      success: true,
    });
    return {
      status: "pending",
      request,
      approvalUrl: this.approvalUrl,
    };
  }

  validateChatAuthorization({ path: requestedPath, mode, quote }) {
    const authorizationQuote = String(quote || "").trim();
    if (authorizationQuote.length < 8 || authorizationQuote.length > 1000) {
      throw Object.assign(new Error("the exact current user authorization message is required"), {
        code: "explicit_authorization_required",
      });
    }
    if (!/(授权|允许|同意|批准|可以访问|准许)/i.test(authorizationQuote)) {
      throw Object.assign(new Error("the user message does not contain explicit authorization"), {
        code: "explicit_authorization_required",
      });
    }
    if (!quoteIncludesPath(authorizationQuote, requestedPath)) {
      throw Object.assign(new Error("the authorization message must contain the exact absolute folder path"), {
        code: "authorization_path_not_confirmed",
      });
    }
    if (
      mode === "read_write" &&
      !/(读写|写入|修改|创建|新建|删除|移动|重命名|覆盖|编辑)/i.test(authorizationQuote)
    ) {
      throw Object.assign(new Error("read/write access requires the user to explicitly authorize a write action"), {
        code: "write_authorization_required",
      });
    }
    return authorizationQuote;
  }

  async grantFromChatAuthorization(input) {
    const authorizationQuote = this.validateChatAuthorization(input);
    const requestedName = cleanGrantName(input.name);
    const baseName =
      requestedName ||
      cleanGrantName(path.basename(input.path)) ||
      `drive-${String(input.path || "C")[0].toUpperCase()}`;
    let grantId = baseName;
    let suffix = 2;
    while (this.findGrantById(grantId)) grantId = `${baseName}-${suffix++}`;

    const now = new Date();
    const expiresAt = new Date(now.getTime() + this.chatGrantMinutes * 60 * 1000).toISOString();
    const grant = {
      id: grantId,
      name: grantId,
      path: input.path,
      mode: input.mode,
      enabled: true,
      source: "chat_authorization",
      createdAt: now.toISOString(),
      updatedAt: now.toISOString(),
      expiresAt,
      authorizationQuote,
      reason: String(input.reason || "").trim().slice(0, 500),
    };
    this.state.grants.push(grant);
    this.saveState();
    await this.audit({
      action: "chat_access_authorized",
      grantId: grant.id,
      path: grant.path,
      mode: grant.mode,
      expiresAt,
      reason: grant.reason,
      success: true,
    });
    return this.publicGrant(grant);
  }

  getRequest(id) {
    return this.pending.requests.find((request) => request.id === id) || null;
  }

  async approveRequest(id, options = {}) {
    const request = this.getRequest(id);
    if (!request) throw Object.assign(new Error("request not found"), { code: "request_not_found" });
    if (request.status !== "pending") {
      throw Object.assign(new Error(`request is already ${request.status}`), { code: "request_already_decided" });
    }

    const mode = normalizeMode(options.mode, request.mode);
    const requestedName = cleanGrantName(options.name || request.name);
    const baseName = requestedName || cleanGrantName(path.basename(request.path)) || `drive-${request.path[0]}`;
    let grantId = baseName;
    let suffix = 2;
    while (this.findGrantById(grantId) && !samePath(this.findGrantById(grantId).path, request.path)) {
      grantId = `${baseName}-${suffix++}`;
    }

    let grant = this.state.grants.find((item) => samePath(item.path, request.path));
    if (!grant) {
      grant = {
        id: grantId,
        name: grantId,
        path: request.path,
        mode,
        enabled: true,
        source: "local_approval",
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
        expiresAt: null,
      };
      this.state.grants.push(grant);
    } else {
      grant.id = grantId;
      grant.name = grantId;
      grant.mode = mode;
      grant.enabled = true;
      grant.updatedAt = new Date().toISOString();
      grant.expiresAt = null;
    }

    request.status = "approved";
    request.decidedAt = new Date().toISOString();
    request.approvedGrantId = grant.id;
    request.approvedMode = grant.mode;
    this.saveState();
    this.savePending();
    await this.audit({
      action: "access_approved",
      requestId: request.id,
      grantId: grant.id,
      path: grant.path,
      mode: grant.mode,
      success: true,
    });
    return this.publicGrant(grant);
  }

  async denyRequest(id) {
    const request = this.getRequest(id);
    if (!request) throw Object.assign(new Error("request not found"), { code: "request_not_found" });
    if (request.status !== "pending") {
      throw Object.assign(new Error(`request is already ${request.status}`), { code: "request_already_decided" });
    }
    request.status = "denied";
    request.decidedAt = new Date().toISOString();
    this.savePending();
    await this.audit({
      action: "access_denied",
      requestId: request.id,
      path: request.path,
      mode: request.mode,
      success: true,
    });
    return request;
  }

  async revokeGrant(id) {
    const grant = this.findGrantById(id);
    if (!grant) throw Object.assign(new Error("grant not found"), { code: "grant_not_found" });
    if (grant.source === "bootstrap") {
      throw Object.assign(new Error("bootstrap grants are managed by the service configuration"), {
        code: "bootstrap_grant",
      });
    }
    grant.enabled = false;
    grant.updatedAt = new Date().toISOString();
    this.saveState();
    await this.audit({
      action: "access_revoked",
      grantId: grant.id,
      path: grant.path,
      mode: grant.mode,
      success: true,
    });
    return this.publicGrant(grant);
  }

  publicGrant(grant) {
    return {
      id: grant.id,
      name: grant.name,
      path: grant.path,
      mode: grant.mode,
      source: grant.source,
      uri: `local://${grant.id}`,
      deviceId: this.deviceId,
      deviceUri: deviceUri(this.deviceId, grant.path),
      expiresAt: grant.expiresAt || null,
    };
  }

  discoverFullTrustDriveGrants() {
    const grants = [];
    for (let code = 65; code <= 90; code += 1) {
      const drivePath = `${String.fromCharCode(code)}:\\`;
      if (!fs.existsSync(drivePath)) continue;
      grants.push(this.fullTrustGrantForPath(drivePath));
    }
    return grants;
  }

  fullTrustGrantForPath(input) {
    const absolutePath = localDrivePath(
      normalizeDevicePath(input, this.deviceId, this.deviceAliases),
    );
    const driveRoot = path.parse(absolutePath).root;
    const driveLetter = driveRoot.slice(0, 1).toUpperCase();
    return {
      id: `Drive-${driveLetter}`,
      name: `Drive-${driveLetter}`,
      path: driveRoot,
      mode: "read_write",
      enabled: true,
      source: "full_trust",
      createdAt: null,
      updatedAt: null,
      expiresAt: null,
    };
  }

  parseVirtualPath(input) {
    let raw = normalizeDevicePath(input, this.deviceId, this.deviceAliases);
    if (/^[A-Za-z]:[\\/]/.test(raw)) {
      const rawParts = raw.slice(2).split(/[\\/]+/).filter(Boolean);
      if (rawParts.some((part) => part === "..")) {
        throw Object.assign(new Error("path traversal is not allowed"), { code: "path_not_authorized" });
      }
      const absolutePath = localDrivePath(raw);
      const grant = this.fullTrust
        ? this.fullTrustGrantForPath(absolutePath)
        : this.findCoveringGrant(absolutePath, "read");
      if (!grant) {
        throw Object.assign(new Error("path is not under an authorized local root"), {
          code: "path_not_authorized",
          approvalUrl: this.approvalUrl,
        });
      }
      return {
        grant,
        relative: path.relative(grant.path, absolutePath).split(path.sep).filter(Boolean).join("/"),
      };
    }
    raw = raw.replace(/^local:\/\/\/?/i, "");
    raw = raw.replace(/^windows\//i, "");
    raw = raw.replace(/\\/g, "/");
    raw = raw.replace(/^\/+/, "");
    if (raw.includes("\0")) {
      throw Object.assign(new Error("invalid path"), { code: "invalid_path" });
    }

    const grants = [
      ...(this.fullTrust ? this.fullTrustDriveGrants : []),
      ...this.state.grants,
    ]
      .filter((grant) => isGrantActive(grant))
      .sort((a, b) => b.id.length - a.id.length);
    for (const grant of grants) {
      const candidate = raw.toLowerCase();
      const id = grant.id.toLowerCase();
      if (candidate === id) {
        if (this.fullTrust && grant.source !== "full_trust") {
          const fullTrustGrant = this.fullTrustGrantForPath(grant.path);
          return {
            grant: fullTrustGrant,
            relative: path.relative(fullTrustGrant.path, path.resolve(grant.path)).split(path.sep).filter(Boolean).join("/"),
          };
        }
        return { grant, relative: "" };
      }
      if (candidate.startsWith(`${id}/`)) {
        const relative = raw.slice(grant.id.length + 1);
        if (this.fullTrust && grant.source !== "full_trust") {
          const parts = relative.split("/").filter(Boolean);
          if (parts.some((part) => part === "..")) {
            throw Object.assign(new Error("path traversal is not allowed"), { code: "path_not_authorized" });
          }
          const absolutePath = path.resolve(grant.path, ...parts);
          const fullTrustGrant = this.fullTrustGrantForPath(absolutePath);
          return {
            grant: fullTrustGrant,
            relative: path.relative(fullTrustGrant.path, absolutePath).split(path.sep).filter(Boolean).join("/"),
          };
        }
        return { grant, relative };
      }
    }

    const defaultGrant = grants.find((grant) => grant.source === "bootstrap") || grants[0];
    if (defaultGrant && !raw.includes("/")) return { grant: defaultGrant, relative: raw };
    throw Object.assign(new Error("path is not under an authorized local:// root"), {
      code: "path_not_authorized",
      approvalUrl: this.approvalUrl,
    });
  }

  async resolvePath(input, requiredMode = "read", options = {}) {
    const { grant, relative } = this.parseVirtualPath(input);
    if (MODE_RANK[grant.mode] < MODE_RANK[requiredMode]) {
      throw Object.assign(new Error(`root ${grant.id} is ${grant.mode}; ${requiredMode} access is required`), {
        code: "write_not_authorized",
        grantId: grant.id,
        approvalUrl: this.approvalUrl,
      });
    }

    const normalizedParts = relative.split("/").filter(Boolean);
    if (normalizedParts.some((part) => part === "..")) {
      throw Object.assign(new Error("path traversal is not allowed"), { code: "path_not_authorized" });
    }
    if (normalizedParts.some((part) => part.includes(":"))) {
      throw Object.assign(new Error("alternate data streams are not allowed"), { code: "path_not_authorized" });
    }
    if (normalizedParts.some((part) => part.toLowerCase() === ".hana-trash")) {
      throw Object.assign(new Error("the internal trash folder is not directly accessible"), {
        code: "path_not_authorized",
      });
    }

    const rootReal = await fsp.realpath(grant.path);
    const candidate = path.resolve(rootReal, ...normalizedParts);
    if (!isInside(candidate, rootReal)) {
      throw Object.assign(new Error("path is outside authorized root"), { code: "path_not_authorized" });
    }

    let real = candidate;
    let exists = true;
    try {
      real = await fsp.realpath(candidate);
    } catch (err) {
      if (!err || err.code !== "ENOENT") throw err;
      exists = false;
      if (!options.allowMissing) throw err;
      let ancestor = path.dirname(candidate);
      while (ancestor !== path.dirname(ancestor)) {
        try {
          const ancestorReal = await fsp.realpath(ancestor);
          if (!isInside(ancestorReal, rootReal)) {
            throw Object.assign(new Error("path parent escapes authorized root"), {
              code: "path_not_authorized",
            });
          }
          break;
        } catch (ancestorErr) {
          if (!ancestorErr || ancestorErr.code !== "ENOENT") throw ancestorErr;
          ancestor = path.dirname(ancestor);
        }
      }
    }

    if (exists && !isInside(real, rootReal)) {
      throw Object.assign(new Error("resolved path escapes authorized root"), { code: "path_not_authorized" });
    }

    if (!this.fullTrust) {
      const protectedRead =
        isInside(real, this.dataDir) ||
        isInside(real, this.logDir);
      if (protectedRead) {
        throw Object.assign(new Error("bridge control and audit data are not accessible through MCP"), {
          code: "bridge_control_path",
        });
      }
      if (requiredMode === "read_write" && isInside(real, this.projectDir)) {
        throw Object.assign(new Error("the bridge program directory is read-only to MCP"), {
          code: "bridge_program_read_only",
        });
      }
    }

    return {
      grant,
      rootReal,
      real,
      exists,
      relative: path.relative(rootReal, real).split(path.sep).filter(Boolean).join("/"),
    };
  }

  async audit(event) {
    const record = {
      timestamp: new Date().toISOString(),
      ...event,
    };
    this.auditQueue = this.auditQueue
      .then(() =>
        appendLineRotating(this.auditFile, `${JSON.stringify(record)}\n`, {
          maxBytes: this.maxAuditBytes,
          backups: 5,
        }),
      )
      .catch(() => {});
    await this.auditQueue;
  }
}

module.exports = {
  AccessController,
  cleanGrantName,
  isInside,
  normalizeMode,
};
