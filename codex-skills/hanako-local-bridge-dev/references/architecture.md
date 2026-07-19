# Hanako Local Bridge Architecture

## Contents

1. Product boundary
2. Runtime topology
3. Rust ownership map
4. Request flows
5. Persistent data
6. Compatibility contracts
7. Security boundaries
8. Legacy boundary

## Product Boundary

Hanako Local Bridge lets cloud Hana use the currently selected Windows computer for:

- local filesystem read/write/search/watch/copy/move/trash;
- image reads as MCP image content;
- approved PowerShell and Python execution;
- multi-device selection and offline queueing;
- local diagnostics, settings, repair and signed updates.

The project has three separately deployed surfaces:

```text
Windows Local Bridge
  -> local MCP + manager + updater + installer

Cloud Hana LocalBridgeGateway
  -> WebSocket authentication and cloud-to-device forwarding

Linux Device Router
  -> MCP device selection, token forwarding and offline queue
```

Do not treat a successful health check on one surface as proof that the whole chain works.

## Runtime Topology

```text
Hana web or Agent
  -> Cloud Hana local_device/local_fs/local_exec tools
  -> Device Router on VPS loopback
  -> LocalBridgeGateway
  -> WSS device connection
  -> Windows hanako-bridge.exe
  -> filesystem / PowerShell / Python
```

Windows local endpoints:

```text
127.0.0.1:8787  MCP and primary health
127.0.0.1:8788  manager UI/API, approvals and client identity
```

The scheduled task starts `hanako-bridge.exe --service` directly. Release builds use the Windows
GUI subsystem; Debug builds remain console applications. The task combines:

- current-user logon trigger;
- one-minute repeating trigger;
- `MultipleInstancesPolicy=IgnoreNew`;
- `RestartOnFailure`;
- no execution time limit.

The repeating trigger covers external termination results that Task Scheduler does not recover
through `RestartOnFailure`.

## Rust Ownership Map

### `crates/hanako-bridge-core`

Own:

- config defaults, migration and deep merge;
- `%INSTALLDIR%` and environment expansion;
- stable device identity;
- path parsing for `local://`, `device://` and absolute Windows paths;
- signed update manifest types and version comparison;
- atomic JSON persistence and `.bak` recovery.

Changes here have broad compatibility impact. Run workspace tests and inspect all callers.

### `apps/hanako-bridge`

Own:

- Axum MCP and manager routes;
- filesystem and image tools;
- access control and approvals;
- PowerShell/Python job execution;
- audit events and job recovery;
- cloud WebSocket authentication/reconnect;
- scheduled task install/repair/start/stop;
- local health and client identity.

### `apps/hanako-manager`

Own:

- Winit/Wry/WebView2 window;
- embedded manager HTML;
- system tray icon and menu;
- single-instance activation;
- local health polling;
- repair/update/settings actions.

Manager lifecycle is independent from Bridge lifecycle. Closing or exiting Manager must not stop
the background MCP task.

### `apps/hanako-updater`

Builds `hanako-maintenance.exe`.

Own:

- HTTPS manifest download;
- RSA XML signature verification;
- SHA256 and size validation;
- safe ZIP extraction;
- transactional replacement and rollback;
- `data/update-state.json`;
- release `pack` command.

### `apps/hanako-installer`

Builds `HanakoLocalBridge-Setup.exe`.

Own:

- embedded runtime ZIP;
- first install and overwrite repair;
- legacy process/task takeover;
- shortcuts and uninstall registration;
- detached uninstall;
- preservation of persistent and unknown files.

### `apps/hanako-device-router`

Own:

- Linux `/health`, `/mcp`, `/devices/register`;
- device discovery and routing;
- `device://<deviceId>/...`;
- MCP token forwarding;
- offline queue persistence.

## Request Flows

### Local MCP Call

```text
HTTP /mcp
  -> Host/Origin/token/body validation
  -> MCP tool dispatch
  -> path and device resolution
  -> access-control decision
  -> filesystem or execution implementation
  -> audit/result persistence
  -> MCP result
```

Full trust bypasses interactive approvals but does not bypass path normalization, request limits,
token checks or audit boundaries.

### Cloud Device Authentication

```text
Bridge loads or creates Ed25519 identity
  -> WSS connect
  -> server nonce
  -> device signs nonce
  -> pending_claim or active
  -> cloud-issued device credential persists locally
```

The Hana web access password is not a device credential and must not enter Bridge config or logs.

### Browser Device Claim

```text
authenticated Hana web session
  -> browser reads loopback client identity
  -> cloud claim API validates claimToken + fingerprint
  -> cloud issues device credential
  -> Bridge reconnects/authenticates as active
```

### Signed Update

```text
Manager/Bridge checks effective manifest
  -> maintenance verifies HTTPS + signature + size + SHA256
  -> detached worker stops old runtime
  -> payload transaction preserves persistent files
  -> service task is repaired and started
  -> update-state.json records result
  -> Manager may reconnect after the old HTTP connection closes
```

The install HTTP request may end while the old Bridge exits. Treat `update-state.json`, installed
payload version and new health as authoritative.

## Persistent Data

Default install:

```text
%LOCALAPPDATA%\HanakoLocalBridge
```

Never remove or overwrite as managed payload:

```text
config.json
data\
logs\
unknown user-created files
```

Important data files include:

```text
data\device.json
data\cloud-identity.json
data\access-control.json
data\pending-requests.json
data\approval-token.txt
data\execution-authorizations.json
data\execution-requests.json
data\jobs\
data\update-state.json
```

Tests must prove persistence across overwrite install and online update whenever payload ownership
or cleanup logic changes.

## Compatibility Contracts

- Preserve unknown JSON fields through config deep merge.
- Keep stable device IDs and cloud identity across updates.
- Keep existing MCP tool names and result shapes unless versioned deliberately.
- Add optional request fields with backward-compatible defaults.
- Preserve UTF-8, UTF-16LE, UTF-16BE and BOM behavior.
- Use atomic file replacement and precondition hashes for concurrent writes.
- Keep Node/PowerShell stable clients compatible with cloud routing during Rust migration.
- Keep Alpha and stable update feeds isolated.

## Security Boundaries

- Bind local MCP and manager interfaces to loopback.
- Require approval token on protected local APIs.
- Reject browser Origin access to MCP.
- Validate Host and request-body limits.
- Keep execution authorizations explicit and auditable.
- Never log or commit private keys, approval tokens, claim tokens, access passwords or credentials.
- Keep update signing private key outside the repository.
- Accept remote updates only over HTTPS and only after signature, size and hash checks.
- Keep cloud internal forwarding endpoints on VPS loopback.

## Legacy Boundary

Legacy Node, PowerShell, VBS and WinUI files remain for:

- stable `1.4.9` maintenance;
- migration fixtures;
- overwrite takeover testing;
- rollback and compatibility reference.

Do not remove them as cleanup during Rust feature work. Remove legacy code only through a separate,
versioned migration with rollback evidence.
