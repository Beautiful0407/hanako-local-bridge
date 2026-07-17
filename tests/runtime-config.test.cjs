const assert = require("assert");
const fsp = require("fs/promises");
const os = require("os");
const path = require("path");

const {
  createDefaultConfig,
  loadRuntimeConfig,
  mergeDeep,
} = require("../lib/runtime-config.cjs");

async function run() {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "hana-runtime-config-test-"));
  const previousConfig = process.env.HANA_LOCAL_BRIDGE_CONFIG;
  try {
    const root = path.join(temp, "root");
    await fsp.mkdir(root, { recursive: true });
    const configPath = path.join(temp, "config.json");
    await fsp.writeFile(
      configPath,
      `${JSON.stringify(
        {
          schemaVersion: 1,
          device: { id: "Configured Device", name: "Configured name" },
          filesystem: {
            port: 29123,
            roots: [
              { name: "Root", path: "root", mode: "read_write" },
              { name: "Install", path: "%INSTALLDIR%", mode: "read" },
            ],
          },
          storage: {
            dataDir: "state/data",
            logDir: "state/logs",
          },
          tunnel: {
            identityFile: "%USERPROFILE%/.ssh/id_ed25519",
          },
        },
        null,
        2,
      )}\n`,
      "utf8",
    );

    const runtime = loadRuntimeConfig({ installDir: temp, configPath });
    assert.equal(runtime.exists, true);
    assert.equal(runtime.config.device.id, "Configured Device");
    assert.equal(runtime.config.filesystem.port, 29123);
    assert.equal(runtime.config.filesystem.approvalPort, 8788);
    assert.equal(runtime.config.filesystem.roots[0].path, root);
    assert.equal(runtime.config.filesystem.roots[1].path, temp);
    assert.equal(runtime.config.filesystem.roots[1].mode, "read");
    assert.equal(runtime.config.storage.dataDir, path.join(temp, "state", "data"));
    assert.equal(runtime.config.storage.logDir, path.join(temp, "state", "logs"));
    assert.ok(path.isAbsolute(runtime.config.tunnel.identityFile));
    assert.equal(runtime.config.cloud.enabled, true);
    assert.equal(runtime.config.cloud.url, "ws://154.201.69.202/local-bridge/connect");
    assert.equal(runtime.config.tunnel.enabled, false);

    const defaults = createDefaultConfig(temp);
    const merged = mergeDeep(defaults, { filesystem: { port: 30000 } });
    assert.equal(merged.filesystem.port, 30000);
    assert.equal(merged.filesystem.approvalPort, 8788);
    assert.equal(defaults.filesystem.port, 8787);
    assert.equal(defaults.cloud.enabled, true);
    assert.equal(defaults.tunnel.enabled, false);

    process.env.HANA_LOCAL_BRIDGE_CONFIG = configPath;
    const fromEnvironment = loadRuntimeConfig({ installDir: temp });
    assert.equal(fromEnvironment.configPath, configPath);
  } finally {
    if (previousConfig === undefined) delete process.env.HANA_LOCAL_BRIDGE_CONFIG;
    else process.env.HANA_LOCAL_BRIDGE_CONFIG = previousConfig;
    await fsp.rm(temp, { recursive: true, force: true });
  }

  console.log("runtime config tests passed");
}

run().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
