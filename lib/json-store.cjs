const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

function timestampSuffix() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

function writeJsonFile(file, data) {
  const temp = `${file}.${process.pid}.${crypto.randomBytes(4).toString("hex")}.tmp`;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  try {
    fs.writeFileSync(temp, `${JSON.stringify(data, null, 2)}\n`, "utf8");
    fs.renameSync(temp, file);
  } finally {
    try {
      fs.rmSync(temp, { force: true });
    } catch {}
  }
}

function writeJsonAtomic(file, data) {
  const backup = `${file}.bak`;
  if (fs.existsSync(file)) {
    try {
      fs.copyFileSync(file, backup);
    } catch {}
  }
  writeJsonFile(file, data);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function preserveCorruptFile(file) {
  if (!fs.existsSync(file)) return null;
  const preserved = `${file}.corrupt-${timestampSuffix()}-${crypto.randomBytes(3).toString("hex")}`;
  fs.renameSync(file, preserved);
  return preserved;
}

function loadJson(file, fallback) {
  try {
    return readJson(file);
  } catch (err) {
    if (err?.code !== "ENOENT" && !(err instanceof SyntaxError)) throw err;
    if (err instanceof SyntaxError) preserveCorruptFile(file);
  }

  const backup = `${file}.bak`;
  try {
    const recovered = readJson(backup);
    writeJsonFile(file, recovered);
    return recovered;
  } catch (err) {
    if (err?.code !== "ENOENT" && !(err instanceof SyntaxError)) throw err;
    if (err instanceof SyntaxError) preserveCorruptFile(backup);
  }

  return typeof fallback === "function" ? fallback() : structuredClone(fallback);
}

module.exports = {
  loadJson,
  writeJsonAtomic,
};
