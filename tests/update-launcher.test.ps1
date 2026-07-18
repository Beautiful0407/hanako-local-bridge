$ErrorActionPreference = "Stop"

function Assert-UpdateLauncher {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

function Write-TestPackageVersion {
  param(
    [string]$Root,
    [string]$Version
  )

  [System.IO.File]::WriteAllText(
    (Join-Path $Root "package.json"),
    (@{ name = "update-launcher-test"; version = $Version } | ConvertTo-Json),
    [System.Text.UTF8Encoding]::new($false)
  )
}

function Wait-UpdateState {
  param(
    [string]$Path,
    [int]$TimeoutSeconds = 15
  )

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  do {
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
      try {
        $state = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
        if ([string]$state.status -in @("succeeded", "failed")) {
          return $state
        }
      } catch {}
    }
    Start-Sleep -Milliseconds 100
  } while ((Get-Date) -lt $deadline)
  throw "Timed out waiting for final update state: $Path"
}

function Wait-RestartMarker {
  param(
    [string]$Path,
    [int]$TimeoutSeconds = 10
  )

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  do {
    if (Test-Path -LiteralPath $Path -PathType Leaf) { return }
    Start-Sleep -Milliseconds 100
  } while ((Get-Date) -lt $deadline)
  throw "Timed out waiting for manager restart marker."
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$testRoot = Join-Path $env:TEMP "HanakoUpdateLauncher-$([Guid]::NewGuid().ToString('N'))"
$statePath = Join-Path $testRoot "data\update-state.json"
$restartMarker = Join-Path $testRoot "manager-restarted.txt"

try {
  New-Item -ItemType Directory -Force -Path `
    (Join-Path $testRoot "data"), `
    (Join-Path $testRoot "logs") | Out-Null
  Copy-Item -LiteralPath (Join-Path $projectRoot "update-and-restart.ps1") -Destination $testRoot

  [System.IO.File]::WriteAllText(
    (Join-Path $testRoot "update.ps1"),
    @'
param(
  [string]$Manifest = "",
  [string]$TargetRoot = ""
)

$mode = (Get-Content -LiteralPath (Join-Path $PSScriptRoot "update-mode.txt") -Raw).Trim()
if ($mode -eq "fail") {
  [Console]::Error.WriteLine("simulated update failure")
  exit 7
}
$nextVersion = (Get-Content -LiteralPath (Join-Path $PSScriptRoot "next-version.txt") -Raw).Trim()
[System.IO.File]::WriteAllText(
  (Join-Path $TargetRoot "package.json"),
  (@{ name = "update-launcher-test"; version = $nextVersion } | ConvertTo-Json),
  [System.Text.UTF8Encoding]::new($false)
)
exit 0
'@,
    [System.Text.UTF8Encoding]::new($false)
  )
  [System.IO.File]::WriteAllText(
    (Join-Path $testRoot "open-manager.ps1"),
    @'
param([string]$InstallRoot = $PSScriptRoot)
[System.IO.File]::WriteAllText(
  (Join-Path $InstallRoot "manager-restarted.txt"),
  (Get-Date).ToUniversalTime().ToString("o"),
  [System.Text.UTF8Encoding]::new($false)
)
'@,
    [System.Text.UTF8Encoding]::new($false)
  )

  . (Join-Path $projectRoot "manager-core.ps1")

  Write-TestPackageVersion -Root $testRoot -Version "1.4.8"
  [System.IO.File]::WriteAllText((Join-Path $testRoot "update-mode.txt"), "success")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "next-version.txt"), "1.4.9")
  $launch = Start-HanakoBridgeUpdate `
    -InstallRoot $testRoot `
    -Manifest (Join-Path $testRoot "manifest.json") `
    -ExpectedVersion "1.4.9" `
    -StatePath $statePath
  Assert-UpdateLauncher ($launch.started -eq $true) "Successful updater did not confirm handoff."
  $success = Wait-UpdateState -Path $statePath
  Wait-RestartMarker -Path $restartMarker
  Assert-UpdateLauncher ($success.status -eq "succeeded") "Successful updater did not persist success."
  Assert-UpdateLauncher ($success.installedVersion -eq "1.4.9") "Successful updater persisted the wrong version."
  Assert-UpdateLauncher ($success.exitCode -eq 0) "Successful updater persisted a non-zero exit code."
  $consumed = Get-HanakoBridgeUpdateResult -InstallRoot $testRoot -StatePath $statePath -Consume
  Assert-UpdateLauncher ($consumed.present -eq $true) "Successful update result was not readable."
  Assert-UpdateLauncher (-not (Test-Path -LiteralPath $statePath)) "Consumed update result was not removed."

  Remove-Item -LiteralPath $restartMarker -Force -ErrorAction SilentlyContinue
  [System.IO.File]::WriteAllText((Join-Path $testRoot "update-mode.txt"), "fail")
  $failedLaunch = Start-HanakoBridgeUpdate `
    -InstallRoot $testRoot `
    -Manifest (Join-Path $testRoot "manifest.json") `
    -ExpectedVersion "1.5.0" `
    -StatePath $statePath
  Assert-UpdateLauncher ($failedLaunch.started -eq $true) "Failed updater did not confirm handoff."
  $failure = Wait-UpdateState -Path $statePath
  Wait-RestartMarker -Path $restartMarker
  Assert-UpdateLauncher ($failure.status -eq "failed") "Failed updater did not persist failure."
  Assert-UpdateLauncher ($failure.exitCode -eq 7) "Failed updater did not preserve the child exit code."
  Assert-UpdateLauncher ($failure.message -match "code 7") "Failed updater message omitted the exit code."

  Remove-Item -LiteralPath $restartMarker -Force -ErrorAction SilentlyContinue
  [System.IO.File]::WriteAllText((Join-Path $testRoot "update-mode.txt"), "success")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "next-version.txt"), "1.4.9")
  Start-HanakoBridgeUpdate `
    -InstallRoot $testRoot `
    -Manifest (Join-Path $testRoot "manifest.json") `
    -ExpectedVersion "1.5.0" `
    -StatePath $statePath | Out-Null
  $mismatch = Wait-UpdateState -Path $statePath
  Wait-RestartMarker -Path $restartMarker
  Assert-UpdateLauncher ($mismatch.status -eq "failed") "Version mismatch was reported as success."
  Assert-UpdateLauncher ($mismatch.message -match "Expected version 1.5.0") "Version mismatch message omitted expected version."

  Write-Host "update launcher tests passed"
} finally {
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
