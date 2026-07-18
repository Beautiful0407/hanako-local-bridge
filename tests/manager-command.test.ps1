$ErrorActionPreference = "Stop"

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$testRoot = Join-Path $env:TEMP "HanakoBridgeManagerCommand-$([Guid]::NewGuid().ToString('N'))"
$commandScript = Join-Path $projectRoot "manager-command.ps1"

try {
  New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
  $stub = @'
function Invoke-HanakoBridgeManagerAction {
  param(
    [string]$Action,
    [string]$InstallRoot,
    [string]$ConfigPath
  )

  Write-Host "Stopped HanakoBridgeManager.exe pid=1234"
  [pscustomobject]@{
    ok = $true
    action = $Action
  }
}

function Get-HanakoBridgeUpdateStatus {
  param(
    [string]$InstallRoot,
    [string]$ConfigPath,
    [string]$Manifest
  )

  [pscustomobject]@{
    currentVersion = "1.4.3"
    latestVersion = "1.4.4"
    updateAvailable = $true
    manifest = "https://example.test/update-manifest.json"
    packageUrl = "https://example.test/HanakoLocalBridge-1.4.4-win-x64.zip"
    publishedAt = "2026-07-18T00:00:00Z"
    notes = "manager command update check"
    signatureVerified = $true
  }
}

function Start-HanakoBridgeUpdate {
  param(
    [string]$InstallRoot,
    [string]$Manifest,
    [string]$ExpectedVersion
  )

  [pscustomobject]@{
    started = $true
    attemptId = "manager-command-update-attempt"
    status = "running"
    processId = 4321
    expectedVersion = $ExpectedVersion
    statePath = (Join-Path $InstallRoot "data\update-state.json")
    manifest = $Manifest
  }
}

function Get-HanakoBridgeUpdateResult {
  param(
    [string]$InstallRoot,
    [switch]$Consume
  )

  [pscustomobject]@{
    present = $true
    status = "succeeded"
    attemptId = "manager-command-update-attempt"
    expectedVersion = "1.4.9"
    installedVersion = "1.4.9"
    message = "Update completed successfully."
    logPath = (Join-Path $InstallRoot "logs\update.log")
    startedAt = "2026-07-18T00:00:00Z"
    finishedAt = "2026-07-18T00:01:00Z"
    exitCode = 0
    consumed = [bool]$Consume
  }
}
'@
  [System.IO.File]::WriteAllText(
    (Join-Path $testRoot "manager-core.ps1"),
    $stub,
    [System.Text.UTF8Encoding]::new($false)
  )

  $env:HANA_MANAGER_ACTION = "repair"
  $raw = @(
    & powershell.exe `
      -NoLogo `
      -NoProfile `
      -NonInteractive `
      -ExecutionPolicy Bypass `
      -File $commandScript `
      -Operation action `
      -InstallRoot $testRoot
  )
  if ($LASTEXITCODE -ne 0) {
    throw "manager-command exited with code $LASTEXITCODE."
  }
  if ($raw.Count -ne 1 -or -not ([string]$raw[0]).TrimStart().StartsWith("{")) {
    throw "manager-command emitted non-JSON output: $($raw -join ' | ')"
  }

  $result = ([string]$raw[0]) | ConvertFrom-Json
  if ($result.ok -ne $true -or $result.action -ne "repair") {
    throw "manager-command JSON result was incorrect."
  }

  $updateRaw = @(
    & powershell.exe `
      -NoLogo `
      -NoProfile `
      -NonInteractive `
      -ExecutionPolicy Bypass `
      -File $commandScript `
      -Operation update-check `
      -InstallRoot $testRoot
  )
  if ($LASTEXITCODE -ne 0) {
    throw "manager-command update-check exited with code $LASTEXITCODE."
  }
  $updateResult = ([string]$updateRaw[0]) | ConvertFrom-Json
  if (
    $updateResult.updateAvailable -ne $true -or
    $updateResult.latestVersion -ne "1.4.4" -or
    $updateResult.signatureVerified -ne $true
  ) {
    throw "manager-command update-check JSON result was incorrect."
  }

  $env:HANA_MANAGER_UPDATE_MANIFEST = "https://example.test/update-manifest.json"
  $env:HANA_MANAGER_UPDATE_EXPECTED_VERSION = "1.4.9"
  $launchRaw = @(
    & powershell.exe `
      -NoLogo `
      -NoProfile `
      -NonInteractive `
      -ExecutionPolicy Bypass `
      -File $commandScript `
      -Operation update-launch `
      -InstallRoot $testRoot
  )
  if ($LASTEXITCODE -ne 0) {
    throw "manager-command update-launch exited with code $LASTEXITCODE."
  }
  $launchResult = ([string]$launchRaw[0]) | ConvertFrom-Json
  if (
    $launchResult.started -ne $true -or
    $launchResult.status -ne "running" -or
    $launchResult.expectedVersion -ne "1.4.9"
  ) {
    throw "manager-command update-launch JSON result was incorrect."
  }

  $env:HANA_MANAGER_UPDATE_CONSUME = "1"
  $resultRaw = @(
    & powershell.exe `
      -NoLogo `
      -NoProfile `
      -NonInteractive `
      -ExecutionPolicy Bypass `
      -File $commandScript `
      -Operation update-result `
      -InstallRoot $testRoot
  )
  if ($LASTEXITCODE -ne 0) {
    throw "manager-command update-result exited with code $LASTEXITCODE."
  }
  $updateOutcome = ([string]$resultRaw[0]) | ConvertFrom-Json
  if (
    $updateOutcome.present -ne $true -or
    $updateOutcome.status -ne "succeeded" -or
    $updateOutcome.installedVersion -ne "1.4.9" -or
    $updateOutcome.consumed -ne $true
  ) {
    throw "manager-command update-result JSON result was incorrect."
  }
  Write-Host "manager command tests passed"
} finally {
  Remove-Item Env:HANA_MANAGER_ACTION -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_MANIFEST -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_EXPECTED_VERSION -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_CONSUME -ErrorAction SilentlyContinue
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
