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
  Write-Host "manager command tests passed"
} finally {
  Remove-Item Env:HANA_MANAGER_ACTION -ErrorAction SilentlyContinue
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
