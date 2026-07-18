# Changelog

## 1.4.8 - 2026-07-18

- Added a signed-release payload inventory and remove obsolete managed files during online updates and overwrite installations.
- Preserved configuration, device data, logs, migration backups, and user-created root files while cleaning old manager/runtime dependencies.

## 1.4.7 - 2026-07-18

- Added a desktop manager shortcut during installation and repair missing desktop, Start menu, and uninstall entries during default-path online updates.
- Paused background polling on the Cloud devices page and disabled claim/query actions while another manager operation is active.
- Removed unused Windows App SDK AI, ML, and Widgets packages from the native manager payload.

## 1.4.6 - 2026-07-18

- Paused background health polling while the Settings page is open and wait for in-flight refreshes before checking for updates.
- Prevented the Settings update section from getting stuck at "not checked" when navigation races with a status refresh.

## 1.4.5 - 2026-07-18

- Replaced large update-package downloads with bounded, retried `curl.exe` transfers to prevent Windows PowerShell `Invoke-WebRequest` from hanging at zero bytes.
- Added a target-install-root option for recovery updates and a download regression test.

## 1.4.4 - 2026-07-18

- Fixed WinUI system tray notification decoding so double-click and right-click work with `NOTIFYICON_VERSION_4`.
- Added tray menu actions to reopen the manager or exit the manager process.
- Added signed online update checking and one-click installation in the manager settings.

## 1.4.3 - 2026-07-18

- Added a native Windows system-tray icon to the WinUI manager.
- Minimizing the manager now hides its window from the taskbar while keeping the bridge services running.
- Added tray actions to reopen or exit the manager.

## 1.4.2 - 2026-07-18

- Fixed WinUI cloud device query and claim failing in fresh Windows PowerShell `-File` processes because `WebRequestSession` was referenced before its utility assembly was loaded.
- Let `Invoke-RestMethod -SessionVariable` create the login session and added a regression test that verifies the same session is reused for the device query.

## 1.4.1 - 2026-07-18

- Moved the public signed update manifest and ZIP package endpoint to the production HTTPS server so automatic updates work while the GitHub source repository remains private.
- Added the Nginx static-release location and deployment/verification procedure for `/local-bridge/releases/`.

## 1.4.0 - 2026-07-18

- Added Bearer-token authentication to the loopback MCP endpoint, rejected browser Origin requests, validated loopback Host headers, and limited MCP and approval request bodies to 1 MiB.
- Added private MCP-token forwarding for the legacy SSH device router without exposing tokens in device-list responses.
- Migrated the official cloud endpoint from plaintext WebSocket to trusted `wss://154-201-69-202.sslip.io`.
- Preserved secondary configured filesystem roots when saving settings.
- Made the WinUI manager fit the current Windows work area and enabled overview scrolling on compact displays.
- Added RSA-SHA256 signatures for remote update manifests, enforced HTTPS, SHA256, and package-size verification, and embedded only the public signing key.
- Added an initial GitHub-managed stable manifest; superseded by the public HTTPS release endpoint in 1.4.1 because private GitHub repositories return 404 to unauthenticated clients.
- Added configuration, update-signature, MCP authentication, Origin, body-limit, and routed-token regression coverage.

## 1.3.1 - 2026-07-18

- Fixed WinUI repair/start/stop/restart actions failing JSON parsing when PowerShell emitted status text before the result.
- Enforced a JSON-only stdout contract in `manager-command.ps1` and added a regression test with a simulated `Stopped ...` preamble.
- Added WinUI fallback parsing for the final complete JSON value when unexpected command output is present.
- Prevented normal repair operations from stopping `HanakoBridgeManager.exe`; installers and updates still close it explicitly when replacing files.
- Restricted process cleanup to known Hanako watchdog, Node service, and legacy tunnel entry points instead of every process mentioning the install directory.
- Removed the private cloud maintenance manual from release payloads and delete legacy installed copies during install or update.

## 1.3.0 - 2026-07-17

- Replaced the primary Windows Forms manager with a modern WinUI 3 desktop application.
- Kept `manager-core.ps1` as the authoritative backend and added a JSON-only `manager-command.ps1` boundary for the native UI.
- Added overview, diagnostics, cloud-device, log, theme, repair, and service controls with stable UI Automation identifiers.
- Added mixed UTF-8 and BOM-less UTF-16LE log decoding without NUL characters or mojibake.
- Added a self-contained .NET 10 and Windows App SDK 2.3.1 manager build with XAML/PRI publish validation.
- Updated the launcher to validate and open the WinUI manager, with automatic Windows Forms fallback.
- Added installer and updater handling for a running native manager, plus installed-manager startup and smoke coverage.

## 1.2.1 - 2026-07-17

- Added a Windows Forms manager for local service status, diagnostics, repair, cloud device listing, device claim, and log viewing.
- Added hidden manager startup through `wscript.exe` and a Start menu shortcut.
- Added explicit `active`, `pending_claim`, `offline`, missing-task, missing-process, and missing-credential diagnostics.
- Added start, stop, restart, and detect-and-repair actions without opening a PowerShell window.
- Added temporary Hana web login for device queries and claim; the access key is never persisted.
- Added manager-core tests that verify offline diagnostics and prevent credentials or private key material from entering reports.
- Added manager files and checks to the Windows installer and isolated installer smoke test.

## 1.2.0 - 2026-07-17

- Replaced per-device SSH reverse tunnels with an active cloud WebSocket connection.
- Added persistent Ed25519 device identity, signed device proof, one-time browser claim tokens, and cloud-issued device credentials.
- Added automatic browser claim after authenticated Hana web login.
- Added WebSocket heartbeat, exponential reconnect, credential persistence, and automatic legacy-config migration.
- Added cloud connector status to local health and client identity endpoints.
- Updated the device router to register explicit loopback forwarding URLs for WebSocket-connected devices.
- Kept the SSH tunnel path as an opt-in compatibility fallback.
- Added cloud connector tests and full regression coverage.

## 1.1.0 - 2026-07-17

- Added browser-to-local-bridge device detection for per-computer file routing.
- Added an origin-restricted loopback identity endpoint with Private Network Access preflight support.
- Added automatic cloud device registration and conflict-free reverse tunnel port allocation.
- Disabled default-device fallback when more than one Windows bridge is configured.
- Added integration coverage for browser identity discovery, registration, and mandatory multi-device selection.

## 1.0.3 - 2026-07-17

- Added `local_fs.read_image` with native MCP image content for PNG, JPEG, GIF, and WebP files.
- Added file-signature validation, an 8 MB default image limit, metadata, SHA256, and read audit events.
- Added local, routed-device, and installer coverage for visual image reads.

## 1.0.2 - 2026-07-17

- Added a Windows-enforced timeout for one-shot SSH health and cleanup commands.
- Prevented a stuck SSH process from blocking the reverse-tunnel watchdog indefinitely.
- Made remote status checks terminate cleanly when SSH becomes unresponsive.
- Protected the updater's calling terminal and parent process chain during old-process cleanup.

## 1.0.1 - 2026-07-16

- Replaced hidden `Read-Host` installer prompts with a visible Windows Forms configuration window.
- Added clear success, cancellation, validation, and installation error dialogs.
- Separated normal double-click GUI installation from `/Q` non-interactive installation.
- Prevented repeated double-clicks from silently waiting for input while background tasks remain stopped.
- Added an installed configuration UI for later settings changes and repair.

## 1.0.0 - 2026-07-16

- Added a per-user Windows installer targeting `%LOCALAPPDATA%\HanakoLocalBridge`.
- Bundled the Node.js runtime so target computers do not need a separate Node installation.
- Added a shared `config.json` for device identity, roots, ports, tunnel, task names, and update settings.
- Added hidden single-instance MCP and SSH watchdogs with automatic crash recovery and reconnect.
- Generalized status, stop, repair, and uninstall scripts to use the real install root and configuration.
- Added migration that preserves config, device identity, authorization state, job state, and logs.
- Added local or URL update manifests with SHA256 verification and persistent-state preservation.
- Added an IExpress self-extracting EXE build and isolated end-to-end installer smoke tests.

## 0.7.1 - 2026-07-16

- Added an opt-in persistent offline queue through `queueIfOffline`.
- Added `local_device.queue` for queued/completed/failed call status.
- Added `local_device.cancel_queued` for cancelling work before reconnect.
- Added automatic queue replay after a device health check reports online.
- Added end-to-end tests that stop a device, queue a write, reconnect, and verify automatic execution.

## 0.7.0 - 2026-07-16

- Added persistent Windows device identity in `data/device.json`.
- Added `device://<deviceId>/C:/...` support for file and script execution paths.
- Added device metadata to health, roots, authorizations, jobs, and runtime discovery.
- Made local and remote SSH tunnel ports configurable per computer.
- Added a cloud MCP device router with online health, device selection, cached tools, and offline errors.
- Added `local_device.devices` for cloud-side device discovery and status.
- Added end-to-end device router tests for routed paths, explicit device selection, and missing devices.

## 0.6.1 - 2026-07-16

- Added cursor pagination to `local_fs.list`.
- Added glob matching, exclude rules, timeout budgets, visit budgets, and loop detection to `local_fs.search`.
- Added `local_fs.watch`, `local_fs.watch_events`, and `local_fs.unwatch`.
- Added an in-memory event ring, sequence cursors, debounce, overflow reporting, and long polling for watches.
- Added integration coverage for pagination, invalid cursors, bounded search, exclusions, and real Windows file events.

## 0.6.0 - 2026-07-16

- Added `local_fs.read_lines` with line numbers, SHA256, encoding, BOM, and newline metadata.
- Added reliable serialized `local_fs.append_text` for concurrent appends.
- Added SHA256-protected `local_fs.apply_patch` with exact replacement counts.
- Added UTF-8 BOM, UTF-16LE BOM, and UTF-16BE BOM detection and preservation.
- Updated `write_text` to preserve an existing file's encoding by default.
- Expanded integration coverage for UTF-16, line ranges, patch mismatch handling, and concurrent appends.

## 0.5.3 - 2026-07-16

- Added a detached hidden execution runner so PowerShell/Python jobs survive MCP process restarts.
- Persisted running job PID, state, output paths, and completion results before a job finishes.
- Added startup recovery for running jobs and exact post-restart status/output retrieval.
- Switched job completion tracking to the runner's `close` event so redirected output is flushed before completion.
- Added atomic JSON backups and automatic recovery of corrupted state files.
- Added audit, watchdog, MCP, and SSH log rotation plus completed-job output tail limits.
- Added end-to-end crash recovery, corrupted-state recovery, rotation, and output trimming tests.

## 0.5.2 - 2026-07-16

- Added normalized path-level locking for file creation, overwrite, copy, move, mkdir, and trash operations.
- Added a second SHA256 check immediately before replacing an existing file.
- Replaced delete-then-rename overwrite behavior with backup, replacement, and rollback handling.
- Added concurrent overwrite and concurrent creation integration tests.
- Added `package.json` as the single runtime version source and standard test/service scripts.

## 0.5.1 - 2026-07-16

- Added fully hidden `wscript.exe` launchers for the MCP and reverse tunnel watchdogs.
- Added automatic MCP restart and SSH tunnel reconnection with exponential backoff.
- Switched production mode to full local file and script execution trust.
- Added full-trust and failure-recovery integration coverage.
