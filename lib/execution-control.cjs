const fs = require("fs");
const fsp = require("fs/promises");
const path = require("path");
const crypto = require("crypto");
const { spawn, spawnSync } = require("child_process");

const { loadJson, writeJsonAtomic } = require("./json-store.cjs");
const { appendLineRotating, trimFileTail } = require("./log-utils.cjs");

const SUPPORTED_RUNTIMES = new Set(["powershell", "python"]);

function isInside(filePath, rootPath) {
  const rel = path.relative(rootPath, filePath);
  return rel === "" || (!!rel && !rel.startsWith("..") && !path.isAbsolute(rel));
}

function samePath(a, b) {
  return path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase();
}

function localDrivePath(value, label = "path", deviceId = null, aliases = []) {
  let raw = String(value || "").trim();
  const match = /^device:\/\/([^/]+)\/(.+)$/i.exec(raw);
  if (match) {
    const requestedDevice = decodeURIComponent(match[1]).toLowerCase();
    const accepted = new Set([deviceId, ...aliases].filter(Boolean).map((item) => String(item).toLowerCase()));
    if (!accepted.has(requestedDevice)) {
      throw Object.assign(new Error(`path targets device ${requestedDevice}, but this bridge is ${deviceId}`), {
        code: "wrong_device",
        requestedDevice,
        deviceId,
      });
    }
    raw = decodeURIComponent(match[2]);
  }
  if (!/^[A-Za-z]:[\\/]/.test(raw)) {
    throw Object.assign(new Error(`${label} must be an absolute local Windows drive path`), {
      code: "invalid_local_path",
    });
  }
  if (raw.includes("\0") || raw.startsWith("\\\\.\\") || raw.startsWith("\\\\?\\")) {
    throw Object.assign(new Error("device paths are not allowed"), { code: "invalid_local_path" });
  }
  if (raw.slice(2).includes(":")) {
    throw Object.assign(new Error("alternate data streams are not allowed"), { code: "invalid_local_path" });
  }
  return path.resolve(raw);
}

function normalizeRuntime(value) {
  const runtime = String(value || "").trim().toLowerCase();
  if (!SUPPORTED_RUNTIMES.has(runtime)) {
    throw Object.assign(new Error("runtime must be powershell or python"), {
      code: "unsupported_runtime",
    });
  }
  return runtime;
}

function normalizeArguments(value) {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) {
    throw Object.assign(new Error("arguments must be an array of strings"), {
      code: "invalid_arguments",
    });
  }
  if (value.length > 64) {
    throw Object.assign(new Error("no more than 64 arguments are allowed"), {
      code: "too_many_arguments",
    });
  }
  let total = 0;
  return value.map((item) => {
    const text = String(item);
    if (text.includes("\0") || text.length > 4096) {
      throw Object.assign(new Error("an argument is invalid or too long"), {
        code: "invalid_argument",
      });
    }
    total += text.length;
    if (total > 32768) {
      throw Object.assign(new Error("combined arguments are too long"), {
        code: "arguments_too_long",
      });
    }
    return text;
  });
}

function isAuthorizationActive(item) {
  if (!item || item.enabled === false) return false;
  if (item.expiresAt && Date.parse(item.expiresAt) <= Date.now()) return false;
  if (item.usesRemaining !== null && item.usesRemaining !== undefined && item.usesRemaining <= 0) return false;
  return true;
}

function specsEqual(a, b) {
  return (
    a.runtime === b.runtime &&
    samePath(a.scriptPath, b.scriptPath) &&
    a.scriptSha256 === b.scriptSha256 &&
    samePath(a.cwd, b.cwd) &&
    Number(a.timeoutSeconds) === Number(b.timeoutSeconds) &&
    JSON.stringify(a.arguments || []) === JSON.stringify(b.arguments || [])
  );
}

async function sha256File(file) {
  const hash = crypto.createHash("sha256");
  const handle = await fsp.open(file, "r");
  try {
    const buffer = Buffer.alloc(1024 * 1024);
    let position = 0;
    while (true) {
      const { bytesRead } = await handle.read(buffer, 0, buffer.length, position);
      if (!bytesRead) break;
      hash.update(buffer.subarray(0, bytesRead));
      position += bytesRead;
    }
  } finally {
    await handle.close();
  }
  return hash.digest("hex");
}

function resolveWithWhere(command) {
  if (!command) return null;
  if (command.includes("\\") || command.includes("/")) {
    return fs.existsSync(command) ? path.resolve(command) : null;
  }
  const result = spawnSync("where.exe", [command], {
    windowsHide: true,
    encoding: "utf8",
    timeout: 5000,
  });
  if (result.status !== 0) return null;
  return String(result.stdout || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean) || null;
}

function probeVersion(command, args) {
  if (!command) return null;
  const result = spawnSync(command, args, {
    windowsHide: true,
    encoding: "utf8",
    timeout: 5000,
  });
  if (result.error || result.status !== 0) return null;
  return String(result.stdout || result.stderr || "").trim().split(/\r?\n/)[0] || "available";
}

function appendLimited(target, chunk, maxBytes) {
  const next = Buffer.concat([target, Buffer.from(chunk)]);
  if (next.length <= maxBytes) return { buffer: next, truncated: false };
  return {
    buffer: next.subarray(next.length - maxBytes),
    truncated: true,
  };
}

class ExecutionController {
  constructor(options) {
    this.projectDir = path.resolve(options.projectDir);
    this.dataDir = path.resolve(options.dataDir);
    this.logDir = path.resolve(options.logDir);
    this.approvalUrl = options.approvalUrl;
    this.fullTrust = options.fullTrust === true;
    this.allowChatAuthorization = options.allowChatAuthorization === true;
    this.chatGrantMinutes = Math.min(1440, Math.max(5, Number(options.chatGrantMinutes) || 120));
    this.maxConcurrentJobs = Math.min(8, Math.max(1, Number(options.maxConcurrentJobs) || 2));
    this.maxOutputBytes = Math.min(8 * 1024 * 1024, Math.max(64 * 1024, Number(options.maxOutputBytes) || 1024 * 1024));
    this.maxAuditBytes = Math.max(64 * 1024, Number(options.maxAuditBytes) || 10 * 1024 * 1024);
    this.deviceId = String(options.deviceId || "windows-device").toLowerCase();
    this.deviceName = String(options.deviceName || this.deviceId);
    this.deviceHostname = String(options.deviceHostname || this.deviceName);
    this.deviceAliases = [this.deviceName, this.deviceHostname];
    this.authorizationsFile = path.join(this.dataDir, "execution-authorizations.json");
    this.requestsFile = path.join(this.dataDir, "execution-requests.json");
    this.jobDir = path.join(this.logDir, "jobs");
    this.auditFile = path.join(this.logDir, "execution-audit.jsonl");
    this.state = { schemaVersion: 1, authorizations: [] };
    this.pending = { schemaVersion: 1, requests: [] };
    this.jobs = new Map();
    this.runtimeCache = null;
    this.auditQueue = Promise.resolve();
    this.runnerScript = path.join(this.projectDir, "lib", "job-runner.cjs");
  }

  async init() {
    await fsp.mkdir(this.dataDir, { recursive: true });
    await fsp.mkdir(this.logDir, { recursive: true });
    await fsp.mkdir(this.jobDir, { recursive: true });
    this.state = loadJson(this.authorizationsFile, { schemaVersion: 1, authorizations: [] });
    this.pending = loadJson(this.requestsFile, { schemaVersion: 1, requests: [] });
    if (!Array.isArray(this.state.authorizations)) this.state.authorizations = [];
    if (!Array.isArray(this.pending.requests)) this.pending.requests = [];

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
    this.saveState();
    this.savePending();
    if (bypassedRequests > 0) {
      await this.audit({
        action: "pending_execution_requests_bypassed",
        count: bypassedRequests,
        trustMode: "full",
        success: true,
      });
    }
    await this.recoverJobs();
  }

  saveState() {
    writeJsonAtomic(this.authorizationsFile, this.state);
  }

  savePending() {
    writeJsonAtomic(this.requestsFile, this.pending);
  }

  async normalizeSpec(input = {}) {
    const runtime = normalizeRuntime(input.runtime);
    const scriptPath = localDrivePath(
      input.scriptPath,
      "scriptPath",
      this.deviceId,
      this.deviceAliases,
    );
    const stat = await fsp.stat(scriptPath).catch((err) => {
      if (err && err.code === "ENOENT") {
        throw Object.assign(new Error("script file does not exist"), { code: "script_not_found" });
      }
      throw err;
    });
    if (!stat.isFile()) {
      throw Object.assign(new Error("scriptPath must point to a file"), { code: "script_file_required" });
    }
    const extension = path.extname(scriptPath).toLowerCase();
    if (runtime === "powershell" && extension !== ".ps1") {
      throw Object.assign(new Error("PowerShell execution requires a .ps1 script"), {
        code: "script_extension_mismatch",
      });
    }
    if (runtime === "python" && extension !== ".py") {
      throw Object.assign(new Error("Python execution requires a .py script"), {
        code: "script_extension_mismatch",
      });
    }
    if (!this.fullTrust && (isInside(scriptPath, this.dataDir) || isInside(scriptPath, this.logDir))) {
      throw Object.assign(new Error("bridge control and audit files cannot be executed"), {
        code: "bridge_control_path",
      });
    }

    const cwd = input.cwd
      ? localDrivePath(input.cwd, "cwd", this.deviceId, this.deviceAliases)
      : path.dirname(scriptPath);
    const cwdStat = await fsp.stat(cwd).catch((err) => {
      if (err && err.code === "ENOENT") {
        throw Object.assign(new Error("working directory does not exist"), { code: "cwd_not_found" });
      }
      throw err;
    });
    if (!cwdStat.isDirectory()) {
      throw Object.assign(new Error("cwd must be a directory"), { code: "cwd_directory_required" });
    }

    return {
      runtime,
      scriptPath,
      scriptSha256: await sha256File(scriptPath),
      arguments: normalizeArguments(input.arguments),
      cwd,
      timeoutSeconds: Math.min(1800, Math.max(1, Number(input.timeoutSeconds) || 120)),
      reason: String(input.reason || "").trim().slice(0, 500),
    };
  }

  validateChatAuthorization(spec, quote) {
    const authorizationQuote = String(quote || "").trim();
    if (authorizationQuote.length < 8 || authorizationQuote.length > 2000) {
      throw Object.assign(new Error("the exact current user authorization message is required"), {
        code: "explicit_authorization_required",
      });
    }
    const hasAuthorization =
      /(authorize|allow|approve|permit|permission|\u6388\u6743|\u5141\u8bb8|\u540c\u610f|\u6279\u51c6|\u51c6\u8bb8)/i.test(
        authorizationQuote,
      );
    const hasExecution =
      /(execute|run|launch|\u6267\u884c|\u8fd0\u884c|\u542f\u52a8)/i.test(authorizationQuote);
    if (!hasAuthorization || !hasExecution) {
      throw Object.assign(new Error("the user message must explicitly authorize execution"), {
        code: "explicit_execution_authorization_required",
      });
    }
    const normalizedQuote = authorizationQuote.toLowerCase().replace(/\//g, "\\");
    const normalizedPath = spec.scriptPath.toLowerCase().replace(/\//g, "\\");
    if (!normalizedQuote.includes(normalizedPath)) {
      throw Object.assign(new Error("the authorization message must contain the exact absolute script path"), {
        code: "authorization_path_not_confirmed",
      });
    }
    for (const argument of spec.arguments) {
      if (argument && !authorizationQuote.includes(argument)) {
        throw Object.assign(new Error(`the authorization message must contain the exact argument: ${argument}`), {
          code: "authorization_arguments_not_confirmed",
        });
      }
    }
    return authorizationQuote;
  }

  listAuthorizations() {
    return this.state.authorizations
      .filter((item) => isAuthorizationActive(item))
      .map((item) => this.publicAuthorization(item));
  }

  listRequests() {
    return [...this.pending.requests].sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
  }

  getRequest(id) {
    return this.pending.requests.find((item) => item.id === id) || null;
  }

  findAuthorization(id) {
    return this.state.authorizations.find((item) => item.id === id && isAuthorizationActive(item)) || null;
  }

  findMatchingAuthorization(spec) {
    return this.state.authorizations.find((item) => isAuthorizationActive(item) && specsEqual(item, spec)) || null;
  }

  async requestRun(input = {}) {
    const spec = await this.normalizeSpec(input);
    if (this.fullTrust) {
      const now = new Date();
      const authorization = {
        id: crypto.randomUUID(),
        ...spec,
        source: "full_trust",
        scope: "once",
        usesRemaining: 1,
        enabled: true,
        createdAt: now.toISOString(),
        updatedAt: now.toISOString(),
        expiresAt: new Date(now.getTime() + 30 * 60 * 1000).toISOString(),
      };
      this.state.authorizations = this.state.authorizations.filter((item) => isAuthorizationActive(item));
      this.state.authorizations.push(authorization);
      this.saveState();
      await this.audit({
        action: "full_trust_execution_authorized",
        authorizationId: authorization.id,
        runtime: authorization.runtime,
        scriptPath: authorization.scriptPath,
        scriptSha256: authorization.scriptSha256,
        arguments: authorization.arguments,
        cwd: authorization.cwd,
        timeoutSeconds: authorization.timeoutSeconds,
        success: true,
      });
      return {
        status: "authorized",
        trustMode: "full",
        approvalRequired: false,
        authorization: this.publicAuthorization(authorization),
      };
    }
    const existing = this.findMatchingAuthorization(spec);
    if (existing) {
      return { status: "authorized", authorization: this.publicAuthorization(existing) };
    }

    if (input.userAuthorizationQuote) {
      if (!this.allowChatAuthorization) {
        throw Object.assign(new Error("chat execution authorization is disabled"), {
          code: "chat_authorization_disabled",
          approvalUrl: this.approvalUrl,
        });
      }
      const authorizationQuote = this.validateChatAuthorization(spec, input.userAuthorizationQuote);
      const now = new Date();
      const authorization = {
        id: crypto.randomUUID(),
        ...spec,
        source: "chat_authorization",
        scope: "once",
        usesRemaining: 1,
        enabled: true,
        createdAt: now.toISOString(),
        updatedAt: now.toISOString(),
        expiresAt: new Date(now.getTime() + this.chatGrantMinutes * 60 * 1000).toISOString(),
        authorizationQuote,
      };
      this.state.authorizations.push(authorization);
      this.saveState();
      await this.audit({
        action: "chat_execution_authorized",
        authorizationId: authorization.id,
        runtime: authorization.runtime,
        scriptPath: authorization.scriptPath,
        scriptSha256: authorization.scriptSha256,
        arguments: authorization.arguments,
        success: true,
      });
      return { status: "authorized", authorization: this.publicAuthorization(authorization) };
    }

    const duplicate = this.pending.requests.find(
      (item) => item.status === "pending" && specsEqual(item, spec),
    );
    if (duplicate) {
      return { status: "pending", request: duplicate, approvalUrl: this.approvalUrl };
    }

    const request = {
      id: crypto.randomUUID(),
      ...spec,
      status: "pending",
      createdAt: new Date().toISOString(),
      decidedAt: null,
    };
    this.pending.requests.push(request);
    this.savePending();
    await this.audit({
      action: "execution_requested",
      requestId: request.id,
      runtime: request.runtime,
      scriptPath: request.scriptPath,
      scriptSha256: request.scriptSha256,
      arguments: request.arguments,
      success: true,
    });
    return { status: "pending", request, approvalUrl: this.approvalUrl };
  }

  async approveRequest(id, options = {}) {
    const request = this.getRequest(id);
    if (!request) throw Object.assign(new Error("execution request not found"), { code: "request_not_found" });
    if (request.status !== "pending") {
      throw Object.assign(new Error(`request is already ${request.status}`), {
        code: "request_already_decided",
      });
    }
    const scope = options.scope === "trusted" ? "trusted" : "once";
    const now = new Date();
    const authorization = {
      id: crypto.randomUUID(),
      runtime: request.runtime,
      scriptPath: request.scriptPath,
      scriptSha256: request.scriptSha256,
      arguments: request.arguments,
      cwd: request.cwd,
      timeoutSeconds: request.timeoutSeconds,
      reason: request.reason,
      source: "local_approval",
      scope,
      usesRemaining: scope === "once" ? 1 : null,
      enabled: true,
      createdAt: now.toISOString(),
      updatedAt: now.toISOString(),
      expiresAt: scope === "once" ? new Date(now.getTime() + 30 * 60 * 1000).toISOString() : null,
    };
    this.state.authorizations.push(authorization);
    request.status = "approved";
    request.decidedAt = now.toISOString();
    request.authorizationId = authorization.id;
    request.approvedScope = scope;
    this.saveState();
    this.savePending();
    await this.audit({
      action: "execution_approved",
      requestId: request.id,
      authorizationId: authorization.id,
      scope,
      runtime: authorization.runtime,
      scriptPath: authorization.scriptPath,
      scriptSha256: authorization.scriptSha256,
      arguments: authorization.arguments,
      success: true,
    });
    return this.publicAuthorization(authorization);
  }

  async denyRequest(id) {
    const request = this.getRequest(id);
    if (!request) throw Object.assign(new Error("execution request not found"), { code: "request_not_found" });
    if (request.status !== "pending") {
      throw Object.assign(new Error(`request is already ${request.status}`), {
        code: "request_already_decided",
      });
    }
    request.status = "denied";
    request.decidedAt = new Date().toISOString();
    this.savePending();
    await this.audit({
      action: "execution_denied",
      requestId: request.id,
      runtime: request.runtime,
      scriptPath: request.scriptPath,
      success: true,
    });
    return request;
  }

  async revokeAuthorization(id) {
    const authorization = this.findAuthorization(id);
    if (!authorization) {
      throw Object.assign(new Error("execution authorization not found"), {
        code: "authorization_not_found",
      });
    }
    authorization.enabled = false;
    authorization.updatedAt = new Date().toISOString();
    this.saveState();
    await this.audit({
      action: "execution_authorization_revoked",
      authorizationId: authorization.id,
      runtime: authorization.runtime,
      scriptPath: authorization.scriptPath,
      success: true,
    });
    return this.publicAuthorization(authorization);
  }

  publicAuthorization(item) {
    return {
      id: item.id,
      runtime: item.runtime,
      scriptPath: item.scriptPath,
      scriptSha256: item.scriptSha256,
      arguments: item.arguments,
      cwd: item.cwd,
      timeoutSeconds: item.timeoutSeconds,
      reason: item.reason,
      source: item.source,
      scope: item.scope,
      usesRemaining: item.usesRemaining,
      expiresAt: item.expiresAt || null,
      deviceId: this.deviceId,
    };
  }

  detectRuntimes({ refresh = false } = {}) {
    if (!refresh && this.runtimeCache && Date.now() - this.runtimeCache.checkedAt < 30000) {
      return this.runtimeCache;
    }
    const windowsPowerShell = path.join(
      process.env.SystemRoot || "C:\\Windows",
      "System32",
      "WindowsPowerShell",
      "v1.0",
      "powershell.exe",
    );
    const powershellCommand =
      resolveWithWhere(process.env.LOCAL_EXEC_POWERSHELL_PATH) ||
      resolveWithWhere(windowsPowerShell) ||
      resolveWithWhere("pwsh.exe") ||
      resolveWithWhere("powershell.exe");
    const powershellVersion = probeVersion(powershellCommand, [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$PSVersionTable.PSVersion.ToString()",
    ]);

    let pythonCommand = resolveWithWhere(process.env.LOCAL_EXEC_PYTHON_PATH);
    let pythonPrefix = [];
    if (!pythonCommand) {
      pythonCommand = resolveWithWhere("py.exe");
      if (pythonCommand) pythonPrefix = ["-3"];
    }
    if (!pythonCommand) pythonCommand = resolveWithWhere("python.exe") || resolveWithWhere("python3.exe");
    const pythonVersion = probeVersion(pythonCommand, [...pythonPrefix, "--version"]);

    this.runtimeCache = {
      checkedAt: Date.now(),
      device: {
        id: this.deviceId,
        name: this.deviceName,
        hostname: this.deviceHostname,
      },
      powershell: {
        available: !!powershellCommand && !!powershellVersion,
        command: powershellCommand,
        prefixArguments: [],
        version: powershellVersion,
      },
      python: {
        available: !!pythonCommand && !!pythonVersion,
        command: pythonCommand,
        prefixArguments: pythonPrefix,
        version: pythonVersion,
      },
    };
    return this.runtimeCache;
  }

  countActiveJobs() {
    return [...this.jobs.values()].filter((job) => job.status === "running" || job.status === "starting").length;
  }

  async runAuthorization(id) {
    if (this.countActiveJobs() >= this.maxConcurrentJobs) {
      throw Object.assign(new Error("the local execution concurrency limit has been reached"), {
        code: "execution_busy",
      });
    }
    const authorization = this.findAuthorization(id);
    if (!authorization) {
      throw Object.assign(new Error("execution authorization not found or expired"), {
        code: "authorization_not_found",
        approvalUrl: this.approvalUrl,
      });
    }
    const currentHash = await sha256File(authorization.scriptPath).catch((err) => {
      if (err && err.code === "ENOENT") {
        throw Object.assign(new Error("authorized script no longer exists"), { code: "script_not_found" });
      }
      throw err;
    });
    if (currentHash !== authorization.scriptSha256) {
      authorization.enabled = false;
      authorization.updatedAt = new Date().toISOString();
      this.saveState();
      throw Object.assign(new Error("the script changed after authorization; submit the task again"), {
        code: "script_sha256_mismatch",
        expected: authorization.scriptSha256,
        actual: currentHash,
      });
    }
    const runtimes = this.detectRuntimes({ refresh: true });
    const runtime = runtimes[authorization.runtime];
    if (!runtime?.available) {
      throw Object.assign(new Error(`${authorization.runtime} runtime is not available`), {
        code: "runtime_unavailable",
      });
    }

    if (authorization.usesRemaining !== null && authorization.usesRemaining !== undefined) {
      authorization.usesRemaining -= 1;
      if (authorization.usesRemaining <= 0) authorization.enabled = false;
    }
    authorization.updatedAt = new Date().toISOString();
    this.saveState();

    const jobId = crypto.randomUUID();
    const stdoutFile = path.join(this.jobDir, `${jobId}.stdout.log`);
    const stderrFile = path.join(this.jobDir, `${jobId}.stderr.log`);
    const summaryFile = path.join(this.jobDir, `${jobId}.json`);
    const specFile = path.join(this.jobDir, `${jobId}.spec.json`);
    const stateFile = path.join(this.jobDir, `${jobId}.runner.json`);
    const resultFile = path.join(this.jobDir, `${jobId}.result.json`);
    await Promise.all([
      fsp.writeFile(stdoutFile, "", "utf8"),
      fsp.writeFile(stderrFile, "", "utf8"),
    ]);

    const executableArgs = authorization.runtime === "powershell"
      ? [
          "-NoLogo",
          "-NoProfile",
          "-NonInteractive",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          authorization.scriptPath,
          ...authorization.arguments,
        ]
      : [...(runtime.prefixArguments || []), authorization.scriptPath, ...authorization.arguments];

    const job = {
      id: jobId,
      authorizationId: authorization.id,
      runtime: authorization.runtime,
      scriptPath: authorization.scriptPath,
      scriptSha256: authorization.scriptSha256,
      arguments: authorization.arguments,
      cwd: authorization.cwd,
      timeoutSeconds: authorization.timeoutSeconds,
      status: "starting",
      createdAt: new Date().toISOString(),
      startedAt: null,
      finishedAt: null,
      exitCode: null,
      signal: null,
      error: null,
      timedOut: false,
      cancelled: false,
      stdoutFile,
      stderrFile,
      summaryFile,
      specFile,
      stateFile,
      resultFile,
      runnerPid: null,
      recovered: false,
      stdoutTail: null,
      stderrTail: null,
      stdoutTruncated: false,
      stderrTruncated: false,
      child: null,
      monitorTimer: null,
      checkingRunner: false,
    };
    this.jobs.set(jobId, job);

    writeJsonAtomic(specFile, {
      schemaVersion: 1,
      jobId,
      command: runtime.command,
      arguments: executableArgs,
      cwd: authorization.cwd,
      timeoutSeconds: authorization.timeoutSeconds,
      stdoutFile,
      stderrFile,
      stateFile,
      resultFile,
      environment: {
        PYTHONUTF8: "1",
        PYTHONIOENCODING: "utf-8",
      },
    });
    await this.persistJob(job);

    let runner;
    try {
      runner = spawn(process.execPath, [this.runnerScript, specFile], {
        cwd: this.projectDir,
        windowsHide: true,
        shell: false,
        detached: true,
        stdio: "ignore",
      });
      await new Promise((resolve, reject) => {
        runner.once("spawn", resolve);
        runner.once("error", reject);
      });
      runner.unref();
    } catch (err) {
      await this.finishJob(job, {
        status: "failed",
        error: err.message || String(err),
      });
      throw err;
    }

    job.runnerPid = runner.pid;
    job.status = "running";
    job.startedAt = new Date().toISOString();
    await this.persistJob(job);
    this.startJobMonitor(job);

    await this.audit({
      action: "execution_started",
      jobId: job.id,
      authorizationId: job.authorizationId,
      runtime: job.runtime,
      scriptPath: job.scriptPath,
      scriptSha256: job.scriptSha256,
      arguments: job.arguments,
      cwd: job.cwd,
      timeoutSeconds: job.timeoutSeconds,
      success: true,
    });
    return this.publicJob(job);
  }

  killProcessTree(pid) {
    if (!pid) return;
    spawnSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      windowsHide: true,
      timeout: 10000,
      encoding: "utf8",
    });
  }

  async cancelJob(id) {
    const job = await this.getJob(id);
    if (!job) throw Object.assign(new Error("execution job not found"), { code: "job_not_found" });
    if (!["starting", "running"].includes(job.status)) return this.publicJob(job);
    job.cancelled = true;
    if (job.monitorTimer) clearInterval(job.monitorTimer);
    job.monitorTimer = null;
    this.killProcessTree(job.runnerPid);
    await this.audit({
      action: "execution_cancel_requested",
      jobId: job.id,
      runtime: job.runtime,
      scriptPath: job.scriptPath,
      success: true,
    });
    await this.finishJob(job, {
      status: "cancelled",
      exitCode: null,
      signal: null,
      error: null,
      cancelled: true,
    });
    return this.publicJob(job);
  }

  async persistJob(job) {
    try {
      writeJsonAtomic(job.summaryFile, this.publicJob(job));
    } catch {}
  }

  isProcessAlive(pid) {
    if (!pid) return false;
    try {
      process.kill(Number(pid), 0);
      return true;
    } catch {
      return false;
    }
  }

  async readRunnerResult(job) {
    return loadJson(job.resultFile, null);
  }

  async finishJob(job, result = {}) {
    if (job.finishedAt) return;
    if (job.monitorTimer) clearInterval(job.monitorTimer);
    job.monitorTimer = null;
    job.status = result.status || "failed";
    job.exitCode = result.exitCode ?? null;
    job.signal = result.signal ?? null;
    job.error = result.error ?? null;
    job.timedOut = result.timedOut === true;
    job.cancelled = result.cancelled === true || job.cancelled === true;
    job.finishedAt = result.finishedAt || new Date().toISOString();
    const [stdoutTruncated, stderrTruncated] = await Promise.all([
      trimFileTail(job.stdoutFile, this.maxOutputBytes).catch(() => false),
      trimFileTail(job.stderrFile, this.maxOutputBytes).catch(() => false),
    ]);
    job.stdoutTruncated ||= stdoutTruncated;
    job.stderrTruncated ||= stderrTruncated;
    await this.persistJob(job);
    await this.audit({
      action: "execution_finished",
      jobId: job.id,
      authorizationId: job.authorizationId,
      runtime: job.runtime,
      scriptPath: job.scriptPath,
      scriptSha256: job.scriptSha256,
      arguments: job.arguments,
      status: job.status,
      exitCode: job.exitCode,
      timedOut: job.timedOut,
      cancelled: job.cancelled,
      recovered: job.recovered === true,
      deviceId: this.deviceId,
      success: job.status === "completed",
    });
  }

  async checkRunner(job) {
    if (job.finishedAt || job.checkingRunner) return;
    job.checkingRunner = true;
    try {
      const result = await this.readRunnerResult(job);
      if (result) {
        await this.finishJob(job, result);
        return;
      }
      if (!this.isProcessAlive(job.runnerPid)) {
        await this.finishJob(job, {
          status: "failed",
          error: "execution runner exited without writing a result",
        });
      }
    } finally {
      job.checkingRunner = false;
    }
  }

  startJobMonitor(job) {
    if (job.monitorTimer || job.finishedAt) return;
    job.monitorTimer = setInterval(() => {
      this.checkRunner(job).catch(() => {});
    }, 250);
    job.monitorTimer.unref?.();
    this.checkRunner(job).catch(() => {});
  }

  async recoverJobs() {
    const files = await fsp.readdir(this.jobDir).catch(() => []);
    for (const file of files) {
      if (!/^[0-9a-f-]+\.json$/i.test(file)) continue;
      const summaryFile = path.join(this.jobDir, file);
      const summary = loadJson(summaryFile, null);
      if (!summary || !["starting", "running"].includes(summary.status)) continue;

      const id = summary.id || file.slice(0, -5);
      const stateFile = path.join(this.jobDir, `${id}.runner.json`);
      const resultFile = path.join(this.jobDir, `${id}.result.json`);
      let state = loadJson(stateFile, null);
      if (!state && summary.status === "starting") {
        await new Promise((resolve) => setTimeout(resolve, 1000));
        state = loadJson(stateFile, null);
      }
      const job = {
        ...summary,
        id,
        summaryFile,
        specFile: path.join(this.jobDir, `${id}.spec.json`),
        stateFile,
        resultFile,
        runnerPid: summary.pid || state?.runnerPid || null,
        recovered: true,
        stdoutFile: path.join(this.jobDir, `${id}.stdout.log`),
        stderrFile: path.join(this.jobDir, `${id}.stderr.log`),
        stdoutTail: null,
        stderrTail: null,
        child: null,
        monitorTimer: null,
        checkingRunner: false,
      };
      this.jobs.set(id, job);

      const result = await this.readRunnerResult(job);
      if (result) {
        await this.finishJob(job, result);
      } else if (this.isProcessAlive(job.runnerPid)) {
        this.startJobMonitor(job);
      } else {
        await this.finishJob(job, {
          status: "failed",
          error: "bridge restarted after the execution runner had already stopped",
        });
      }
    }
  }

  async getJob(id) {
    if (this.jobs.has(id)) return this.jobs.get(id);
    const summaryFile = path.join(this.jobDir, `${id}.json`);
    return loadJson(summaryFile, null);
  }

  async waitForJob(id, options = {}) {
    const pollMs = Math.min(1000, Math.max(50, Number(options.pollMs) || 100));
    const initial = await this.getJob(id);
    if (!initial) throw Object.assign(new Error("execution job not found"), { code: "job_not_found" });
    const timeoutMs = Math.min(
      31 * 60 * 1000,
      Math.max(5000, Number(options.timeoutMs) || (Number(initial.timeoutSeconds) + 10) * 1000),
    );
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const job = await this.getJob(id);
      if (!job) throw Object.assign(new Error("execution job not found"), { code: "job_not_found" });
      if (!["starting", "running"].includes(job.status)) return this.publicJob(job);
      await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
    throw Object.assign(new Error("timed out waiting for the local execution job result"), {
      code: "job_wait_timeout",
      jobId: id,
    });
  }

  publicJob(job) {
    return {
      id: job.id,
      authorizationId: job.authorizationId,
      runtime: job.runtime,
      scriptPath: job.scriptPath,
      scriptSha256: job.scriptSha256,
      arguments: job.arguments,
      cwd: job.cwd,
      timeoutSeconds: job.timeoutSeconds,
      status: job.status,
      createdAt: job.createdAt,
      startedAt: job.startedAt,
      finishedAt: job.finishedAt,
      exitCode: job.exitCode,
      signal: job.signal,
      error: job.error,
      timedOut: job.timedOut === true,
      cancelled: job.cancelled === true,
      stdoutTruncated: job.stdoutTruncated === true,
      stderrTruncated: job.stderrTruncated === true,
      pid: job.runnerPid || job.pid || null,
      recovered: job.recovered === true,
      deviceId: this.deviceId,
    };
  }

  async readJobOutput(id, options = {}) {
    const job = await this.getJob(id);
    if (!job) throw Object.assign(new Error("execution job not found"), { code: "job_not_found" });
    const stdout =
      job.stdoutTail instanceof Buffer
        ? job.stdoutTail.toString("utf8")
        : await fsp.readFile(path.join(this.jobDir, `${id}.stdout.log`), "utf8").catch(() => "");
    const stderr =
      job.stderrTail instanceof Buffer
        ? job.stderrTail.toString("utf8")
        : await fsp.readFile(path.join(this.jobDir, `${id}.stderr.log`), "utf8").catch(() => "");
    const maxChars = Math.min(2 * 1024 * 1024, Math.max(1024, Number(options.maxChars) || 256 * 1024));
    return {
      job: this.publicJob(job),
      stdout: stdout.slice(-maxChars),
      stderr: stderr.slice(-maxChars),
      returnedTail: true,
    };
  }

  async audit(event) {
    const record = { timestamp: new Date().toISOString(), ...event };
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
  ExecutionController,
  normalizeArguments,
  normalizeRuntime,
  sha256File,
};
