const path = require("path");

const { loadRuntimeConfig } = require("../lib/runtime-config.cjs");

function argument(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : "";
}

const installDir = path.resolve(argument("--install-dir") || path.resolve(__dirname, ".."));
const configPath = argument("--config") || undefined;
process.stdout.write(`${JSON.stringify(loadRuntimeConfig({ installDir, configPath }))}\n`);
