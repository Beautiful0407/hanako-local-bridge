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

Hanako Local Bridge is one Rust product. It has one user-facing identity, one Windows installer,
one manager entry, one configuration model, one product version, one update policy and one
diagnostic/repair experience. New product behavior is implemented only in Rust.

The product contains several runtime roles:

```text
Hanako Local Bridge product
  -> Windows background Bridge role
  -> Windows Manager role
  -> hidden maintenance/update role
  -> installer/bootstrap artifact
  -> cloud gateway and Linux Device Router role
```

These roles are not separate products. They must not acquire separate user accounts, settings
stores, installers, version schemes, release channels or management applications.

The runtime is deployed on both Windows and Linux because the responsibilities and operating
systems differ:

```text
Windows installation
  -> local MCP + manager + maintenance helper

Cloud Hana LocalBridgeGateway
  -> WebSocket authentication and cloud-to-device forwarding

Linux Device Router
  -> MCP device selection, token forwarding and offline queue
```

Multiple internal processes are allowed where they provide real reliability:

- the background Bridge must survive closing the Manager;
- an updater must be able to replace a running Bridge;
- the Linux router cannot be the same operating-system binary as the Windows desktop runtime.

This is an implementation boundary, not a product boundary. Users manage Hanako Local Bridge as
one program. Do not treat a successful health check on one runtime role as proof that the whole
chain works.

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

Launching `hanako-bridge.exe` without arguments is the user-facing product entry and opens the
internal single-instance Manager. `--status`, `--repair` and `--doctor` are direct Rust maintenance
roles. Desktop and Start menu shortcuts must target `hanako-bridge.exe`, never the internal Manager
binary.

## Rust Ownership Map

The Cargo workspace is an internal modularization mechanism. `apps/*` names build targets and
runtime roles, not standalone products. Shared product contracts belong in
`crates/hanako-bridge-core`; user-facing configuration, versioning and release behavior must remain
coherent across all targets.

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
- Develop all new product functionality in Rust.
- Keep Node/PowerShell artifacts only as migration inputs, compatibility fixtures and rollback
  references until their removal gates are satisfied.
- Keep one product version and coordinated compatibility matrix for Windows and cloud runtime
  roles. Platform-specific build numbers may be recorded internally but must not become separate
  user-facing product lines.
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

- migration fixtures;
- overwrite takeover testing;
- rollback and compatibility reference.

Do not add features to legacy implementations and do not use them as the primary fix path. Remove
legacy code only through a versioned Rust migration with rollback evidence, after installed stable
clients can be upgraded without losing configuration, identity, data or service recovery.
