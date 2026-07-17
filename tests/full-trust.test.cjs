const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");
const { version: packageVersion } = require("../package.json");

const projectDir = path.resolve(__dirname, "..");
const serverFile = path.join(projectDir, "server.cjs");

async function waitFor(url, child) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`server exited early with code ${child.exitCode}`);
    try {
      const response = await fetch(url);
      if (response.ok) return response.json();
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${url}`);
}

async function rpc(base, id, name, args = {}) {
  const response = await fetch(`${base}/mcp`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: { name, arguments: args },
    }),
  });
  assert.equal(response.status, 200);
  return response.json();
}

function textResult(response) {
  assert.ok(response.result, JSON.stringify(response));
  return response.result.content[0].text;
}

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-full-trust-test-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const powershellScript = path.join(root, "full-trust.ps1");
  const pythonScript = path.join(root, "full-trust.py");
  await fsp.mkdir(root, { recursive: true });
  await fsp.writeFile(
    powershellScript,
    'param([string]$Name)\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\nWrite-Output "FULL:$Name"\n',
    "utf8",
  );
  await fsp.writeFile(pythonScript, 'import sys\nprint("FULLPY:" + sys.argv[1])\n', "utf8");

  const mcpPort = 32000 + Math.floor(Math.random() * 1000);
  const approvalPort = mcpPort + 1;
  const env = {
    ...process.env,
    LOCAL_AGENT_TRUST_MODE: "full",
    LOCAL_FS_MCP_ROOT: root,
    LOCAL_FS_MCP_ROOT_NAME: "TestRoot",
    LOCAL_FS_MCP_ROOTS_JSON: JSON.stringify([
      { name: "Bridge", path: projectDir, mode: "read" },
    ]),
    LOCAL_FS_MCP_HOST: "127.0.0.1",
    LOCAL_FS_MCP_PORT: String(mcpPort),
    LOCAL_FS_MCP_APPROVAL_PORT: String(approvalPort),
    LOCAL_FS_MCP_DATA_DIR: data,
    LOCAL_FS_MCP_LOG_DIR: logs,
    LOCAL_AGENT_DEVICE_ID: "full-trust-device",
    LOCAL_AGENT_DEVICE_NAME: "Full Trust Device",
    LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION: "0",
    HANA_CLOUD_BRIDGE_ENABLED: "0",
  };

  let stdout = "";
  let stderr = "";
  const child = spawn(process.execPath, [serverFile], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });

  const base = `http://127.0.0.1:${mcpPort}`;
  try {
    const health = await waitFor(`${base}/health`, child);
    assert.equal(health.version, packageVersion);
    assert.equal(health.device.id, "full-trust-device");
    assert.equal(health.trustMode, "full");
    assert.equal(health.capabilities.fullFileAccess, true);
    assert.equal(health.capabilities.absoluteWindowsPaths, true);
    assert.equal(health.capabilities.approvalRequired, false);
    assert.equal(health.pendingRequests, 0);
    assert.equal(health.pendingExecutions, 0);

    const toolsResponse = await fetch(`${base}/mcp`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }),
    }).then((response) => response.json());
    const tools = new Map(toolsResponse.result.tools.map((tool) => [tool.name, tool]));
    assert.match(tools.get("local_fs.read_text").description, /absolute Windows/);
    assert.match(tools.get("local_exec.execute").description, /without a quote or approval/);
    assert.equal(tools.get("local_fs.request_access").inputSchema.properties.userAuthorizationQuote, undefined);
    assert.equal(tools.get("local_exec.execute").inputSchema.properties.userAuthorizationQuote, undefined);

    const listed = await rpc(base, 2, "local_fs.list", { path: root });
    const listedResult = JSON.parse(textResult(listed));
    assert.equal(listedResult.mode, "read_write");
    assert.ok(listedResult.entries.some((entry) => entry.name === "full-trust.ps1"));

    const directPath = path.join(data, "direct.txt");
    const write = await rpc(base, 3, "local_fs.write_text", {
      path: directPath,
      text: "full trust",
    });
    const writeStat = JSON.parse(textResult(write));
    assert.equal(writeStat.size, 10);
    assert.equal(await fsp.readFile(directPath, "utf8"), "full trust");

    const read = await rpc(base, 4, "local_fs.read_text", { path: directPath });
    assert.equal(textResult(read), "full trust");

    const legacyAliasPath = `local://Bridge/full-trust-alias-${process.pid}.txt`;
    const legacyAliasRealPath = path.join(projectDir, `full-trust-alias-${process.pid}.txt`);
    try {
      const aliasWrite = await rpc(base, 41, "local_fs.write_text", {
        path: legacyAliasPath,
        text: "legacy alias is writable in full trust",
      });
      assert.ok(aliasWrite.result, JSON.stringify(aliasWrite));
      assert.equal(await fsp.readFile(legacyAliasRealPath, "utf8"), "legacy alias is writable in full trust");
    } finally {
      await fsp.rm(legacyAliasRealPath, { force: true });
    }

    const accessRequest = await rpc(base, 5, "local_fs.request_access", {
      path: root,
      mode: "read_write",
    });
    const accessResult = JSON.parse(textResult(accessRequest));
    assert.equal(accessResult.status, "authorized");
    assert.equal(accessResult.trustMode, "full");
    assert.equal(accessResult.approvalRequired, false);

    const executionScriptPath = `device://full-trust-device/${powershellScript.replace(/\\/g, "/")}`;
    const execution = await rpc(base, 6, "local_exec.execute", {
      runtime: "powershell",
      scriptPath: executionScriptPath,
      arguments: ["direct"],
      timeoutSeconds: 30,
      reason: "full trust integration test",
    });
    const executionResult = JSON.parse(textResult(execution));
    assert.equal(executionResult.status, "completed");
    assert.equal(executionResult.authorization.source, "full_trust");
    assert.equal(executionResult.job.exitCode, 0);
    assert.match(executionResult.stdout, /FULL:direct/);
    assert.equal(executionResult.authorization.deviceId, "full-trust-device");
    assert.equal(executionResult.job.deviceId, "full-trust-device");

    const runtimes = JSON.parse(textResult(await rpc(base, 7, "local_exec.runtimes", { refresh: true })));
    if (runtimes.python.available) {
      const pythonExecution = await rpc(base, 8, "local_exec.execute", {
        runtime: "python",
        scriptPath: pythonScript,
        arguments: ["direct"],
        timeoutSeconds: 30,
      });
      const pythonResult = JSON.parse(textResult(pythonExecution));
      assert.equal(pythonResult.status, "completed");
      assert.match(pythonResult.stdout, /FULLPY:direct/);
    }

    const finalHealth = await fetch(`${base}/health`).then((response) => response.json());
    assert.equal(finalHealth.pendingRequests, 0);
    assert.equal(finalHealth.pendingExecutions, 0);
    const accessAudit = await fsp.readFile(path.join(logs, "access-audit.jsonl"), "utf8");
    const executionAudit = await fsp.readFile(path.join(logs, "execution-audit.jsonl"), "utf8");
    assert.ok(accessAudit.includes('"action":"full_trust_access_authorized"'));
    assert.ok(executionAudit.includes('"action":"full_trust_execution_authorized"'));
    assert.ok(executionAudit.includes('"action":"execution_finished"'));
  } finally {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve)).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("full-trust tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
