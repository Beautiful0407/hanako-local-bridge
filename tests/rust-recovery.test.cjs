const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const projectDir = path.resolve(__dirname, "..");
const bridgeExe = path.join(projectDir, "target", "debug", "hanako-bridge.exe");

function startBridge(configPath) {
  const output = { stdout: "", stderr: "" };
  const child = spawn(bridgeExe, [], {
    env: {
      ...process.env,
      HANA_LOCAL_BRIDGE_CONFIG: configPath,
      LOCAL_AGENT_TRUST_MODE: "full",
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
  return { child, output };
}

async function waitFor(url, instance) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if (instance.child.exitCode !== null) {
      throw new Error(`Rust bridge exited with code ${instance.child.exitCode}\n${instance.output.stderr}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return response.json();
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${url}\n${instance.output.stderr}`);
}

async function stopBridge(instance) {
  if (!instance || instance.child.exitCode !== null) return;
  instance.child.kill();
  await Promise.race([
    new Promise((resolve) => instance.child.once("exit", resolve)),
    new Promise((_, reject) => setTimeout(() => reject(new Error("bridge did not stop")), 10000)),
  ]);
}

async function rpc(base, token, id, name, arguments = {}) {
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

function textResult(response) {
  assert.ok(response.result, JSON.stringify(response));
  return JSON.parse(response.result.content[0].text);
}

async function run() {
  await fsp.access(bridgeExe);
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hanako-rust-recovery-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const configPath = path.join(temp, "config.json");
  const scriptPath = path.join(root, "long-job.ps1");
  const port = 38000 + Math.floor(Math.random() * 400);
  const base = `http://127.0.0.1:${port}`;
  await fsp.mkdir(root, { recursive: true });
  await fsp.mkdir(data, { recursive: true });
  await fsp.mkdir(logs, { recursive: true });
  await fsp.writeFile(
    scriptPath,
    "Start-Sleep -Seconds 3\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\nWrite-Output 'RECOVERED_JOB'\n",
    "utf8",
  );
  await fsp.writeFile(
    configPath,
    `${JSON.stringify({
      schemaVersion: 1,
      device: { id: "rust-recovery-device", name: "Rust Recovery Device" },
      filesystem: {
        host: "127.0.0.1",
        port,
        approvalPort: port + 1,
        trustMode: "full",
        roots: [{ name: "RecoveryRoot", path: root, mode: "read_write" }],
      },
      storage: { dataDir: data, logDir: logs },
      cloud: { enabled: false, url: "wss://example.invalid/local-bridge/connect" },
    }, null, 2)}\n`,
    "utf8",
  );

  let first = startBridge(configPath);
  try {
    await waitFor(`${base}/health`, first);
    const token = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();
    const request = textResult(
      await rpc(base, token, 1, "local_exec.request_run", {
        runtime: "powershell",
        scriptPath,
        arguments: [],
        timeoutSeconds: 30,
      }),
    );
    const started = textResult(
      await rpc(base, token, 2, "local_exec.run", {
        authorizationId: request.authorization.id,
      }),
    );
    assert.ok(["starting", "running"].includes(started.status), JSON.stringify(started));

    await new Promise((resolve) => setTimeout(resolve, 300));
    await stopBridge(first);
    first = null;

    const second = startBridge(configPath);
    try {
      await waitFor(`${base}/health`, second);
      const deadline = Date.now() + 15000;
      let job;
      while (Date.now() < deadline) {
        const status = textResult(
          await rpc(base, token, 3, "local_exec.job_status", { jobId: started.id }),
        );
        job = status;
        if (!["starting", "running"].includes(status.status)) break;
        await new Promise((resolve) => setTimeout(resolve, 200));
      }
      assert.equal(job.status, "completed", JSON.stringify(job));
      assert.equal(job.recovered, true, JSON.stringify(job));
      const output = textResult(
        await rpc(base, token, 4, "local_exec.job_output", { jobId: started.id }),
      );
      assert.match(output.stdout, /RECOVERED_JOB/);
    } finally {
      await stopBridge(second);
    }
  } finally {
    await stopBridge(first);
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("Rust recovery integration test passed");
}

run().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
