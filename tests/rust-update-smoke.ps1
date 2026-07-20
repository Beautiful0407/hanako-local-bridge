$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$buildRoot = Join-Path $repo "build"
$runId = [Guid]::NewGuid().ToString("N")
$installRoot = Join-Path $buildRoot "rust-update-smoke-$runId"
$installer = Join-Path $buildRoot "rust-release-alpha10\HanakoLocalBridge-Setup-2.0.0-alpha.10.exe"
$alpha9Package = Join-Path $buildRoot "rust-release-alpha9\HanakoLocalBridge-2.0.0-alpha.9-win-x64.zip"
$alpha10Manifest = Join-Path $buildRoot "rust-release-alpha10\update-manifest.json"
$currentMaintenance = Join-Path $repo "target\release\hanako-maintenance.exe"
$passed = $false

function Assert-Path([string]$Path, [string]$Message) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw $Message
  }
}

function Invoke-GuiProcess([string]$FilePath, [string[]]$Arguments) {
  $quoted = $Arguments | ForEach-Object {
    if ($_ -match '[\s"]') {
      '"' + $_.Replace('"', '\"') + '"'
    } else {
      $_
    }
  }
  $process = Start-Process -FilePath $FilePath -ArgumentList ($quoted -join " ") -Wait -PassThru -WindowStyle Hidden
  return $process.ExitCode
}

try {
  Assert-Path $installer "Rust Alpha 10 installer is missing."
  Assert-Path $alpha9Package "Rust Alpha 9 payload is missing."
  Assert-Path $alpha10Manifest "Rust Alpha 10 update manifest is missing."
  Assert-Path $currentMaintenance "Current Rust maintenance binary is missing."
  New-Item -ItemType Directory -Force -Path $installRoot | Out-Null

  $exitCode = Invoke-GuiProcess $installer @(
    "--payload",
    $alpha9Package,
    "--test-mode",
    "--install-root",
    $installRoot
  )
  if ($exitCode -ne 0) {
    throw "Installing the Alpha 9 payload failed with exit code $exitCode."
  }
  $alpha9Payload = Get-Content -LiteralPath (Join-Path $installRoot "payload-manifest.json") -Raw | ConvertFrom-Json
  if ($alpha9Payload.version -ne "2.0.0-alpha.9") {
    throw "The update fixture was not installed at Alpha 9."
  }

  # Exercise the current updater against an older installed payload.
  $maintenance = Join-Path $installRoot "hanako-maintenance.exe"
  $oldMaintenanceHash = (Get-FileHash -LiteralPath $maintenance -Algorithm SHA256).Hash
  Copy-Item -LiteralPath $currentMaintenance -Destination $maintenance -Force
  $currentMaintenanceHash = (Get-FileHash -LiteralPath $currentMaintenance -Algorithm SHA256).Hash
  if ((Get-FileHash -LiteralPath $maintenance -Algorithm SHA256).Hash -ne $currentMaintenanceHash) {
    throw "The current maintenance binary was not injected into the Alpha 9 fixture."
  }
  if ($oldMaintenanceHash -eq $currentMaintenanceHash) {
    throw "The Alpha 9 fixture already contains the current maintenance binary."
  }

  New-Item -ItemType Directory -Force -Path `
    (Join-Path $installRoot "data"), `
    (Join-Path $installRoot "logs") | Out-Null
  Set-Content -LiteralPath (Join-Path $installRoot "data\preserve-data.txt") -Value "keep-data" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $installRoot "logs\preserve-log.txt") -Value "keep-log" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $installRoot "unknown-user-file.txt") -Value "keep-unknown" -Encoding utf8
  $configBefore = Get-FileHash -LiteralPath (Join-Path $installRoot "config.json") -Algorithm SHA256
  $bridgeBefore = Get-FileHash -LiteralPath (Join-Path $installRoot "hanako-bridge.exe") -Algorithm SHA256

  $output = & $maintenance apply `
    --install-root $installRoot `
    --manifest $alpha10Manifest `
    --expected-version "2.0.0-alpha.10" `
    --test-mode
  if ($LASTEXITCODE -ne 0) {
    throw "Alpha 9 maintenance launcher failed with exit code $LASTEXITCODE."
  }
  $handoff = $output | ConvertFrom-Json
  if (-not $handoff.started) {
    throw "Alpha 9 maintenance launcher did not confirm worker handoff."
  }

  $statePath = [string]$handoff.statePath
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  $state = $null
  $lastStateError = ""
  while ([DateTime]::UtcNow -lt $deadline) {
    if (Test-Path -LiteralPath $statePath) {
      try {
        $state = Get-Content -LiteralPath $statePath -Raw -Encoding utf8 | ConvertFrom-Json
        if (@("succeeded", "failed") -contains [string]$state.status) {
          break
        }
      } catch {
        $lastStateError = $_.Exception.Message
      }
    }
    Start-Sleep -Milliseconds 200
  }
  if (-not $state -or $state.status -ne "succeeded") {
    throw "Alpha 9 to Alpha 10 update did not succeed at $statePath. Last read error: $lastStateError. State: $($state | ConvertTo-Json -Compress)"
  }
  if ($state.installedVersion -ne "2.0.0-alpha.10") {
    throw "Update state did not report Alpha 10."
  }

  $payload = Get-Content -LiteralPath (Join-Path $installRoot "payload-manifest.json") -Raw | ConvertFrom-Json
  if ($payload.version -ne "2.0.0-alpha.10") {
    throw "Installed payload did not advance to Alpha 10."
  }
  $bridgeAfter = Get-FileHash -LiteralPath (Join-Path $installRoot "hanako-bridge.exe") -Algorithm SHA256
  if ($bridgeAfter.Hash -eq $bridgeBefore.Hash) {
    throw "The bridge binary was not replaced during the update."
  }
  if ((Get-FileHash -LiteralPath (Join-Path $installRoot "config.json") -Algorithm SHA256).Hash -ne $configBefore.Hash) {
    throw "The update changed config.json."
  }
  foreach ($relative in @(
    "data\preserve-data.txt",
    "logs\preserve-log.txt",
    "unknown-user-file.txt"
  )) {
    Assert-Path (Join-Path $installRoot $relative) "The update removed persistent file $relative."
  }

  $passed = $true
  Write-Output "Rust Alpha 9 to Alpha 10 update smoke test passed"
} finally {
  if ($passed -and (Test-Path -LiteralPath $installRoot)) {
    Remove-Item -LiteralPath $installRoot -Recurse -Force
  }
}
