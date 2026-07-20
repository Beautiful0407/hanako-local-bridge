# Hanako Local Bridge Release Runbook

## Contents

1. Scope and safety
2. Preconditions
3. Version preparation
4. Quality gate
5. Signed package and installer
6. Smoke tests
7. Commit, tag and GitHub release
8. VPS Alpha feed
9. Local online update
10. Rollback
11. Operation record

## Scope And Safety

This runbook publishes the independent Local Bridge Windows artifacts and, when required, the
compatible Linux Device Router. It does not upgrade the Cloud Hana application release.

Keep feeds separate:

```text
Stable: /local-bridge/releases/update-manifest.json
Alpha:  /local-bridge/releases/alpha/update-manifest.json
```

Never publish Alpha content over the stable manifest.

Do not print:

- signing private key;
- approval token;
- cloud device private key or credential;
- Hana access password;
- full production config or logs.

## Preconditions

```powershell
git status --short --branch
git fetch --all --tags --prune
git log -1 --oneline --decorate
```

Confirm:

- intended branch and remote;
- clean or fully understood worktree;
- next version is unused;
- previous package needed by update smoke still exists;
- signing key file exists;
- Rust/MSVC/Node toolchains work;
- VPS SSH identity proves the expected hostname and user before writes.

Signing key location:

```text
%USERPROFILE%\.hanako-update-signing\private-key.xml
```

Check existence only. Never output its content.

## Version Preparation

For a runtime release, update:

```text
Cargo.toml workspace version
Cargo.lock workspace package versions
tests that assert current version
installer/update smoke source and target versions
CHANGELOG.md
README.md
DEVELOPMENT_MANUAL.md
OPERATION_MANUAL.md
RUST_MIGRATION.md
WINDOWS_INSTALLER_UPDATE_MANUAL.md
CLOUD_DEPLOYMENT_GUIDE.md
```

Use one target version across source, tests, package, manifest, installer, tag and release title.

## Quality Gate

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo build --workspace
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs
```

Run affected legacy tests when migration or shared config behavior changes.

## Signed Package And Installer

Set release variables:

```powershell
$version = '2.0.0-alpha.N'
$label = 'alphaN'
$out = Join-Path (Get-Location) "build\rust-release-$label"
$package = "HanakoLocalBridge-$version-win-x64.zip"
$installer = "HanakoLocalBridge-Setup-$version.exe"
```

Create signed runtime ZIP and manifest:

```powershell
& '.\target\release\hanako-maintenance.exe' pack `
  --binaries '.\target\release' `
  --output $out `
  --public-key '.\update-public-key.xml' `
  --version $version `
  --channel 'alpha' `
  --package-url $package `
  --signing-key (Join-Path $env:USERPROFILE '.hanako-update-signing\private-key.xml') `
  --notes "Hanako Local Bridge Rust $version: <release summary>"
```

Embed the exact signed ZIP:

```powershell
$env:HANA_INSTALLER_PAYLOAD = (Resolve-Path (Join-Path $out $package)).Path
cargo build -p hanako-bootstrap --release
Copy-Item -LiteralPath '.\target\release\HanakoLocalBridge-Setup.exe' `
  -Destination (Join-Path $out $installer) -Force
```

Record hashes:

```powershell
Get-ChildItem -LiteralPath $out -File |
  Select-Object Name, Length, @{
    Name = 'SHA256'
    Expression = {
      (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
  }
```

Verify manifest version, channel, package name, size and SHA256 match the generated ZIP.

## Smoke Tests

Update smoke must start from the previous Alpha package and end at the new package:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
  tests\rust-update-smoke.ps1
```

It must also create legacy shortcut and uninstall metadata fixtures, verify migration to
`hanako-bridge.exe`, and prove a forced Shell integration failure reports `failed` and rolls the
managed payload back.

Installer smoke must verify:

- legacy takeover;
- installed PE subsystem is Windows GUI;
- first install and overwrite;
- data/log/unknown-file preservation;
- desktop and Start menu shortcuts target `hanako-bridge.exe`;
- manager repair;
- repeated unified product launches keep one internal Manager instance;
- closing manager leaves Bridge healthy;
- force-terminated Bridge returns as a different PID without a visible window;
- uninstall removes task, registry entry and install tree.

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
  tests\rust-installer-smoke.ps1
```

Do not publish if either smoke test fails.

For a remote-download change, also run the ignored probe with the exact public package URL, signed
manifest size and SHA256. It must download to a temporary test directory and must not install or
start services.

## Commit, Tag And GitHub Release

Review:

```powershell
git diff --check
git diff --stat
git status --short --branch
```

Commit with a message that explains the user-visible change or root cause. Push the commit before
publishing:

```powershell
git push origin <branch>
git tag -a "v$version" -m "Hanako Local Bridge $version"
git push origin "v$version"
```

Create a prerelease:

```powershell
gh release create "v$version" `
  (Join-Path $out $package) `
  (Join-Path $out $installer) `
  (Join-Path $out 'update-manifest.json') `
  --repo Beautiful0407/hanako--MCP- `
  --title "Hanako Local Bridge $version" `
  --prerelease `
  --notes "<tested release notes>"
```

Read back release asset names, sizes and digests. Do not assume upload succeeded because the command
returned a URL.

## VPS Alpha Feed

First prove the host:

```powershell
ssh -o BatchMode=yes -o ConnectTimeout=10 root@154.201.69.202 `
  "hostname; whoami; systemctl is-active hanako-server.service"
```

Current web root:

```text
/var/www/hanako-local-bridge-releases/alpha
```

Before writing:

- inspect current Alpha manifest;
- inspect stable manifest;
- record stable hash;
- create a timestamped backup under
  `/opt/openhanako-migration/backups/`;
- upload to a private `/tmp` staging directory;
- verify staging hashes.

Publish order:

1. versioned ZIP;
2. versioned installer;
3. versioned manifest;
4. current Alpha manifest last, using a temporary file plus atomic `mv`.

Preserve the old current manifest as:

```text
update-manifest-<previous-version>.json
```

No Nginx reload or Hana restart is required for static release files. Verify:

- Alpha public manifest reports target version/channel/hash/size;
- package exists and hash matches;
- the explicit remote download probe succeeds from the Windows release machine;
- stable public manifest version and hash are unchanged;
- `hanako-server.service` remains active.

## Local Online Update

Before updating the real installation:

```text
%LOCALAPPDATA%\HanakoLocalBridge
```

Create a timestamped backup of at least `config.json` and `data\`. Record the config SHA256.

Check the manager update endpoint using the approval token internally, without printing it. Trigger
the target version once. The request may time out while the old Bridge exits.

Wait for:

```text
data\update-state.json status = succeeded
installedVersion = target version
8787 health ok
8788 health ok
cloud.status = active
config hash unchanged
desktop and Start menu shortcuts target hanako-bridge.exe
uninstall DisplayIcon targets hanako-bridge.exe
uninstall DisplayVersion equals target version
```

Export the scheduled task and verify `TimeTrigger`, `PT1M` and `IgnoreNew`. Confirm the Bridge
process has `MainWindowHandle=0`.

For lifecycle changes, force-terminate only the installed Bridge and wait for a different PID.
Repeat both health checks and cloud status. Close the Manager and confirm Bridge remains healthy.

## Rollback

### Feed rollback

Restore the previous Alpha manifest from the timestamped backup or versioned manifest. Do not remove
already published versioned packages during an incident; keeping them makes rollback and audit
deterministic.

### Windows rollback

Use a previously signed installer or signed manifest. Preserve current `config.json`, `data` and
`logs`. Verify the older version still understands any new persistent fields before rolling back.

### Failed update transaction

Inspect:

```text
data\update-state.json
logs\update.log
payload-manifest.json
```

The updater should restore managed payload files automatically after replacement failure. Do not
manually copy random EXEs into the install directory while a worker may still be active.

## Operation Record

For each VPS publication, save a sanitized record under:

```text
/opt/openhanako-migration/operations/
```

Include:

- time, host, source commit and tag;
- artifact names, sizes and SHA256;
- destination and backup path;
- stable manifest before/after;
- service restart status;
- public feed verification;
- local update and runtime verification;
- remaining caveats.

Never include credentials or token contents.
