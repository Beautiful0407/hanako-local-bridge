# Hanako Local Bridge Rust Migration

## Status

The Rust implementation is currently `2.0.0-alpha.1`.

It is a tested development replacement for the local Windows bridge core, but it is not yet the production installer. Stable installed clients remain on `1.4.9` until the updater, installer payload, and Linux cloud router are migrated and verified.

Do not copy the Alpha EXEs over an existing installation manually. The final installer must preserve `config.json`, `data`, `logs`, cloud identity, approvals, execution authorizations, and update result history.

## Why Rust

The previous package bundled Node.js, PowerShell/VBS watchdog logic, and a self-contained .NET/Windows App SDK manager. The Rust design produces two native executables and uses the WebView2 runtime already present on supported Windows systems.

Measured on Windows x64 with the release profile:

```text
hanako-bridge.exe   4.54 MiB
hanako-manager.exe  2.19 MiB
combined            6.73 MiB
```

For comparison, the stable `1.4.9` installer is about `95.91 MiB`. An idle Rust preview service used about `6.6 MiB` working set during the July 19, 2026 verification.

## Workspace

```text
Cargo.toml
Cargo.lock
crates/
  hanako-bridge-core/
apps/
  hanako-bridge/
  hanako-manager/
tests/
  rust-integration.test.cjs
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
cargo build --workspace
node tests\rust-integration.test.cjs
cargo build --workspace --release
$env:HANAKO_RUST_BRIDGE_EXE = (Resolve-Path 'target\release\hanako-bridge.exe').Path
node tests\rust-integration.test.cjs
target\release\hanako-manager.exe --smoke-test
```

The integration test uses random loopback ports and a temporary root. It validates the real EXE rather than only calling Rust functions.

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

## Remaining Production Work

1. Implement the signed Rust online updater with HTTPS manifest download, RSA signature compatibility, SHA256 and size verification, detached replacement, rollback, and persisted result reporting.
2. Replace the installer payload so it ships only the Rust bridge, Rust manager, configuration, public update key, and required assets.
3. Remove Node.js, WinUI/.NET, PowerShell watchdog, and VBS launchers only after overwrite and update migration tests pass.
4. Port `cloud/device-router.cjs` to a Linux Rust service with device routing and offline queue compatibility.
5. Add cloud connector mock tests, active-job restart recovery tests, complete tool audit logging, and isolated Task Scheduler installation tests.
6. Verify tray interactions and complete installation on clean Windows 10 and Windows 11 virtual machines.
7. Publish a signed Alpha installer to a separate prerelease channel before moving stable clients from `1.4.9`.

## Release Rule

Every committed development stage must bump the product version. Do not publish an installer or update manifest from an uncommitted working tree, and do not overwrite the stable channel with an Alpha build.
