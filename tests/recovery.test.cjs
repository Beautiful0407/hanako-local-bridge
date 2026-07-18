const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const { appendLineRotating, trimFileTail } = require("../lib/log-utils.cjs");

const projectDir = path.resolve(__dirname, "..");
const serverFile = path.join(projectDir, "server.cjs");

async function waitForHealth(base, child, output) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`server exited early with code ${child.exitCode}\n${output.stderr}`);
    }
    try {
      const response = await fetch(`${base}/health`);
      if (response.ok) return response.json();
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${base}/health\n${output.stderr}`);
}

async function rpc(base, token, id, name, args = {}) {
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

async function startServer(env) {
  const output = { stdout: "", stderr: "" };
  const child = spawn(process.execPath, [serverFile], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (chunk) => {
    output.stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    output.stderr += chunk;
  });
  const base = `http://127.0.0.1:${env.LOCAL_FS_MCP_PORT}`;
  await waitForHealth(base, child, output);
  return { child, output, base };
}

async function stopServer(instance) {
  if (!instance || instance.child.exitCode !== null) return;
  instance.child.kill();
  await Promise.race([
    new Promise((resolve) => instance.child.once("exit", resolve)),
    new Promise((_, reject) => setTimeout(() => reject(new Error("server did not stop")), 10000)),
  ]);
}

async function waitForJob(base, token, jobId) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const response = await rpc(base, token, 900, "local_exec.job_status", { jobId });
    const job = JSON.parse(textResult(response));
    if (!["starting", "running"].includes(job.status)) return job;
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for recovered job ${jobId}`);
}

async function waitForFile(file) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    try {
      await fsp.access(file);
      return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${file}`);
}

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-recovery-test-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const script = path.join(root, "recover.ps1");
  await fsp.mkdir(root, { recursive: true });
  await fsp.writeFile(
    script,
    'param([string]$Name)\nStart-Sleep -Seconds 4\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\nWrite-Output "RECOVERED:$Name"\n',
    "utf8",
  );

  const mcpPort = 34000 + Math.floor(Math.random() * 1000);
  const env = {
    ...process.env,
    LOCAL_AGENT_TRUST_MODE: "full",
    LOCAL_FS_MCP_ROOT: root,
    LOCAL_FS_MCP_ROOT_NAME: "RecoveryRoot",
    LOCAL_FS_MCP_HOST: "127.0.0.1",
    LOCAL_FS_MCP_PORT: String(mcpPort),
    LOCAL_FS_MCP_APPROVAL_PORT: String(mcpPort + 1),
    LOCAL_FS_MCP_DATA_DIR: data,
    LOCAL_FS_MCP_LOG_DIR: logs,
    LOCAL_AGENT_DEVICE_ID: "recovery-device",
    LOCAL_AGENT_DEVICE_NAME: "Recovery Device",
    LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION: "0",
    HANA_CLOUD_BRIDGE_ENABLED: "0",
  };

  let server;
  try {
    server = await startServer(env);
    const token = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();
    const request = await rpc(server.base, token, 1, "local_exec.request_run", {
      runtime: "powershell",
      scriptPath: script,
      arguments: ["after-restart"],
      timeoutSeconds: 30,
    });
    const authorization = JSON.parse(textResult(request)).authorization;
    const started = JSON.parse(
      textResult(
        await rpc(server.base, token, 2, "local_exec.run", {
          authorizationId: authorization.id,
        }),
      ),
    );
    assert.equal(started.status, "running");
    assert.ok(Number(started.pid) > 0);

    const summaryFile = path.join(logs, "jobs", `${started.id}.json`);
    const runnerFile = path.join(logs, "jobs", `${started.id}.runner.json`);
    await Promise.all([waitForFile(summaryFile), waitForFile(runnerFile)]);
    await stopServer(server);
    server = null;

    await new Promise((resolve) => setTimeout(resolve, 500));
    server = await startServer(env);
    const recovered = await waitForJob(server.base, token, started.id);
    assert.equal(recovered.status, "completed");
    assert.equal(recovered.exitCode, 0);
    assert.equal(recovered.recovered, true);
    assert.equal(recovered.pid, started.pid);
    const output = JSON.parse(
      textResult(await rpc(server.base, token, 3, "local_exec.job_output", { jobId: started.id })),
    );
    assert.match(output.stdout, /RECOVERED:after-restart/);

    await stopServer(server);
    server = null;
    await fsp.writeFile(path.join(data, "access-control.json"), "{broken", "utf8");
    await fsp.writeFile(path.join(data, "execution-authorizations.json"), "{broken", "utf8");
    server = await startServer(env);
    const health = await fetch(`${server.base}/health`).then((response) => response.json());
    assert.equal(health.ok, true);
    const dataFiles = await fsp.readdir(data);
    assert.ok(dataFiles.some((file) => file.startsWith("access-control.json.corrupt-")));
    assert.ok(dataFiles.some((file) => file.startsWith("execution-authorizations.json.corrupt-")));

    const rotationFile = path.join(logs, "rotation-test.log");
    const line = `${"x".repeat(40 * 1024)}\n`;
    await appendLineRotating(rotationFile, line, { maxBytes: 64 * 1024, backups: 2 });
    await appendLineRotating(rotationFile, line, { maxBytes: 64 * 1024, backups: 2 });
    assert.equal((await fsp.stat(`${rotationFile}.1`)).isFile(), true);
    assert.equal((await fsp.stat(rotationFile)).isFile(), true);

    const tailFile = path.join(logs, "tail-test.log");
    await fsp.writeFile(tailFile, `prefix-${"y".repeat(100 * 1024)}-tail`, "utf8");
    assert.equal(await trimFileTail(tailFile, 64 * 1024), true);
    const tailContent = await fsp.readFile(tailFile, "utf8");
    assert.ok(Buffer.byteLength(tailContent, "utf8") <= 64 * 1024);
    assert.match(tailContent, /-tail$/);
  } finally {
    await stopServer(server).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("recovery tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
