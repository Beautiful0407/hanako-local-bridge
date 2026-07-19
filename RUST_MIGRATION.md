# Hanako Local Bridge Rust Migration

## Status

The Rust implementation is currently `2.0.0-alpha.8`.

Alpha 8 contains the Windows bridge, manager, signed updater, and embedded installer. The release Bridge now uses the Windows GUI subsystem, so Task Scheduler runs it without allocating a visible console window. Debug builds remain console applications, redirected service-command JSON still works, and closing the manager leaves the independent background Bridge running. The separate Alpha feed and recovery behavior remain in place. The compatible Linux cloud router remains deployed at Alpha 2.

Do not copy the Alpha EXEs over an existing installation manually. Use the embedded Alpha 8 installer. It supports first install and overwrite repair of an existing Node or Alpha installation while preserving `config.json`, `data`, `logs`, cloud identity, approvals, execution authorizations, jobs, and update result history.

## Why Rust

The previous package bundled Node.js, PowerShell/VBS watchdog logic, and a self-contained .NET/Windows App SDK manager. The Rust design produces three Windows runtime executables plus a bootstrap installer and uses the WebView2 runtime already present on supported Windows systems.

Measured from the rebuilt Windows x64 Alpha 8 release:

```text
hanako-bridge.exe       6,267,392 bytes
hanako-manager.exe      2,299,904 bytes
hanako-maintenance.exe  5,707,264 bytes
runtime ZIP             6,674,871 bytes
embedded installer      8,898,048 bytes
```

For comparison, the stable `1.4.9` installer is about `95.91 MiB`. On the cloud host, the Rust device router used about `5.1 MiB` after the observation period; the replaced Node router used about `48.8 MiB`.

## Workspace

```text
Cargo.toml
Cargo.lock
crates/
  hanako-bridge-core/
apps/
  hanako-bridge/
  hanako-device-router/
  hanako-installer/
  hanako-manager/
  hanako-updater/          # hanako-maintenance.exe
tests/
  rust-integration.test.cjs
  rust-device-router.test.cjs
  rust-audit.test.cjs
  rust-recovery.test.cjs
  rust-installer-smoke.ps1
  rust-update-smoke.ps1
```

### `hanako-bridge-core`

- Deep-merges existing JSON configuration with defaults.
- Expands `%INSTALLDIR%` and other environment variables.
- Migrates legacy cloud and update URLs.
- Persists stable device identity.
- Writes JSON atomically with `.bak` recovery and corrupt-file preservation.
- Resolves `local://`, `device://`, aliases, and approved absolute paths.

### `hanako-bridge`

- Serves the token-protected MCP endpoint and local manager API with Axum.
- Registers all 31 local filesystem and execution tools.
- Supports full-trust and approval modes.
- Reads and writes UTF-8, UTF-16LE, and UTF-16BE while preserving BOM state.
- Uses atomic replacement and SHA256 preconditions for concurrent writes.
- Supports bounded search, image blocks, polling watches, recoverable trash, copy, move, append, and exact patching.
- Executes approved PowerShell and Python scripts through an isolated job runner.
- Persists jobs, supports timeout and cancellation, and recovers runner results after service restart.
- Reuses Ed25519 cloud identity and reconnects to the cloud WebSocket with heartbeat and backoff.
- Installs or repairs a scheduled task that starts the Rust service directly.

### `hanako-manager`

- Uses Winit, Wry, WebView2, and `tray-icon`.
- Shows overview, diagnostics, configured roots, logs, and settings.
- Hides to the system tray when minimized or closed.
- Restores on tray double-click and provides Open and Exit menu commands.
- Starts or repairs the Rust service when the local manager endpoint is unavailable.
- Uses the Windows GUI subsystem in release builds, so no console window is shown.

### `hanako-maintenance.exe`

- Checks local or HTTPS manifests and verifies the legacy RSA XML public key format.
- Downloads packages with HTTPS, validates SHA256 and size, and rejects HTTP.
- Extracts ZIP files with traversal protection.
- Applies payloads transactionally and preserves `config.json`, `data`, `logs`, and unknown user files.
- Removes stale files only from the previous managed payload manifest.
- Runs as a detached worker, records `data/update-state.json`, and rolls back on replacement failure.
- Provides the Rust `pack` command for creating runtime-only ZIP packages and update manifests.

### `HanakoLocalBridge-Setup.exe`

- Embeds the Rust runtime ZIP at build time.
- Installs per-user under `%LOCALAPPDATA%\HanakoLocalBridge` without administrator elevation.
- Creates desktop and Start menu shortcuts using Rust.
- Registers a per-user uninstall entry and uses a detached Rust uninstall worker.
- Uses the same payload transaction as online updates for overwrite installation.

### `hanako-device-router`

- Replaces `cloud/device-router.cjs` on Linux.
- Keeps `/health`, `/mcp`, and `/devices/register`.
- Preserves the 34-tool surface, `device://<deviceId>/...` selection, token forwarding, and offline queue files.
- Uses the same JSON config, cache, and queue paths as the Node router.

## Toolchain

Verified toolchain:

```text
Rust/Cargo: 1.97.1
Visual Studio Build Tools: C:\BuildTools2026
Windows SDK: 10.0.26100.0
```

Load the MSVC environment in PowerShell before Cargo builds:

```powershell
$vcvars = 'C:\BuildTools2026\VC\Auxiliary\Build\vcvars64.bat'
$lines = cmd.exe /d /s /c "`"$vcvars`" >nul && set"
foreach ($line in $lines) {
  if ($line -match '^([^=]+)=(.*)$') {
    [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
  }
}
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

## Build And Test

Run the strict validation chain:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
$env:HANAKO_RUST_BRIDGE_EXE = (Resolve-Path 'target\release\hanako-bridge.exe').Path
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-update-smoke.ps1
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-installer-smoke.ps1
$manager = Start-Process target\release\hanako-manager.exe -ArgumentList '--smoke-test' -Wait -PassThru
if ($manager.ExitCode -ne 0) { throw "Manager smoke test failed" }
```

The integration tests use random loopback ports and temporary roots. The installer smoke test launches a detached legacy Node process through VBS, verifies Alpha 8 takeover, asserts the installed Bridge PE subsystem is Windows GUI, calls the installed manager repair API, checks overwrite and manager single-instance behavior, closes the manager while confirming the Bridge stays healthy, and uninstalls after WebView2 startup. The update smoke test installs an Alpha 7 payload, injects the current Alpha 8 maintenance binary, and verifies deterministic signed update handoff.

## Release Packaging

The production private key remains outside the repository:

```text
%USERPROFILE%\.hanako-update-signing\private-key.xml
```

Build a signed prerelease:

```powershell
cargo build --workspace --release

target\release\hanako-maintenance.exe pack `
  --binaries target\release `
  --output build\rust-release-alpha8 `
  --public-key update-public-key.xml `
  --version 2.0.0-alpha.8 `
  --channel alpha `
  --package-url HanakoLocalBridge-2.0.0-alpha.8-win-x64.zip `
  --signing-key "$env:USERPROFILE\.hanako-update-signing\private-key.xml" `
  --notes "Hanako Local Bridge Rust 2.0.0-alpha.8: invisible persistent background service"

$env:HANA_INSTALLER_PAYLOAD = (
  Resolve-Path 'build\rust-release-alpha8\HanakoLocalBridge-2.0.0-alpha.8-win-x64.zip'
).Path
cargo build -p hanako-bootstrap --release
```

## Preview

Set `HANA_LOCAL_BRIDGE_CONFIG` to an isolated config before starting a preview:

```powershell
$env:HANA_LOCAL_BRIDGE_CONFIG = 'C:\path\to\preview\config.json'
target\debug\hanako-bridge.exe
```

Open:

```text
http://127.0.0.1:<approvalPort>/manager/
```

Do not point a development preview at the production data directory.

## Compatibility

The Rust service reads the existing camelCase `config.json` structure and keeps unknown configuration fields during deep merge. It keeps the established storage filenames and cloud identity format so the final installer can migrate without forcing every device to be claimed again.

Supported compatibility paths include:

```text
local://<alias>/...
device://<deviceId>/C:/...
C:\absolute\path
```

The final migration must verify these files across overwrite installation and online update:

```text
config.json
data/device.json
data/cloud-identity.json
data/access-control.json
data/pending-requests.json
data/approval-token.txt
data/execution-authorizations.json
data/execution-requests.json
data/jobs/
logs/
```

## Verified Deployment

The Linux router was built on Ubuntu 22.04 with Rust `1.97.1` and deployed as:

```text
/opt/hanako-local-device-router/hanako-device-router
/etc/systemd/system/hanako-local-device-router.service
```

Live verification on July 19, 2026 confirmed:

```text
router version: 2.0.0-alpha.2
tools: 34
online devices: laptop-hl78935t, 5cd5469l5j
local_device.devices: success
local_fs.roots routed to laptop-hl78935t: success
Hana web entry: HTTP 200 and connected UI
```

The previous Node script and a root-only timestamped backup remain on the server for rollback.

## Remaining Production Work

1. Publish the signed Alpha 8 installer and manifest to the separate prerelease channel.
2. Verify installation, tray behavior, update, uninstall, and reboot recovery on clean Windows 10 and Windows 11 virtual machines.
3. Run a staged migration on a non-primary device before offering the Rust installer to the stable fleet.
4. Keep the Node/PowerShell/VBS/WinUI implementation until stable clients have completed a rollback-capable migration.
5. Remove legacy code only in a later version-bumped commit after stable telemetry and backups are confirmed.

## Release Rule

Every committed development stage must bump the product version. Do not publish an installer or update manifest from an uncommitted working tree, and do not overwrite the stable channel with an Alpha build.
