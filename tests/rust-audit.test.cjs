const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const projectDir = path.resolve(__dirname, "..");
const bridgeExe = path.join(projectDir, "target", "debug", "hanako-bridge.exe");

async function waitFor(url, child, output) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Rust bridge exited with code ${child.exitCode}\n${output.stderr}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return response.json();
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${url}\n${output.stderr}`);
}

async function callTool(base, token, id, name, arguments = {}) {
  const response = await fetch(`${base}/mcp`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: { name, arguments },
    }),
  });
  assert.equal(response.status, 200);
  return response.json();
}

async function run() {
  await fsp.access(bridgeExe);
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hanako-rust-audit-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const configPath = path.join(temp, "config.json");
  const port = 37500 + Math.floor(Math.random() * 500);
  await fsp.mkdir(root, { recursive: true });
  await fsp.mkdir(data, { recursive: true });
  await fsp.mkdir(logs, { recursive: true });
  await fsp.writeFile(
    configPath,
    `${JSON.stringify({
      schemaVersion: 1,
      device: { id: "rust-audit-device", name: "Rust Audit Device" },
      filesystem: {
        host: "127.0.0.1",
        port,
        approvalPort: port + 1,
        trustMode: "full",
        roots: [{ name: "AuditRoot", path: root, mode: "read_write" }],
      },
      storage: { dataDir: data, logDir: logs },
      cloud: { enabled: false, url: "wss://example.invalid/local-bridge/connect" },
    }, null, 2)}\n`,
    "utf8",
  );

  const output = { stdout: "", stderr: "" };
  const child = spawn(bridgeExe, ["--service"], {
    env: {
      ...process.env,
      HANA_LOCAL_BRIDGE_CONFIG: configPath,
      LOCAL_AGENT_DEVICE_ID: "rust-audit-device",
      LOCAL_AGENT_DEVICE_NAME: "Rust Audit Device",
    },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdout.on("data", (chunk) => {
    output.stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    output.stderr += chunk;
  });

  try {
    const health = await waitFor(`http://127.0.0.1:${port}/health`, child, output);
    assert.equal(health.ok, true);
    const token = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();

    const success = await callTool(
      `http://127.0.0.1:${port}`,
      token,
      1,
      "local_fs.mkdir",
      { path: "local://AuditRoot/new-directory" },
    );
    assert.ok(success.result, JSON.stringify(success));

    const failure = await callTool(
      `http://127.0.0.1:${port}`,
      token,
      2,
      "local_fs.read_text",
      { path: "local://AuditRoot/missing.txt" },
    );
    assert.ok(failure.error, JSON.stringify(failure));
    assert.match(JSON.stringify(failure), /not[_ ]found|ENOENT|missing/i);

    const auditPath = path.join(logs, "mcp-audit.jsonl");
    const deadline = Date.now() + 5000;
    let lines = [];
    while (Date.now() < deadline) {
      try {
        lines = (await fsp.readFile(auditPath, "utf8"))
          .trim()
          .split(/\r?\n/)
          .filter(Boolean)
          .map((line) => JSON.parse(line));
        if (lines.length >= 2) break;
      } catch {}
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.ok(lines.length >= 2, "Rust MCP audit log did not receive both calls");
    const successAudit = lines.find((entry) => entry.tool === "local_fs.mkdir");
    const failureAudit = lines.find((entry) => entry.tool === "local_fs.read_text");
    assert.equal(successAudit.ok, true);
    assert.equal(failureAudit.ok, false);
    assert.equal(failureAudit.code, "path_not_found");
    assert.equal(typeof successAudit.durationMs, "number");
    assert.equal(Object.hasOwn(successAudit, "arguments"), false);
  } finally {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve)).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("Rust audit integration test passed");
}

run().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
