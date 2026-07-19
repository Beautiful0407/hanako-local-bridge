[CmdletBinding()]
param(
  [string]$RepoRoot = "",
  [ValidateSet(0, 1)]
  [int]$IncludeLocalRuntime = 1,
  [string]$CapturedAt = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-RepoRoot {
  param([string]$RequestedRoot)

  if (-not [string]::IsNullOrWhiteSpace($RequestedRoot)) {
    return [System.IO.Path]::GetFullPath($RequestedRoot)
  }
  return [System.IO.Path]::GetFullPath(
    (Join-Path $PSScriptRoot "..\..\..")
  )
}

function Invoke-ToolVersion {
  param(
    [string]$CommandName,
    [string[]]$Arguments
  )

  $command = Get-Command $CommandName -ErrorAction SilentlyContinue
  if (-not $command) {
    return $null
  }
  try {
    $output = @(& $command.Source @Arguments 2>&1)
    return [string]($output | Select-Object -First 1)
  } catch {
    return "error: $($_.Exception.Message)"
  }
}

function Invoke-GitText {
  param(
    [string]$Root,
    [string[]]$Arguments
  )

  try {
    $output = @(& git -C $Root @Arguments 2>$null)
    if ($LASTEXITCODE -ne 0) {
      return $null
    }
    return ($output -join "`n").Trim()
  } catch {
    return $null
  }
}

function Get-WorkspaceVersion {
  param([string]$Root)

  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    return $null
  }
  try {
    Push-Location $Root
    try {
      $metadata = (& cargo metadata --no-deps --format-version 1 2>$null) |
        ConvertFrom-Json
    } finally {
      Pop-Location
    }
    $bridge = @($metadata.packages | Where-Object name -eq "hanako-bridge") |
      Select-Object -First 1
    return [string]$bridge.version
  } catch {
    return $null
  }
}

function Get-StableVersion {
  param([string]$Root)

  $packagePath = Join-Path $Root "package.json"
  if (-not (Test-Path -LiteralPath $packagePath)) {
    return $null
  }
  try {
    return [string](
      Get-Content -LiteralPath $packagePath -Raw -Encoding utf8 |
        ConvertFrom-Json
    ).version
  } catch {
    return $null
  }
}

function Get-ReleaseArtifacts {
  param([string]$Root)

  $buildRoot = Join-Path $Root "build"
  if (-not (Test-Path -LiteralPath $buildRoot)) {
    return @()
  }
  $releaseDir = Get-ChildItem -LiteralPath $buildRoot -Directory -ErrorAction SilentlyContinue |
    Where-Object Name -like "rust-release-*" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
  if (-not $releaseDir) {
    return @()
  }
  return @(
    Get-ChildItem -LiteralPath $releaseDir.FullName -File -ErrorAction SilentlyContinue |
      Where-Object Extension -in ".zip", ".exe", ".json" |
      Sort-Object Name |
      ForEach-Object {
        [ordered]@{
          name = $_.Name
          length = $_.Length
          sha256 = (
            Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
          ).Hash.ToLowerInvariant()
          path = $_.FullName
        }
      }
  )
}

function Get-LocalRuntime {
  $installRoot = Join-Path $env:LOCALAPPDATA "HanakoLocalBridge"
  $health = $null
  $managerHealth = $null
  try {
    $health = Invoke-RestMethod "http://127.0.0.1:8787/health" -TimeoutSec 3
  } catch {}
  try {
    $managerHealth = Invoke-RestMethod "http://127.0.0.1:8788/health" -TimeoutSec 3
  } catch {}

  $task = Get-ScheduledTask -TaskName "Hanako Local FS MCP" -ErrorAction SilentlyContinue
  $taskInfo = Get-ScheduledTaskInfo -TaskName "Hanako Local FS MCP" -ErrorAction SilentlyContinue
  $taskXml = if ($task) {
    Export-ScheduledTask -TaskName "Hanako Local FS MCP"
  } else {
    ""
  }

  $listeners = @(
    netstat -ano |
      Select-String -Pattern '^\s*TCP\s+127\.0\.0\.1:(8787|8788)\s+.*LISTENING\s+(\d+)' |
      ForEach-Object {
        [ordered]@{
          port = [int]$_.Matches[0].Groups[1].Value
          pid = [int]$_.Matches[0].Groups[2].Value
        }
      }
  )
  $bridgePid = @($listeners | Where-Object port -eq 8787 | Select-Object -First 1).pid
  $bridgeProcess = if ($bridgePid) {
    Get-Process -Id $bridgePid -ErrorAction SilentlyContinue
  } else {
    $null
  }

  return [ordered]@{
    installRoot = $installRoot
    installed = Test-Path -LiteralPath (Join-Path $installRoot "hanako-bridge.exe")
    version = if ($health) { [string]$health.version } else { $null }
    health8787 = [bool]($health -and $health.ok)
    health8788 = [bool]($managerHealth -and $managerHealth.ok)
    cloudStatus = if ($health) { [string]$health.cloud.status } else { $null }
    cloudLastError = if ($health) { [string]$health.cloud.lastError } else { $null }
    cloudLastConnectedAt = if ($health) {
      [string]$health.cloud.lastConnectedAt
    } else {
      $null
    }
    cloudLastSeenAt = if ($health) { [string]$health.cloud.lastSeenAt } else { $null }
    deviceId = if ($health) { [string]$health.device.id } else { $null }
    bridgePid = $bridgePid
    mainWindowHandle = if ($bridgeProcess) {
      [int64]$bridgeProcess.MainWindowHandle
    } else {
      $null
    }
    managerCount = @(Get-Process -Name "hanako-manager" -ErrorAction SilentlyContinue).Count
    listeners = $listeners
    task = [ordered]@{
      exists = [bool]$task
      state = if ($task) { [string]$task.State } else { $null }
      lastTaskResult = if ($taskInfo) { $taskInfo.LastTaskResult } else { $null }
      hidden = if ($task) { [bool]$task.Settings.Hidden } else { $null }
      restartCount = if ($task) { $task.Settings.RestartCount } else { $null }
      restartInterval = if ($task) { [string]$task.Settings.RestartInterval } else { $null }
      timeTrigger = $taskXml -match "<TimeTrigger>"
      minuteInterval = $taskXml -match "<Interval>PT1M</Interval>"
      ignoreNew = $taskXml -match "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"
    }
  }
}

$repo = Resolve-RepoRoot $RepoRoot
if (-not (Test-Path -LiteralPath (Join-Path $repo ".git"))) {
  throw "RepoRoot is not the Hanako Local Bridge Git repository: $repo"
}

$dirty = Invoke-GitText $repo @("status", "--porcelain=v1")
$tag = Invoke-GitText $repo @("describe", "--tags", "--abbrev=0")
$remote = Invoke-GitText $repo @("remote", "get-url", "origin")

$result = [ordered]@{
  capturedAt = if ([string]::IsNullOrWhiteSpace($CapturedAt)) {
    (Get-Date).ToString("o")
  } else {
    $CapturedAt
  }
  repository = [ordered]@{
    root = $repo
    remote = $remote
    branch = Invoke-GitText $repo @("branch", "--show-current")
    commit = Invoke-GitText $repo @("rev-parse", "HEAD")
    shortCommit = Invoke-GitText $repo @("rev-parse", "--short", "HEAD")
    latestTag = $tag
    dirty = -not [string]::IsNullOrWhiteSpace($dirty)
    changes = if ([string]::IsNullOrWhiteSpace($dirty)) { @() } else { @($dirty -split "`n") }
  }
  product = [ordered]@{
    rustVersion = Get-WorkspaceVersion $repo
    stableVersion = Get-StableVersion $repo
  }
  tools = [ordered]@{
    git = Invoke-ToolVersion "git" @("--version")
    cargo = Invoke-ToolVersion "cargo" @("--version")
    rustc = Invoke-ToolVersion "rustc" @("--version")
    node = Invoke-ToolVersion "node" @("--version")
    npm = Invoke-ToolVersion "npm.cmd" @("--version")
    gh = Invoke-ToolVersion "gh" @("--version")
    python = Invoke-ToolVersion "python" @("--version")
  }
  signingKeyPresent = Test-Path -LiteralPath (
    Join-Path $env:USERPROFILE ".hanako-update-signing\private-key.xml"
  )
  artifacts = Get-ReleaseArtifacts $repo
  localRuntime = if ([bool]$IncludeLocalRuntime) { Get-LocalRuntime } else { $null }
}

$result | ConvertTo-Json -Depth 12
