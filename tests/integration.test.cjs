const assert = require("assert");
const fs = require("fs");
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

function imageResult(response) {
  assert.ok(response.result, JSON.stringify(response));
  return response.result.content.find((block) => block.type === "image");
}

async function waitJob(base, jobId) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const response = await rpc(base, 900, "local_exec.job_status", { jobId });
    const job = JSON.parse(textResult(response));
    if (!["starting", "running"].includes(job.status)) return job;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for local execution job ${jobId}`);
}

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-local-fs-test-"));
  const root = path.join(temp, "root");
  const extra = path.join(temp, "extra");
  const chatExtra = path.join(temp, "chat-extra");
  const data = path.join(temp, "data");
  const logs = path.join(temp, "logs");
  await fsp.mkdir(root, { recursive: true });
  await fsp.mkdir(extra, { recursive: true });
  await fsp.mkdir(chatExtra, { recursive: true });
  const powershellScript = path.join(root, "echo.ps1");
  const pythonScript = path.join(root, "echo.py");
  const changingScript = path.join(root, "changing.ps1");
  await fsp.writeFile(
    powershellScript,
    'param([string]$Name)\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new()\nWrite-Output "PS:$Name"\n',
    "utf8",
  );
  await fsp.writeFile(pythonScript, 'import sys\nprint("PY:" + sys.argv[1])\n', "utf8");
  await fsp.writeFile(changingScript, 'Write-Output "before"\n', "utf8");
  const imageFile = path.join(root, "pixel.png");
  const imageBytes = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zl1sAAAAASUVORK5CYII=",
    "base64",
  );
  await fsp.writeFile(imageFile, imageBytes);
  await fsp.writeFile(path.join(root, "fake.png"), "not an image", "utf8");

  const mcpPort = 22000 + Math.floor(Math.random() * 10000);
  const approvalPort = mcpPort + 1;
  const env = {
    ...process.env,
    LOCAL_AGENT_TRUST_MODE: "approval",
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
    LOCAL_AGENT_DEVICE_ID: "test-device",
    LOCAL_AGENT_DEVICE_NAME: "Integration Test Device",
    LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION: "1",
    LOCAL_FS_MCP_CHAT_GRANT_MINUTES: "120",
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
  const approvalBase = `http://127.0.0.1:${approvalPort}`;
  try {
    const health = await waitFor(`${base}/health`, child);
    assert.equal(health.version, packageVersion);
    assert.equal(health.device.id, "test-device");
    assert.equal(health.capabilities.write, true);
    assert.equal(health.capabilities.imageRead, true);
    assert.equal(health.capabilities.asynchronousExecution, true);
    const identityPreflight = await fetch(`${approvalBase}/api/client-identity`, {
      method: "OPTIONS",
      headers: {
        Origin: "http://154.201.69.202",
        "Access-Control-Request-Private-Network": "true",
      },
    });
    assert.equal(identityPreflight.status, 200);
    assert.equal(identityPreflight.headers.get("access-control-allow-origin"), "http://154.201.69.202");
    assert.equal(identityPreflight.headers.get("access-control-allow-private-network"), "true");
    const clientIdentity = await fetch(`${approvalBase}/api/client-identity`, {
      headers: { Origin: "http://154.201.69.202" },
    }).then((response) => response.json());
    assert.equal(clientIdentity.device.id, "test-device");
    assert.equal(clientIdentity.device.name, "Integration Test Device");
    const rejectedIdentity = await fetch(`${approvalBase}/api/client-identity`, {
      headers: { Origin: "http://untrusted.example" },
    });
    assert.equal(rejectedIdentity.status, 403);
    const token = (await fsp.readFile(path.join(data, "approval-token.txt"), "utf8")).trim();

    const toolsResponse = await fetch(`${base}/mcp`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }),
    }).then((response) => response.json());
    const toolNames = toolsResponse.result.tools.map((tool) => tool.name);
    assert.ok(toolNames.includes("local_fs.request_access"));
    assert.ok(toolNames.includes("local_fs.write_text"));
    assert.ok(toolNames.includes("local_fs.read_lines"));
    assert.ok(toolNames.includes("local_fs.read_image"));
    assert.ok(toolNames.includes("local_fs.append_text"));
    assert.ok(toolNames.includes("local_fs.apply_patch"));
    assert.ok(toolNames.includes("local_fs.watch"));
    assert.ok(toolNames.includes("local_fs.watch_events"));
    assert.ok(toolNames.includes("local_fs.unwatch"));
    assert.ok(toolNames.includes("local_fs.delete_to_trash"));
    assert.ok(toolNames.includes("local_exec.request_run"));
    assert.ok(toolNames.includes("local_exec.execute"));
    assert.ok(toolNames.includes("local_exec.run"));
    assert.ok(toolNames.includes("local_exec.job_output"));
    const requestAccessTool = toolsResponse.result.tools.find((tool) => tool.name === "local_fs.request_access");
    const executeTool = toolsResponse.result.tools.find((tool) => tool.name === "local_exec.execute");
    assert.ok(requestAccessTool.inputSchema.properties.userAuthorizationQuote);
    assert.ok(executeTool.inputSchema.properties.userAuthorizationQuote);

    const imageResponse = await rpc(base, 114, "local_fs.read_image", {
      path: "local://TestRoot/pixel.png",
    });
    const image = imageResult(imageResponse);
    assert.equal(image.mimeType, "image/png");
    assert.equal(image.data, imageBytes.toString("base64"));
    const imageMetadata = JSON.parse(textResult(imageResponse));
    assert.equal(imageMetadata.name, "pixel.png");
    assert.equal(imageMetadata.size, imageBytes.length);

    const fakeImage = await rpc(base, 115, "local_fs.read_image", {
      path: "local://TestRoot/fake.png",
    });
    assert.equal(fakeImage.error.data.code, "unsupported_image_format");

    const runtimesResponse = await rpc(base, 100, "local_exec.runtimes", { refresh: true });
    const runtimes = JSON.parse(textResult(runtimesResponse));
    assert.equal(runtimes.powershell.available, true);

    const powershellRequest = await rpc(base, 101, "local_exec.request_run", {
      runtime: "powershell",
      scriptPath: powershellScript,
      arguments: ["cloud-test"],
      timeoutSeconds: 30,
      reason: "integration test",
      userAuthorizationQuote: `I authorize you to execute ${powershellScript} with argument cloud-test`,
    });
    const powershellAuthorization = JSON.parse(textResult(powershellRequest)).authorization;
    assert.equal(powershellAuthorization.source, "chat_authorization");
    assert.equal(powershellAuthorization.usesRemaining, 1);

    const powershellRun = await rpc(base, 102, "local_exec.run", {
      authorizationId: powershellAuthorization.id,
    });
    const powershellJob = JSON.parse(textResult(powershellRun));
    const powershellFinished = await waitJob(base, powershellJob.id);
    assert.equal(powershellFinished.status, "completed");
    assert.equal(powershellFinished.exitCode, 0);
    const powershellOutput = await rpc(base, 103, "local_exec.job_output", {
      jobId: powershellJob.id,
    });
    assert.match(JSON.parse(textResult(powershellOutput)).stdout, /PS:cloud-test/);

    const oneStepExecution = await rpc(base, 113, "local_exec.execute", {
      runtime: "powershell",
      scriptPath: powershellScript,
      arguments: ["one-step"],
      timeoutSeconds: 30,
      reason: "one-step integration test",
      userAuthorizationQuote: `I authorize you to execute ${powershellScript} with argument one-step`,
    });
    const oneStepResult = JSON.parse(textResult(oneStepExecution));
    assert.equal(oneStepResult.status, "completed");
    assert.equal(oneStepResult.job.exitCode, 0);
    assert.match(oneStepResult.stdout, /PS:one-step/);

    const localExecutionRequest = await rpc(base, 104, "local_exec.request_run", {
      runtime: "powershell",
      scriptPath: powershellScript,
      arguments: ["approval-test"],
      timeoutSeconds: 30,
      reason: "local approval integration test",
    });
    const pendingExecution = JSON.parse(textResult(localExecutionRequest)).request;
    assert.equal(pendingExecution.status, "pending");
    const approvedExecution = await fetch(
      `${approvalBase}/api/execution/requests/${pendingExecution.id}/approve`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-approval-token": token,
        },
        body: JSON.stringify({ scope: "once" }),
      },
    );
    assert.equal(approvedExecution.status, 200, await approvedExecution.text());
    const executionStatus = await rpc(base, 105, "local_exec.request_status", {
      requestId: pendingExecution.id,
    });
    const approvedExecutionStatus = JSON.parse(textResult(executionStatus));
    assert.equal(approvedExecutionStatus.request.status, "approved");
    assert.ok(approvedExecutionStatus.authorization.id);
    const approvedRun = await rpc(base, 106, "local_exec.run", {
      authorizationId: approvedExecutionStatus.authorization.id,
    });
    const approvedJob = JSON.parse(textResult(approvedRun));
    assert.equal((await waitJob(base, approvedJob.id)).status, "completed");
    const approvedOutput = await rpc(base, 107, "local_exec.job_output", { jobId: approvedJob.id });
    assert.match(JSON.parse(textResult(approvedOutput)).stdout, /PS:approval-test/);

    const changingRequest = await rpc(base, 108, "local_exec.request_run", {
      runtime: "powershell",
      scriptPath: changingScript,
      timeoutSeconds: 30,
      userAuthorizationQuote: `I authorize you to execute ${changingScript}`,
    });
    const changingAuthorization = JSON.parse(textResult(changingRequest)).authorization;
    await fsp.writeFile(changingScript, 'Write-Output "after"\n', "utf8");
    const changedRun = await rpc(base, 109, "local_exec.run", {
      authorizationId: changingAuthorization.id,
    });
    assert.equal(changedRun.error.data.code, "script_sha256_mismatch");

    if (runtimes.python.available) {
      const pythonRequest = await rpc(base, 110, "local_exec.request_run", {
        runtime: "python",
        scriptPath: pythonScript,
        arguments: ["cloud-python"],
        timeoutSeconds: 30,
        userAuthorizationQuote: `I authorize you to execute ${pythonScript} with argument cloud-python`,
      });
      const pythonAuthorization = JSON.parse(textResult(pythonRequest)).authorization;
      const pythonRun = await rpc(base, 111, "local_exec.run", {
        authorizationId: pythonAuthorization.id,
      });
      const pythonJob = JSON.parse(textResult(pythonRun));
      assert.equal((await waitJob(base, pythonJob.id)).status, "completed");
      const pythonOutput = await rpc(base, 112, "local_exec.job_output", { jobId: pythonJob.id });
      assert.match(JSON.parse(textResult(pythonOutput)).stdout, /PY:cloud-python/);
    }

    const write = await rpc(base, 2, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "hello",
    });
    const writeStat = JSON.parse(textResult(write));
    assert.equal(writeStat.size, 5);
    assert.equal(await fsp.readFile(path.join(root, "hello.txt"), "utf8"), "hello");

    const deviceRoot = `device://test-device/${root.replace(/\\/g, "/")}`;
    const deviceWrite = await rpc(base, 201, "local_fs.write_text", {
      path: `${deviceRoot}/device-path.txt`,
      text: "device path",
    });
    assert.ok(deviceWrite.result, JSON.stringify(deviceWrite));
    assert.equal(await fsp.readFile(path.join(root, "device-path.txt"), "utf8"), "device path");
    const wrongDevice = await rpc(base, 202, "local_fs.read_text", {
      path: `device://another-device/${root.replace(/\\/g, "/")}/device-path.txt`,
    });
    assert.equal(wrongDevice.error.data.code, "wrong_device");

    const overwriteRejected = await rpc(base, 3, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "changed",
      overwrite: true,
    });
    assert.equal(overwriteRejected.error.data.code, "expected_sha256_required");

    const overwritten = await rpc(base, 4, "local_fs.write_text", {
      path: "local://TestRoot/hello.txt",
      text: "changed",
      overwrite: true,
      expectedSha256: writeStat.sha256,
    });
    const overwrittenStat = JSON.parse(textResult(overwritten));
    assert.equal(await fsp.readFile(path.join(root, "hello.txt"), "utf8"), "changed");
    assert.notEqual(overwrittenStat.sha256, writeStat.sha256);

    const racePath = "local://TestRoot/concurrent.txt";
    const raceSeed = JSON.parse(
      textResult(
        await rpc(base, 401, "local_fs.write_text", {
          path: racePath,
          text: "seed",
        }),
      ),
    );
    const raceResponses = await Promise.all(
      Array.from({ length: 8 }, (_, index) =>
        rpc(base, 410 + index, "local_fs.write_text", {
          path: racePath,
          text: `winner-${index}`,
          overwrite: true,
          expectedSha256: raceSeed.sha256,
        }),
      ),
    );
    const raceSuccesses = raceResponses.filter((response) => response.result);
    const raceFailures = raceResponses.filter((response) => response.error);
    assert.equal(raceSuccesses.length, 1);
    assert.equal(raceFailures.length, 7);
    for (const response of raceFailures) assert.equal(response.error.data.code, "sha256_mismatch");
    assert.match(await fsp.readFile(path.join(root, "concurrent.txt"), "utf8"), /^winner-\d$/);

    const createRacePath = "local://TestRoot/concurrent-create.txt";
    const createRaceResponses = await Promise.all(
      Array.from({ length: 6 }, (_, index) =>
        rpc(base, 430 + index, "local_fs.write_text", {
          path: createRacePath,
          text: `creator-${index}`,
        }),
      ),
    );
    assert.equal(createRaceResponses.filter((response) => response.result).length, 1);
    const createRaceFailures = createRaceResponses.filter((response) => response.error);
    assert.equal(createRaceFailures.length, 5);
    for (const response of createRaceFailures) assert.equal(response.error.data.code, "overwrite_required");

    const utf16Path = "local://TestRoot/utf16.txt";
    const utf16Write = JSON.parse(
      textResult(
        await rpc(base, 450, "local_fs.write_text", {
          path: utf16Path,
          text: "alpha\r\nbeta\r\n",
          encoding: "utf16le",
          bom: true,
        }),
      ),
    );
    assert.equal(utf16Write.encoding, "utf16le");
    assert.equal(utf16Write.bom, true);
    const utf16Raw = await fsp.readFile(path.join(root, "utf16.txt"));
    assert.deepEqual([...utf16Raw.subarray(0, 2)], [0xff, 0xfe]);
    assert.equal(textResult(await rpc(base, 451, "local_fs.read_text", { path: utf16Path })), "alpha\r\nbeta\r\n");

    const lineResult = JSON.parse(
      textResult(
        await rpc(base, 452, "local_fs.read_lines", {
          path: utf16Path,
          startLine: 2,
          lineCount: 1,
        }),
      ),
    );
    assert.equal(lineResult.encoding, "utf16le");
    assert.equal(lineResult.bom, true);
    assert.equal(lineResult.newline, "crlf");
    assert.equal(lineResult.totalLines, 2);
    assert.deepEqual(lineResult.lines, [{ number: 2, text: "beta" }]);

    const appended = JSON.parse(
      textResult(
        await rpc(base, 453, "local_fs.append_text", {
          path: utf16Path,
          text: "gamma\r\n",
        }),
      ),
    );
    assert.equal(appended.encoding, "utf16le");
    assert.equal(appended.bom, true);
    assert.equal(
      textResult(await rpc(base, 454, "local_fs.read_text", { path: utf16Path })),
      "alpha\r\nbeta\r\ngamma\r\n",
    );

    const patched = JSON.parse(
      textResult(
        await rpc(base, 455, "local_fs.apply_patch", {
          path: utf16Path,
          expectedSha256: appended.sha256,
          edits: [{ oldText: "beta", newText: "BETA", expectedOccurrences: 1 }],
        }),
      ),
    );
    assert.equal(patched.replacements, 1);
    assert.equal(patched.encoding, "utf16le");
    assert.equal(
      textResult(await rpc(base, 456, "local_fs.read_text", { path: utf16Path })),
      "alpha\r\nBETA\r\ngamma\r\n",
    );

    const patchMismatch = await rpc(base, 457, "local_fs.apply_patch", {
      path: utf16Path,
      expectedSha256: patched.sha256,
      edits: [{ oldText: "alpha", newText: "ALPHA", expectedOccurrences: 2 }],
    });
    assert.equal(patchMismatch.error.data.code, "patch_context_mismatch");

    const appendRacePath = "local://TestRoot/append-race.txt";
    const appendRaceResponses = await Promise.all(
      Array.from({ length: 8 }, (_, index) =>
        rpc(base, 460 + index, "local_fs.append_text", {
          path: appendRacePath,
          text: `[${index}]`,
        }),
      ),
    );
    assert.equal(appendRaceResponses.filter((response) => response.result).length, 8);
    const appendRaceText = await fsp.readFile(path.join(root, "append-race.txt"), "utf8");
    for (let index = 0; index < 8; index += 1) assert.ok(appendRaceText.includes(`[${index}]`));

    const pagedDir = path.join(root, "paged");
    await fsp.mkdir(pagedDir, { recursive: true });
    for (let index = 0; index < 5; index += 1) {
      await fsp.writeFile(path.join(pagedDir, `item-${index}.txt`), String(index), "utf8");
    }
    const pageOne = JSON.parse(
      textResult(
        await rpc(base, 480, "local_fs.list", {
          path: "local://TestRoot/paged",
          limit: 2,
        }),
      ),
    );
    assert.equal(pageOne.totalEntries, 5);
    assert.equal(pageOne.entries.length, 2);
    assert.ok(pageOne.nextCursor);
    const pageTwo = JSON.parse(
      textResult(
        await rpc(base, 481, "local_fs.list", {
          path: "local://TestRoot/paged",
          limit: 2,
          cursor: pageOne.nextCursor,
        }),
      ),
    );
    assert.equal(pageTwo.offset, 2);
    assert.equal(pageTwo.entries.length, 2);
    assert.equal(new Set([...pageOne.entries, ...pageTwo.entries].map((entry) => entry.name)).size, 4);
    const invalidCursor = await rpc(base, 482, "local_fs.list", {
      path: "local://TestRoot/paged",
      cursor: "not-a-cursor",
    });
    assert.equal(invalidCursor.error.data.code, "invalid_cursor");

    const searchDir = path.join(root, "search-area");
    await fsp.mkdir(path.join(searchDir, "deep"), { recursive: true });
    await fsp.mkdir(path.join(searchDir, "skip"), { recursive: true });
    await fsp.writeFile(path.join(searchDir, "keep.txt"), "keep", "utf8");
    await fsp.writeFile(path.join(searchDir, "note.md"), "note", "utf8");
    await fsp.writeFile(path.join(searchDir, "deep", "nested.txt"), "nested", "utf8");
    await fsp.writeFile(path.join(searchDir, "skip", "hidden.txt"), "hidden", "utf8");
    const searchResult = JSON.parse(
      textResult(
        await rpc(base, 483, "local_fs.search", {
          path: "local://TestRoot/search-area",
          glob: "**/*.txt",
          exclude: ["skip/**"],
          limit: 20,
          maxVisited: 100,
          timeoutMs: 5000,
        }),
      ),
    );
    const searchNames = searchResult.results.map((entry) => entry.name);
    assert.ok(searchNames.includes("keep.txt"));
    assert.ok(searchNames.includes("nested.txt"));
    assert.ok(!searchNames.includes("hidden.txt"));
    assert.equal(searchResult.truncated, false);
    const budgetedSearch = JSON.parse(
      textResult(
        await rpc(base, 484, "local_fs.search", {
          path: "local://TestRoot/search-area",
          glob: "**/*",
          limit: 20,
          maxVisited: 1,
          timeoutMs: 5000,
        }),
      ),
    );
    assert.equal(budgetedSearch.truncated, true);
    assert.ok(budgetedSearch.truncationReasons.includes("visit_budget"));

    const watch = JSON.parse(
      textResult(
        await rpc(base, 485, "local_fs.watch", {
          path: "local://TestRoot",
          debounceMs: 0,
        }),
      ),
    );
    await rpc(base, 486, "local_fs.write_text", {
      path: "local://TestRoot/watch-created.txt",
      text: "watch me",
    });
    const watchEvents = JSON.parse(
      textResult(
        await rpc(base, 487, "local_fs.watch_events", {
          watchId: watch.watchId,
          afterSequence: 0,
          waitMs: 5000,
        }),
      ),
    );
    assert.ok(watchEvents.events.some((event) => event.relativePath === "watch-created.txt"));
    const unwatch = JSON.parse(
      textResult(await rpc(base, 488, "local_fs.unwatch", { watchId: watch.watchId })),
    );
    assert.equal(unwatch.closed, true);

    await rpc(base, 5, "local_fs.mkdir", {
      path: "local://TestRoot/nested/folder",
      recursive: true,
    });
    assert.equal(fs.statSync(path.join(root, "nested", "folder")).isDirectory(), true);

    await rpc(base, 6, "local_fs.copy", {
      source: "local://TestRoot/hello.txt",
      destination: "local://TestRoot/nested/copied.txt",
    });
    await rpc(base, 7, "local_fs.move", {
      source: "local://TestRoot/nested/copied.txt",
      destination: "local://TestRoot/nested/moved.txt",
    });
    assert.equal(await fsp.readFile(path.join(root, "nested", "moved.txt"), "utf8"), "changed");

    const bridgeWrite = await rpc(base, 8, "local_fs.write_text", {
      path: "local://Bridge/should-not-exist.txt",
      text: "blocked",
    });
    assert.ok(["write_not_authorized", "bridge_program_read_only"].includes(bridgeWrite.error.data.code));

    const traversal = await rpc(base, 9, "local_fs.read_text", {
      path: "local://TestRoot/../outside.txt",
    });
    assert.equal(traversal.error.data.code, "path_not_authorized");

    const requestResponse = await rpc(base, 10, "local_fs.request_access", {
      path: extra,
      mode: "read_write",
      name: "Extra",
      reason: "integration test",
    });
    const request = JSON.parse(textResult(requestResponse)).request;
    assert.equal(request.status, "pending");

    const approved = await fetch(`${approvalBase}/api/requests/${request.id}/approve`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-approval-token": token,
      },
      body: JSON.stringify({ mode: "read_write", name: "Extra" }),
    });
    assert.equal(approved.status, 200, await approved.text());

    const extraWrite = await rpc(base, 11, "local_fs.write_text", {
      path: "local://Extra/from-cloud.txt",
      text: "approved",
    });
    textResult(extraWrite);
    assert.equal(await fsp.readFile(path.join(extra, "from-cloud.txt"), "utf8"), "approved");

    const missingExactPath = await rpc(base, 12, "local_fs.request_access", {
      path: chatExtra,
      mode: "read",
      name: "BadChatGrant",
      userAuthorizationQuote: "我授权你访问这个文件夹",
    });
    assert.equal(missingExactPath.error.data.code, "authorization_path_not_confirmed");

    const chatAuthorization = await rpc(base, 13, "local_fs.request_access", {
      path: chatExtra,
      mode: "read_write",
      name: "ChatExtra",
      reason: "integration test",
      userAuthorizationQuote: `我授权你读写 ${chatExtra}`,
    });
    const chatGrant = JSON.parse(textResult(chatAuthorization)).grant;
    assert.equal(chatGrant.source, "chat_authorization");
    assert.ok(chatGrant.expiresAt);

    const chatWrite = await rpc(base, 14, "local_fs.write_text", {
      path: "local://ChatExtra/chat-authorized.txt",
      text: "automatic",
    });
    textResult(chatWrite);
    assert.equal(await fsp.readFile(path.join(chatExtra, "chat-authorized.txt"), "utf8"), "automatic");

    const trash = await rpc(base, 15, "local_fs.delete_to_trash", {
      path: "local://TestRoot/nested/moved.txt",
    });
    const trashResult = JSON.parse(textResult(trash));
    assert.equal(trashResult.recoverable, true);
    assert.equal(fs.existsSync(path.join(root, "nested", "moved.txt")), false);
    assert.equal(fs.existsSync(path.join(root, ".hana-trash", trashResult.trashName)), true);

    const audit = await fsp.readFile(path.join(logs, "access-audit.jsonl"), "utf8");
    assert.ok(audit.includes('"action":"write_text"'));
    assert.ok(audit.includes('"action":"access_approved"'));
    assert.ok(audit.includes('"action":"chat_access_authorized"'));
    const executionAudit = await fsp.readFile(path.join(logs, "execution-audit.jsonl"), "utf8");
    assert.ok(executionAudit.includes('"action":"execution_started"'));
    assert.ok(executionAudit.includes('"action":"execution_finished"'));
    assert.ok(executionAudit.includes('"action":"execution_approved"'));
  } finally {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve)).catch(() => {});
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("integration tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
