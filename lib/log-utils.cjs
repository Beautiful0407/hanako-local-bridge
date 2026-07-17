const fsp = require("fs/promises");
const path = require("path");

async function rotateFile(file, options = {}) {
  const maxBytes = Math.max(64 * 1024, Number(options.maxBytes) || 10 * 1024 * 1024);
  const backups = Math.min(20, Math.max(1, Number(options.backups) || 5));
  const incomingBytes = Math.max(0, Number(options.incomingBytes) || 0);
  const stat = await fsp.stat(file).catch((err) => {
    if (err?.code === "ENOENT") return null;
    throw err;
  });
  if (!stat || stat.size + incomingBytes <= maxBytes) return false;

  await fsp.rm(`${file}.${backups}`, { force: true }).catch(() => {});
  for (let index = backups - 1; index >= 1; index -= 1) {
    await fsp.rename(`${file}.${index}`, `${file}.${index + 1}`).catch((err) => {
      if (err?.code !== "ENOENT") throw err;
    });
  }
  await fsp.rename(file, `${file}.1`).catch((err) => {
    if (err?.code !== "ENOENT") throw err;
  });
  return true;
}

async function appendLineRotating(file, line, options = {}) {
  const text = String(line);
  await fsp.mkdir(path.dirname(file), { recursive: true });
  await rotateFile(file, {
    ...options,
    incomingBytes: Buffer.byteLength(text, "utf8"),
  });
  await fsp.appendFile(file, text, "utf8");
}

async function trimFileTail(file, maxBytes) {
  const limit = Math.max(1, Number(maxBytes) || 1);
  const stat = await fsp.stat(file).catch((err) => {
    if (err?.code === "ENOENT") return null;
    throw err;
  });
  if (!stat || stat.size <= limit) return false;

  const handle = await fsp.open(file, "r");
  let tail;
  try {
    const buffer = Buffer.alloc(limit);
    const { bytesRead } = await handle.read(buffer, 0, limit, stat.size - limit);
    tail = buffer.subarray(0, bytesRead);
  } finally {
    await handle.close();
  }
  await fsp.writeFile(file, tail);
  return true;
}

module.exports = {
  appendLineRotating,
  rotateFile,
  trimFileTail,
};
