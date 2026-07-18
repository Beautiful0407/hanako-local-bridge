param(
  [string]$Manifest = "",
  [switch]$Force,
  [switch]$SkipStart,
  [switch]$RememberManifest
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

function Get-TextResource {
  param([string]$Source)
  if ($Source -match "^https?://") {
    return (Invoke-WebRequest -UseBasicParsing -Uri $Source -TimeoutSec 30).Content
  }
  return Get-Content -LiteralPath ([System.IO.Path]::GetFullPath($Source)) -Raw
}

function Resolve-Resource {
  param(
    [string]$Base,
    [string]$Reference
  )
  if ($Reference -match "^https?://") { return $Reference }
  if ($Base -match "^https?://") {
    return ([Uri]::new([Uri]$Base, $Reference)).AbsoluteUri
  }
  if ([System.IO.Path]::IsPathRooted($Reference)) {
    return [System.IO.Path]::GetFullPath($Reference)
  }
  return [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $Base) $Reference))
}

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot
$config = $runtime.config
if ([string]::IsNullOrWhiteSpace($Manifest)) {
  $Manifest = [string]$config.update.manifest
}
if ([string]::IsNullOrWhiteSpace($Manifest)) {
  throw "No update manifest configured. Pass -Manifest <path-or-url>."
}
if ($Manifest -notmatch "^https?://") {
  $Manifest = [System.IO.Path]::GetFullPath($Manifest)
}

$manifestData = Get-TextResource -Source $Manifest | ConvertFrom-Json
$currentPackage = Get-Content -LiteralPath (Join-Path $installRoot "package.json") -Raw | ConvertFrom-Json
$currentVersion = [version]$currentPackage.version
$targetVersion = [version]$manifestData.version
if (-not $Force -and $targetVersion -le $currentVersion) {
  Write-Host "Already up to date: $currentVersion"
  exit 0
}

$packageSource = Resolve-Resource -Base $Manifest -Reference ([string]$manifestData.packageUrl)
$tempRoot = Join-Path $env:TEMP "HanakoLocalBridgeUpdate-$PID-$([Guid]::NewGuid().ToString('N'))"
$packageFile = Join-Path $tempRoot "package.zip"
$stage = Join-Path $tempRoot "payload"

try {
  New-Item -ItemType Directory -Force -Path $tempRoot, $stage | Out-Null
  if ($packageSource -match "^https?://") {
    Invoke-WebRequest -UseBasicParsing -Uri $packageSource -OutFile $packageFile -TimeoutSec 120
  } else {
    Copy-Item -LiteralPath $packageSource -Destination $packageFile -Force
  }

  $actualHash = (Get-FileHash -LiteralPath $packageFile -Algorithm SHA256).Hash.ToLowerInvariant()
  $expectedHash = ([string]$manifestData.sha256).Trim().ToLowerInvariant()
  if ($expectedHash -and $actualHash -ne $expectedHash) {
    throw "Update package SHA256 mismatch. Expected $expectedHash, got $actualHash."
  }

  Expand-Archive -LiteralPath $packageFile -DestinationPath $stage -Force
  $stagedPackageFile = Join-Path $stage "package.json"
  if (-not (Test-Path -LiteralPath $stagedPackageFile -PathType Leaf)) {
    throw "Update package is missing package.json."
  }
  $stagedPackage = Get-Content -LiteralPath $stagedPackageFile -Raw | ConvertFrom-Json
  if ([version]$stagedPackage.version -ne $targetVersion) {
    throw "Manifest version $targetVersion does not match package version $($stagedPackage.version)."
  }

  $tasks = Get-BridgeTaskNames -Runtime $runtime
  foreach ($taskName in @($tasks.Mcp, $tasks.Tunnel)) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  }
  Stop-BridgeProcesses -InstallRoot $installRoot -Runtime $runtime
  $managerRoot = [System.IO.Path]::GetFullPath((Join-Path $installRoot "manager")).TrimEnd("\") + "\"
  $managerDeadline = (Get-Date).AddSeconds(30)
  $emptyManagerChecks = 0
  do {
    $managerProcesses = @(
      Get-CimInstance Win32_Process -Filter "Name = 'HanakoBridgeManager.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
          $executableMatches =
            -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
            [System.IO.Path]::GetFullPath($_.ExecutablePath).StartsWith(
              $managerRoot,
              [System.StringComparison]::OrdinalIgnoreCase
            )
          $commandMatches =
            -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and
            ([string]$_.CommandLine).IndexOf(
              $managerRoot,
              [System.StringComparison]::OrdinalIgnoreCase
            ) -ge 0
          $executableMatches -or $commandMatches
        }
    )
    if ($managerProcesses.Count -eq 0) {
      $emptyManagerChecks++
    } else {
      $emptyManagerChecks = 0
      foreach ($managerProcess in $managerProcesses) {
        Stop-Process -Id $managerProcess.ProcessId -Force -ErrorAction SilentlyContinue
      }
    }
    Start-Sleep -Milliseconds 350
  } while ((Get-Date) -lt $managerDeadline -and $emptyManagerChecks -lt 3)
  if ($emptyManagerChecks -lt 3) {
    throw "HanakoBridgeManager.exe did not exit before update."
  }

  Get-ChildItem -LiteralPath $stage -Force | ForEach-Object {
    if ($_.Name -notin @("config.json", "data", "logs")) {
      Copy-Item -LiteralPath $_.FullName -Destination $installRoot -Recurse -Force
    }
  }
  Remove-Item `
    -LiteralPath (Join-Path $installRoot "CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md") `
    -Force `
    -ErrorAction SilentlyContinue

  if ($RememberManifest) {
    $updatedRuntime = Get-BridgeRuntime -InstallRoot $installRoot
    $updatedRuntime.config.update.manifest = $Manifest
    Write-BridgeJson -Value $updatedRuntime.config -Path $updatedRuntime.configPath
  }

  $serviceArguments = @{ NonInteractive = $true }
  if ($SkipStart) { $serviceArguments.SkipStart = $true }
  & (Join-Path $installRoot "install-background-service.ps1") @serviceArguments
  Write-Host "Updated Hanako Local Bridge: $currentVersion -> $targetVersion"
} catch {
  try {
    & (Join-Path $installRoot "repair.ps1") -NonInteractive
  } catch {}
  throw
} finally {
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedRoot = [System.IO.Path]::GetFullPath($tempRoot)
  if ($resolvedRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
