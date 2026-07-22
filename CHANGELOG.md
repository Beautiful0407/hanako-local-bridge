# Changelog

## 2.0.0-alpha.17 - 2026-07-22

- Fixes the real root cause behind the `cannot remove ...\hanako-bridge.exe: 拒绝访问。 (os error 5); rollback also failed` install failure. A still-running `hanako-bridge.exe` holds an exclusive image lock on its own file for its whole lifetime, so the file can never be deleted while that process lives — the alpha.16 timed retry could not help because the lock is not transient. On a machine where `stop_installed_service_and_processes` fails to terminate the old Bridge (for example when it cannot resolve the process's executable path), the overwrite hit that permanent lock and aborted the whole install and its rollback.
- Overwriting a managed file now falls back to renaming the locked original aside to a `.hanako-old` sidecar (renaming a running image is allowed on Windows even though deleting it is not), then puts the new file in place. The running process keeps mapping the displaced image and picks up the new binary on its next launch, so installs no longer depend on the old process having already exited. Leftover sidecars are swept at the start of the next install once their process has exited.
- Adds a regression test that locks a binary with a real running process, then asserts the overwrite succeeds and the live file is displaced instead of the install failing, plus sidecar-path and sweep unit coverage.

## 2.0.0-alpha.16 - 2026-07-22

- Retries the install/update payload transaction against transient Windows file locks so installs no longer die with `拒绝访问。 (os error 5); rollback also failed` on machines where an antivirus scanner briefly opens a freshly written executable, or a just-terminated Bridge process has not released its handle yet. The remove/rename/copy steps in `apply`, `rollback`, and `replace_file` now retry for up to ten seconds on `ERROR_ACCESS_DENIED` (5) and `ERROR_SHARING_VIOLATION` (32), while non-transient errors still fail immediately.
- Adds unit coverage for the transient-lock retry helper: it recovers after repeated access-denied errors and gives up at once on unrelated errors.

## 2.0.0-alpha.15 - 2026-07-21

- Steers connected agents toward the correct execution pattern via the `local_exec.execute` tool description: `execute` blocks and is only for short tasks, while long-running work (npm install, large recursive scans, downloads) should use `request_run` + `run` and poll `job_status`/`job_output`, which is not bound by the request timeout. Also documents that a child process started with a bare `Start-Process` detaches and its stdout is not captured; run it inline or with `-NoNewWindow -Wait`. Behavior is unchanged; this is guidance only.

## 2.0.0-alpha.14 - 2026-07-21

- Keeps the cloud WebSocket connection alive during long-running RPC calls. The connection loop previously awaited each `rpc_request` inline, so a slow tool call (an npm install, a large recursive directory scan, a long script) froze the heartbeat/ping loop for the whole call and the server dropped the connection as dead. RPC handling is now dispatched to its own task, with responses delivered through an outbound channel, while the loop keeps servicing heartbeats and pings.
- Corrects the integration and device-router tool-count assertions (33 bridge tools, 36 through the router) after the alpha.13 process-management tools were added.

## 2.0.0-alpha.13 - 2026-07-21

- Adds `local_exec.list_processes` and `local_exec.terminate` so the agent can observe and manage arbitrary processes, returning a structured result (terminated / failed-with-reason / protected / matched) instead of an opaque error. Protection is PID-aware: the bridge, its running job workers, and the manager/updater are never killed; a by-name match of more than one process requires confirmation.
- Exposes lightweight runtime metrics (uptime, per-tool call counts, cloud reconnect count) on `/health` and the manager snapshot, using process-local counters with no external telemetry.
- Bounds the device-router offline queue even when every item is still queued, evicting the oldest item so an offline device with repeated `queueIfOffline` calls can no longer grow the queue without limit.
- Fixes a device-router panic where a downstream device returning a non-object tool entry (or inputSchema) could crash tool refresh; malformed entries are now left untouched.
- Adds a regression test for accepting a fully-received update download that ends without a clean TLS close, plus device-router routing and queue coverage.

## 2.0.0-alpha.12 - 2026-07-20

- Makes remote update package downloads retryable and resumable after interrupted response bodies.
- Sends HTTP Range requests from the retained partial-file offset and safely restarts when a server ignores the range.
- Accepts a body-close error only when the downloaded file already reached the signed manifest size, then still requires the existing SHA256 verification.
- Reduces each remote request timeout to three minutes, adds TCP keepalive, and forces HTTP/1.1 for more predictable Windows update downloads.
- Adds deterministic interrupted-body coverage and an explicit ignored probe for validating a real signed release URL, size, and SHA256.

## 2.0.0-alpha.11 - 2026-07-20

- Repairs desktop and Start menu shortcuts during signed online updates so existing installations migrate from the internal `hanako-manager.exe` target to the unified `hanako-bridge.exe` product entry.
- Refreshes the uninstall `DisplayIcon` and `DisplayVersion` during online updates, matching fresh and overwrite installs.
- Moves Windows Shell integration into the shared Rust maintenance library so the installer and updater use one implementation.
- Treats Shell integration repair failure as an update failure and rolls the managed payload back instead of reporting a partial success.
- Extends the Alpha 10-to-Alpha 11 signed update smoke test with isolated shortcut and uninstall-registry migration checks.

## 2.0.0-alpha.10 - 2026-07-20

- Makes `hanako-bridge.exe` the single user-facing Windows product entry.
- Opens the Rust tray manager when the product entry is launched without arguments, while `--service` remains the explicit background-service role.
- Adds top-level `--status`, `--repair`, and `--doctor` commands without routing product maintenance through legacy scripts.
- Points desktop, Start menu, uninstall display-icon, post-install launch, and post-update relaunch behavior at the unified product entry.
- Keeps `hanako-manager.exe` as an internal single-instance UI role and `hanako-maintenance.exe` as the hidden self-update role.
- Extends integration and installer coverage for explicit service startup, shortcut targets, repeated product launches, background survival, and Alpha 9-to-Alpha 10 signed updates.

## 2.0.0-alpha.9 - 2026-07-19

- Adds a one-minute repeating Task Scheduler trigger to the existing logon trigger.
- Keeps `MultipleInstancesPolicy=IgnoreNew`, so the periodic trigger does not create duplicate Bridge processes while the service is healthy.
- Recovers the background Bridge after an external force termination that Task Scheduler reports as `0xFFFFFFFF` and does not handle through `RestartOnFailure`.
- Extends the installed-service smoke test to terminate the Bridge, wait for a different process ID, verify both health endpoints, and assert that the recovered process has no visible window.
- Advances signed update coverage from Alpha 8 to Alpha 9.

## 2.0.0-alpha.8 - 2026-07-19

- Builds the release `hanako-bridge.exe` as a Windows GUI subsystem executable so Task Scheduler can run it without allocating a visible console window.
- Keeps debug builds as console applications for local diagnostics.
- Preserves redirected JSON output for manager service commands after the subsystem change.
- Adds a PE subsystem regression assertion to the installed-service smoke test.
- Verifies closing the tray manager does not stop the independent background MCP bridge.
- Advances signed update coverage from Alpha 7 to Alpha 8.

## 2.0.0-alpha.7 - 2026-07-19

- Added a separate signed Alpha update feed under `/local-bridge/releases/alpha/` while leaving the stable `1.4.9` feed unchanged.
- Automatically selects the Alpha feed for prerelease Rust builds that still carry the official stable manifest URL.
- Preserves custom update manifest URLs and supports explicitly selecting the Alpha channel.
- Shows the effective manifest in manager status and settings so the displayed source matches the source actually used.
- Retries manager update checks once after a transient local connection failure and translates raw `Failed to fetch` errors into a Chinese recovery message.
- Added regression coverage for stable, prerelease, explicit channel, custom manifest, and manager update error behavior.

## 2.0.0-alpha.6 - 2026-07-19

- Replaced the fixed 2.5-second manager refresh after repair or restart with a 30-second recovery loop that tolerates the expected local disconnect.
- Clears transient connection errors after the local service returns and keeps service-action buttons disabled until recovery is complete.
- Reports `connecting` and `authenticating` cloud states as warnings instead of final errors; an intentionally disabled cloud connection is healthy.
- Adds a warning-level overall manager state and displays cloud state plus the last connection error in diagnostic details.
- Handles settings-triggered restarts with the same recovery flow when the manager port is unchanged.
- Added regression coverage for cloud transition classification and the manager recovery state machine.

## 2.0.0-alpha.5 - 2026-07-19

- Localized cloud connection, trust mode, diagnostic item, diagnostic status, root permission, root source, and update status values in the Rust manager.
- Added clearer Chinese progress, success, connection, and Windows access-denied messages to the diagnostics interface.
- Fixed manager repair, restart, stop, and settings-triggered restart failing with `Access is denied (os error 5)` inside the scheduled-task Windows Job.
- Replaced `CREATE_BREAKAWAY_FROM_JOB` workers with an independent hidden on-demand scheduled task that cleans itself up after the service action.
- Added a real installed-service regression step that fails on Alpha 4 and verifies the manager repair API on Alpha 5.
- Added Alpha 4-to-Alpha 5 signed update coverage and rebuilt the Alpha 5 ZIP, manifest, and embedded installer.

## 2.0.0-alpha.4 - 2026-07-19

- Fixed migration from stable Node installations whose scheduled task launches `wscript`, a PowerShell watchdog, and a detached `node.exe` process.
- Detect legacy installations even when they do not contain a Rust `payload-manifest.json`.
- Stop legacy MCP and tunnel tasks, then terminate only processes whose executable or command line belongs to the target Hanako installation directory.
- Preserve the real bridge service repair exit code, stdout, stderr, and rollback error instead of reducing every failure to `installed bridge service failed to start`.
- Replaced the direct-Node installer fixture with a detached VBS/Node fixture that fails on Alpha 3 and passes on Alpha 4.
- Added signed Alpha 3-to-Alpha 4 update coverage and rebuilt the Alpha 4 ZIP, manifest, and embedded installer.

## 2.0.0-alpha.3 - 2026-07-19

- Fixed the Rust manager accepting any listener on the configured approval port, which could open the legacy Node manager endpoint and display `{"error":"invalid approval token"}`.
- Added explicit Rust runtime and exact-version identity checks before the manager opens its WebView.
- Added per-installation manager single-instance activation so repeated shortcut clicks restore the existing window instead of opening duplicate windows.
- Changed scheduled-task repair to stop the previous task and wait for both local ports to be released before starting the Rust replacement.
- Added bounded uninstall retries so short-lived WebView2 file handles do not leave a partially removed installation.
- Added regression coverage for legacy-service takeover, overwrite data preservation, installed-manager multiple launch, uninstall after WebView2 startup, and signed Alpha 2-to-Alpha 3 update.
- Rebuilt the signed Alpha 3 ZIP, manifest, and embedded Windows installer. The cloud Linux device router remains on compatible Rust Alpha 2.

## 2.0.0-alpha.2 - 2026-07-19

- Ported the signed updater, embedded Windows installer, and Linux multi-device router to Rust.
- Added detached updater handoff with launcher-PID waiting so Windows can replace a running maintenance binary without an intermittent `os error 5`.
- Added transactional payload replacement, stale managed-file cleanup, rollback, signed manifest compatibility, persistent update state, and preservation of configuration, data, logs, and unknown user files.
- Added per-user shortcuts, uninstall registration, detached uninstall, UTF-16 Task Scheduler XML, and isolated install/overwrite/uninstall smoke coverage.
- Added complete MCP audit events without sensitive arguments and active execution-job recovery across bridge restarts.
- Added Rust cloud protocol, device-router, audit, recovery, installer, and Alpha 1-to-Alpha 2 update integration tests.
- Rebuilt the signed `2.0.0-alpha.2` ZIP and embedded installer after the updater fix.
- Deployed the Rust device router to the cloud host and verified 34 tools, two online Windows devices, real MCP calls, offline-queue compatibility, and the public Hana web entry.
- Kept the installed Windows stable channel on `1.4.9`; Alpha 2 remains a separate prerelease until clean Windows 10/11 rollout validation is complete.

## 2.0.0-alpha.1 - 2026-07-19

- Added a Rust workspace with shared configuration, device identity, path resolution, and crash-safe JSON storage.
- Ported the Windows MCP bridge, all 31 filesystem and execution tools, approval modes, job runner, cloud WebSocket connector, and scheduled-task service controls to Rust.
- Added a lightweight Rust WebView2 manager with overview, diagnostics, roots, logs, settings, tray restore, and tray exit behavior.
- Added debug and release EXE integration coverage, including token authentication, atomic writes, SHA256 concurrency checks, UTF-16, images, search, watches, PowerShell jobs, private-network CORS, manager HTML, and favicon behavior.
- Kept the production `1.4.9` installer and updater unchanged until the Rust updater, installer payload, and Linux device router are complete.

## 1.4.9 - 2026-07-18

- Added a verified handoff between the manager and the detached online updater before the manager exits.
- Persist update success or failure, verify the installed version, and show the result when the manager reopens.
- Keep the manager open with an actionable error when the updater cannot start instead of silently appearing to do nothing.

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
