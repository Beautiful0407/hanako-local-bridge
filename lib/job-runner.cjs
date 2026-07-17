const fs = require("fs");
const { spawn, spawnSync } = require("child_process");

const { loadJson, writeJsonAtomic } = require("./json-store.cjs");

function killProcessTree(pid) {
  if (!pid) return;
  spawnSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
    windowsHide: true,
    timeout: 10000,
    encoding: "utf8",
  });
}

async function main() {
  const specFile = process.argv[2];
  if (!specFile) throw new Error("job runner requires a spec file");
  const spec = loadJson(specFile, null);
  if (!spec) throw new Error("job runner spec is missing or invalid");

  const startedAt = new Date().toISOString();
  writeJsonAtomic(spec.stateFile, {
    schemaVersion: 1,
    jobId: spec.jobId,
    runnerPid: process.pid,
    childPid: null,
    startedAt,
  });

  const stdoutFd = fs.openSync(spec.stdoutFile, "a");
  const stderrFd = fs.openSync(spec.stderrFile, "a");
  let child;
  try {
    child = spawn(spec.command, spec.arguments, {
      cwd: spec.cwd,
      windowsHide: true,
      shell: false,
      stdio: ["ignore", stdoutFd, stderrFd],
      env: {
        ...process.env,
        ...(spec.environment || {}),
      },
    });
  } finally {
    fs.closeSync(stdoutFd);
    fs.closeSync(stderrFd);
  }

  writeJsonAtomic(spec.stateFile, {
    schemaVersion: 1,
    jobId: spec.jobId,
    runnerPid: process.pid,
    childPid: child.pid || null,
    startedAt,
  });

  let finished = false;
  let timedOut = false;
  let timer = null;

  const finish = (result) => {
    if (finished) return;
    finished = true;
    if (timer) clearTimeout(timer);
    writeJsonAtomic(spec.resultFile, {
      schemaVersion: 1,
      jobId: spec.jobId,
      runnerPid: process.pid,
      childPid: child.pid || null,
      startedAt,
      finishedAt: new Date().toISOString(),
      timedOut,
      cancelled: false,
      ...result,
    });
  };

  child.once("error", (err) => {
    finish({
      status: "failed",
      exitCode: null,
      signal: null,
      error: err.message || String(err),
    });
  });
  child.once("close", (code, signal) => {
    finish({
      status: timedOut ? "timed_out" : code === 0 ? "completed" : "failed",
      exitCode: code,
      signal,
      error: null,
    });
  });

  timer = setTimeout(() => {
    timedOut = true;
    killProcessTree(child.pid);
  }, Math.max(1, Number(spec.timeoutSeconds) || 300) * 1000);
}

main().catch((err) => {
  const specFile = process.argv[2];
  try {
    const spec = specFile ? loadJson(specFile, null) : null;
    if (spec?.resultFile) {
      writeJsonAtomic(spec.resultFile, {
        schemaVersion: 1,
        jobId: spec.jobId,
        runnerPid: process.pid,
        childPid: null,
        startedAt: null,
        finishedAt: new Date().toISOString(),
        status: "failed",
        exitCode: null,
        signal: null,
        error: err.message || String(err),
        timedOut: false,
        cancelled: false,
      });
    }
  } catch {}
  process.exitCode = 1;
});
