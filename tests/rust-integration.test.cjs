const assert = require("assert");
const crypto = require("crypto");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const projectDir = path.resolve(__dirname, "..");
const bridgeExe =
  process.env.HANAKO_RUST_BRIDGE_EXE ||
  path.join(projectDir, "target", "debug", "hanako-bridge.exe");
const expectedVersion = process.env.HANAKO_RUST_EXPECTED_VERSION || "2.0.0-alpha.5";

async function checkedFetch(label, url, options) {
  try {
    return await fetch(url, options);
  } catch (error) {
    throw new Error(`${label} fetch failed: ${error.stack || error}`);
  }
}

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

async function rpc(base, token, id, name, args = {}) {
  const response = await checkedFetch(`RPC ${name}`, `${base}/mcp`, {
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

async function waitJob(base, token, jobId) {
  const deadline = Date.now() + 25000;
  while (Date.now() < deadline) {
    const response = await rpc(base, token, 900, "local_exec.job_status", { jobId });
    const job = JSON.parse(textResult(response));
    if (!["starting", "running"].includes(job.status)) return job;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for Rust job ${jobId}`);
}

async function run() {
  await fsp.access(bridgeExe);
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hanako-rust-integration-"));
  const root = path.join(temp, "root");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  const configPath = path.join(temp, "config.json");
  await fsp.mkdir(root, { recursive: true });
  const powershellScript = path.join(root, "echo.ps1");
  await fsp.writeFile(
    powershellScript,
    'param([string]$Name)\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\nWrite-Output "RUST:$Name"\n',
    "utf8",
  );
  const imageBytes = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zl1sAAAAASUVORK5CYII=",
    "base64",
  );
  await fsp.writeFile(path.join(root, "pixel.png"), imageBytes);
  const port = 36500 + Math.floor(Math.random() * 1000);
  const approvalPort = port + 1;
  await fsp.writeFile(
    configPath,
    `${JSON.stringify(
      {
        schemaVersion: 1,
        device: { id: "rust-test-device", name: "Rust Test Device" },
        filesystem: {
          host: "127.0.0.1",
          port,
          approvalPort,
          trustMode: "full",
          allowChatAuthorization: false,
          chatGrantMinutes: 120,
          roots: [{ name: "TestRoot", path: root, mode: "read_write" }],
        },
        storage: { dataDir: data, logDir: logs },
        cloud: {
          enabled: false,
          url: "wss://154-201-69-202.sslip.io/local-bridge/connect",
          reconnectMinSeconds: 3,
          reconnectMaxSeconds: 60,
          heartbeatSeconds: 25,
        },
      },
      null,
      2,
    )}\n`,
    "utf8",
  );

  const output = { stdout: "", stderr: "" };
  const child = spawn(bridgeExe, [], {
    env: {
      ...process.env,
      HANA_LOCAL_BRIDGE_CONFIG: configPath,
      LOCAL_AGENT_DEVICE_ID: "rust-test-device",
      LOCAL_AGENT_DEVICE_NAME: "Rust Test Device",
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
  child.on("exit", (code) => {
    if (code !== null && code !== 0) {
      console.error(`Rust bridge exited with code ${code}`);
      console.error(output.stdout);
      console.error(output.stderr);
    }
  });

  const base = `http://127.0.0.1:${port}`;
  try {
    const health = await waitFor(`${base}/health`, child, output);
    assert.equal(health.ok, true);
    assert.equal(health.version, expectedVersion);
    assert.equal(health.device.id, "rust-test-device");
    assert.equal(health.trustMode, "full");
    assert.equal(health.capabilities.write, true);
    assert.equal(health.capabilities.asynchronousExecution, true);
    const token = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();

    const unauthorized = await checkedFetch("unauthorized MCP", `${base}/mcp`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    });
    assert.equal(unauthorized.status, 401);

    const toolsResponse = await checkedFetch("tools/list", `${base}/mcp`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }),
    }).then((response) => response.json());
    const tools = new Map(toolsResponse.result.tools.map((tool) => [tool.name, tool]));
    assert.equal(tools.size, 31);
    for (const name of [
      "local_fs.read_image",
      "local_fs.apply_patch",
      "local_fs.watch_events",
      "local_exec.execute",
      "local_exec.cancel_job",
    ]) {
      assert.ok(tools.has(name), name);
    }
    assert.match(tools.get("local_exec.execute").description, /without a quote or approval/);
    assert.equal(
      tools.get("local_exec.execute").inputSchema.properties.userAuthorizationQuote,
      undefined,
    );

    const write = await rpc(base, token, 3, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "hello",
    });
    const written = JSON.parse(textResult(write));
    assert.equal(await fsp.readFile(path.join(root, "hello.txt"), "utf8"), "hello");
    assert.equal(written.size, 5);
    assert.equal(written.sha256, crypto.createHash("sha256").update("hello").digest("hex"));

    const overwriteRejected = await rpc(base, token, 4, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "changed",
      overwrite: true,
    });
    assert.equal(overwriteRejected.error.data.code, "expected_sha256_required");
    const overwritten = await rpc(base, token, 5, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "changed",
      overwrite: true,
      expectedSha256: written.sha256,
    });
    assert.ok(overwritten.result, JSON.stringify(overwritten));

    const utf16 = JSON.parse(
      textResult(
        await rpc(base, token, 6, "local_fs.write_text", {
          path: "local://TestRoot/utf16.txt",
          text: "alpha\r\nbeta\r\n",
          encoding: "utf16le",
          bom: true,
        }),
      ),
    );
    assert.equal(utf16.encoding, "utf16le");
    const lines = JSON.parse(
      textResult(
        await rpc(base, token, 7, "local_fs.read_lines", {
          path: "local://TestRoot/utf16.txt",
          startLine: 2,
          lineCount: 1,
        }),
      ),
    );
    assert.deepEqual(lines.lines, [{ number: 2, text: "beta" }]);

    const image = await rpc(base, token, 8, "local_fs.read_image", {
      path: "local://TestRoot/pixel.png",
    });
    const imageBlock = image.result.content.find((block) => block.type === "image");
    assert.equal(imageBlock.mimeType, "image/png");
    assert.equal(imageBlock.data, imageBytes.toString("base64"));

    const search = JSON.parse(
      textResult(
        await rpc(base, token, 9, "local_fs.search", {
          path: "local://TestRoot",
          glob: "*.txt",
          limit: 20,
        }),
      ),
    );
    assert.ok(search.results.some((entry) => entry.name === "hello.txt"));

    const watch = JSON.parse(
      textResult(
        await rpc(base, token, 10, "local_fs.watch", {
          path: "local://TestRoot",
          recursive: false,
        }),
      ),
    );
    await fsp.writeFile(path.join(root, "watch-created.txt"), "watch", "utf8");
    const events = JSON.parse(
      textResult(
        await rpc(base, token, 11, "local_fs.watch_events", {
          watchId: watch.watchId,
          afterSequence: 0,
          waitMs: 3000,
        }),
      ),
    );
    assert.ok(events.events.some((event) => event.relativePath === "watch-created.txt"));
    await rpc(base, token, 12, "local_fs.unwatch", { watchId: watch.watchId });

    const execution = JSON.parse(
      textResult(
        await rpc(base, token, 13, "local_exec.execute", {
          runtime: "powershell",
          scriptPath: powershellScript,
          arguments: ["integration"],
          timeoutSeconds: 30,
        }),
      ),
    );
    assert.equal(execution.status, "completed", JSON.stringify(execution));
    assert.equal(execution.job.exitCode, 0);
    assert.match(execution.stdout, /RUST:integration/);

    const request = JSON.parse(
      textResult(
        await rpc(base, token, 14, "local_exec.request_run", {
          runtime: "powershell",
          scriptPath: powershellScript,
          arguments: ["async"],
          timeoutSeconds: 30,
        }),
      ),
    );
    const started = JSON.parse(
      textResult(
        await rpc(base, token, 15, "local_exec.run", {
          authorizationId: request.authorization.id,
        }),
      ),
    );
    assert.equal((await waitJob(base, token, started.id)).status, "completed");

    const identityPreflight = await checkedFetch(
      "client identity preflight",
      `http://127.0.0.1:${approvalPort}/api/client-identity`,
      {
        method: "OPTIONS",
        headers: {
          Origin: "https://154-201-69-202.sslip.io",
          "Access-Control-Request-Private-Network": "true",
        },
      },
    );
    assert.equal(identityPreflight.status, 200);
    assert.equal(
      identityPreflight.headers.get("access-control-allow-private-network"),
      "true",
    );

    const approvalHealth = await checkedFetch(
      "approval health",
      `http://127.0.0.1:${approvalPort}/health`,
    ).then((response) => response.json());
    assert.equal(approvalHealth.ok, true);
    assert.equal(approvalHealth.runtime, "rust");
    assert.equal(approvalHealth.version, expectedVersion);

    const managerPage = await checkedFetch(
      "manager page",
      `http://127.0.0.1:${approvalPort}/manager/`,
    );
    assert.equal(managerPage.status, 200);
    assert.match(await managerPage.text(), /Hanako Local Bridge/);

    const favicon = await checkedFetch(
      "manager favicon",
      `http://127.0.0.1:${approvalPort}/favicon.ico`,
    );
    assert.equal(favicon.status, 204);
  } finally {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve)).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("Rust integration tests passed");
}

run().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
