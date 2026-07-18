param(
  [string]$PackagePath = "",
  [string]$InstallDir = "",
  [string]$MigrateFrom = "",
  [string]$TaskPrefix = "",
  [string]$DeviceId = "",
  [string]$DeviceName = "",
  [string]$RootPath = "",
  [string]$VpsHost = "",
  [string]$CloudUrl = "",
  [string]$SshUser = "",
  [string]$IdentityFile = "",
  [int]$McpPort = 0,
  [int]$ApprovalPort = 0,
  [int]$RemotePort = 0,
  [switch]$DisableTunnel,
  [switch]$Gui,
  [switch]$NonInteractive,
  [switch]$SkipStart,
  [switch]$NoMigrate,
  [switch]$SkipUninstallRegistration
)

$ErrorActionPreference = "Stop"
$script:InstallerGui = [bool]$Gui

trap {
  $message = $_.Exception.Message
  if ($script:InstallerGui) {
    try {
      Add-Type -AssemblyName System.Windows.Forms
      [System.Windows.Forms.MessageBox]::Show(
        $message,
        "Hanako Local Bridge installation failed",
        [System.Windows.Forms.MessageBoxButtons]::OK,
        [System.Windows.Forms.MessageBoxIcon]::Error
      ) | Out-Null
    } catch {}
  }
  [Console]::Error.WriteLine($message)
  exit 1
}

function Get-EnvironmentFlag {
  param([string]$Name)
  $value = [Environment]::GetEnvironmentVariable($Name)
  return @("1", "true", "yes", "on") -contains ([string]$value).Trim().ToLowerInvariant()
}

function Resolve-OptionalInteger {
  param(
    [int]$Current,
    [string]$EnvironmentName
  )
  if ($Current -gt 0) { return $Current }
  $raw = [Environment]::GetEnvironmentVariable($EnvironmentName)
  if ($raw -match "^\d+$") { return [int]$raw }
  return 0
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
  $InstallDir = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_INSTALL_DIR")
}
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
  $InstallDir = Join-Path $env:LOCALAPPDATA "HanakoLocalBridge"
}
if ([string]::IsNullOrWhiteSpace($PackagePath)) {
  $PackagePath = Join-Path $PSScriptRoot "payload.zip"
}
if ([string]::IsNullOrWhiteSpace($TaskPrefix)) {
  $TaskPrefix = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_TASK_PREFIX")
}
if ([string]::IsNullOrWhiteSpace($DeviceId)) {
  $DeviceId = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_DEVICE_ID")
}
if ([string]::IsNullOrWhiteSpace($DeviceName)) {
  $DeviceName = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_DEVICE_NAME")
}
if ([string]::IsNullOrWhiteSpace($RootPath)) {
  $RootPath = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_ROOT_PATH")
}
if ([string]::IsNullOrWhiteSpace($VpsHost)) {
  $VpsHost = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_VPS_HOST")
}
if ([string]::IsNullOrWhiteSpace($SshUser)) {
  $SshUser = [Environment]::GetEnvironmentVariable("HANA_BRIDGE_SSH_USER")
}
$McpPort = Resolve-OptionalInteger -Current $McpPort -EnvironmentName "HANA_BRIDGE_MCP_PORT"
$ApprovalPort = Resolve-OptionalInteger -Current $ApprovalPort -EnvironmentName "HANA_BRIDGE_APPROVAL_PORT"
$RemotePort = Resolve-OptionalInteger -Current $RemotePort -EnvironmentName "HANA_BRIDGE_REMOTE_PORT"
if (Get-EnvironmentFlag -Name "HANA_BRIDGE_NONINTERACTIVE") { $NonInteractive = $true }
if (Get-EnvironmentFlag -Name "HANA_BRIDGE_SKIP_START") { $SkipStart = $true }
if (Get-EnvironmentFlag -Name "HANA_BRIDGE_DISABLE_TUNNEL") { $DisableTunnel = $true }
if (Get-EnvironmentFlag -Name "HANA_BRIDGE_NO_MIGRATE") { $NoMigrate = $true }
if (Get-EnvironmentFlag -Name "HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION") {
  $SkipUninstallRegistration = $true
}

$installDir = [System.IO.Path]::GetFullPath($InstallDir)
$packagePath = [System.IO.Path]::GetFullPath($PackagePath)
if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
  throw "Installer payload not found: $packagePath"
}

if ($Gui -and -not $NonInteractive) {
  $uiScript = Join-Path $PSScriptRoot "configuration-ui.ps1"
  if (-not (Test-Path -LiteralPath $uiScript -PathType Leaf)) {
    throw "Installer configuration UI is missing."
  }
  $uiArguments = @{
    InstallRoot = $installDir
    ConfigPath = (Join-Path $installDir "config.json")
    CollectOnly = $true
  }
  foreach ($entry in @{
    DeviceId = $DeviceId
    DeviceName = $DeviceName
    RootPath = $RootPath
    VpsHost = $VpsHost
    CloudUrl = $CloudUrl
    SshUser = $SshUser
    IdentityFile = $IdentityFile
    TaskPrefix = $TaskPrefix
  }.GetEnumerator()) {
    if (-not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
      $uiArguments[$entry.Key] = $entry.Value
    }
  }
  if ($McpPort -gt 0) { $uiArguments.McpPort = $McpPort }
  if ($ApprovalPort -gt 0) { $uiArguments.ApprovalPort = $ApprovalPort }
  if ($RemotePort -gt 0) { $uiArguments.RemotePort = $RemotePort }
  if ($DisableTunnel) { $uiArguments.DisableTunnel = $true }

  $uiResult = & $uiScript @uiArguments
  if ($uiResult.Cancelled) {
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.MessageBox]::Show(
      "Installation was cancelled. No service changes were made.",
      "Hanako Local Bridge",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Information
    ) | Out-Null
    exit 0
  }
  $DeviceId = [string]$uiResult.DeviceId
  $DeviceName = [string]$uiResult.DeviceName
  $RootPath = [string]$uiResult.RootPath
  $VpsHost = [string]$uiResult.VpsHost
  $CloudUrl = [string]$uiResult.CloudUrl
  $SshUser = [string]$uiResult.SshUser
  $IdentityFile = [string]$uiResult.IdentityFile
  $TaskPrefix = [string]$uiResult.TaskPrefix
  $McpPort = [int]$uiResult.McpPort
  $ApprovalPort = [int]$uiResult.ApprovalPort
  $RemotePort = [int]$uiResult.RemotePort
  $DisableTunnel = -not [bool]$uiResult.TunnelEnabled
  $NonInteractive = $true
}

$effectivePrefix = if ([string]::IsNullOrWhiteSpace($TaskPrefix)) { "Hanako Local FS" } else { $TaskPrefix.Trim() }
$taskNames = @("$effectivePrefix MCP", "$effectivePrefix Tunnel")
$migrationSource = $null

if (-not $NoMigrate -and -not [string]::IsNullOrWhiteSpace($MigrateFrom)) {
  $candidate = [System.IO.Path]::GetFullPath($MigrateFrom)
  if (Test-Path -LiteralPath (Join-Path $candidate "server.cjs") -PathType Leaf) {
    $migrationSource = $candidate
  }
}

if (-not $NoMigrate -and -not $migrationSource -and $effectivePrefix -eq "Hanako Local FS") {
  foreach ($taskName in $taskNames) {
    $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    foreach ($action in @($task.Actions)) {
      if ([string]$action.Arguments -match '"([^"]+)\\run-(?:local-fs|reverse-tunnel)-hidden\.vbs"') {
        $candidate = Split-Path -Parent $Matches[1]
        if (
          $candidate -and
          ([System.IO.Path]::GetFullPath($candidate) -ne $installDir) -and
          (Test-Path -LiteralPath (Join-Path $candidate "server.cjs") -PathType Leaf)
        ) {
          $migrationSource = [System.IO.Path]::GetFullPath($candidate)
          break
        }
      }
    }
    if ($migrationSource) { break }
  }
}

if (-not $NoMigrate -and -not $migrationSource -and $effectivePrefix -eq "Hanako Local FS") {
  $legacyDesktopRoot = Join-Path $env:USERPROFILE "Desktop\Hanako-Local-FS-MCP-Bridge"
  if (
    ([System.IO.Path]::GetFullPath($legacyDesktopRoot) -ne $installDir) -and
    (Test-Path -LiteralPath (Join-Path $legacyDesktopRoot "server.cjs") -PathType Leaf)
  ) {
    $migrationSource = [System.IO.Path]::GetFullPath($legacyDesktopRoot)
  }
}

foreach ($taskName in $taskNames) {
  Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
}
foreach ($source in @($installDir, $migrationSource) | Where-Object { $_ } | Select-Object -Unique) {
  $stopScript = Join-Path $source "stop.ps1"
  if (Test-Path -LiteralPath $stopScript -PathType Leaf) {
    try {
      & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $stopScript -KeepTasks
    } catch {}
  }
}
if ($migrationSource) {
  $forwardLocalPort = if ($McpPort -gt 0) { $McpPort } else { 8787 }
  $forwardRemotePort = if ($RemotePort -gt 0) { $RemotePort } else { 18787 }
  $forwardMarker = "127.0.0.1:${forwardRemotePort}:127.0.0.1:${forwardLocalPort}"
  $currentPid = $PID
  Get-CimInstance Win32_Process |
    Where-Object {
      $_.ProcessId -ne $currentPid -and
      -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and (
        $_.CommandLine.IndexOf($migrationSource, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $_.CommandLine.IndexOf($forwardMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
      )
    } |
    Sort-Object ProcessId -Descending |
    ForEach-Object {
      Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    }
  Start-Sleep -Seconds 2
}

$installedManagerRoot = [System.IO.Path]::GetFullPath((Join-Path $installDir "manager")).TrimEnd("\") + "\"
$managerDeadline = (Get-Date).AddSeconds(30)
$emptyManagerChecks = 0
do {
  $managerProcesses = @(
    Get-CimInstance Win32_Process -Filter "Name = 'HanakoBridgeManager.exe'" -ErrorAction SilentlyContinue |
      Where-Object {
        $executableMatches =
          -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
          [System.IO.Path]::GetFullPath($_.ExecutablePath).StartsWith(
            $installedManagerRoot,
            [System.StringComparison]::OrdinalIgnoreCase
          )
        $commandMatches =
          -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and
          ([string]$_.CommandLine).IndexOf(
            $installedManagerRoot,
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
  throw "HanakoBridgeManager.exe did not exit before installation."
}

$stage = Join-Path $env:TEMP "HanakoLocalBridgeInstall-$PID-$([Guid]::NewGuid().ToString('N'))"
try {
  New-Item -ItemType Directory -Force -Path $stage | Out-Null
  Expand-Archive -LiteralPath $packagePath -DestinationPath $stage -Force
  foreach ($required in @(
    "server.cjs",
    "package.json",
    "bridge-common.ps1",
    "manager-core.ps1",
    "manager-command.ps1",
    "manager-ui.ps1",
    "run-manager.vbs",
    "manager\HanakoBridgeManager.exe",
    "runtime\node.exe"
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $stage $required))) {
      throw "Installer payload is invalid; missing $required"
    }
  }

  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  if ($migrationSource -and $migrationSource -ne $installDir) {
    $migrationBackup = Join-Path $installDir "migration-backup-$((Get-Date).ToString('yyyyMMdd-HHmmss'))"
    $hasExistingState = $false
    foreach ($persistentName in @("config.json", "data", "logs")) {
      if (Test-Path -LiteralPath (Join-Path $installDir $persistentName)) {
        $hasExistingState = $true
        break
      }
    }
    if ($hasExistingState) {
      New-Item -ItemType Directory -Force -Path $migrationBackup | Out-Null
      foreach ($persistentName in @("config.json", "data", "logs")) {
        $existingPath = Join-Path $installDir $persistentName
        if (Test-Path -LiteralPath $existingPath) {
          Copy-Item -LiteralPath $existingPath -Destination $migrationBackup -Recurse -Force
        }
      }
    }

    $sourceConfig = Join-Path $migrationSource "config.json"
    if (Test-Path -LiteralPath $sourceConfig -PathType Leaf) {
      Copy-Item -LiteralPath $sourceConfig -Destination (Join-Path $installDir "config.json") -Force
    }
    foreach ($persistentName in @("data", "logs")) {
      $sourcePath = Join-Path $migrationSource $persistentName
      $destinationPath = Join-Path $installDir $persistentName
      if (Test-Path -LiteralPath $sourcePath -PathType Container) {
        New-Item -ItemType Directory -Force -Path $destinationPath | Out-Null
        Get-ChildItem -LiteralPath $sourcePath -Force | ForEach-Object {
          Copy-Item -LiteralPath $_.FullName -Destination $destinationPath -Recurse -Force
        }
      }
    }
  }

  Get-ChildItem -LiteralPath $stage -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $installDir -Recurse -Force
  }
  Remove-Item `
    -LiteralPath (Join-Path $installDir "CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md") `
    -Force `
    -ErrorAction SilentlyContinue

  $configureArguments = @{
    InstallRoot = $installDir
    ConfigPath = (Join-Path $installDir "config.json")
  }
  foreach ($entry in @{
    DeviceId = $DeviceId
    DeviceName = $DeviceName
    RootPath = $RootPath
    VpsHost = $VpsHost
    CloudUrl = $CloudUrl
    SshUser = $SshUser
    IdentityFile = $IdentityFile
    TaskPrefix = $effectivePrefix
  }.GetEnumerator()) {
    if (-not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
      $configureArguments[$entry.Key] = $entry.Value
    }
  }
  if ($McpPort -gt 0) { $configureArguments.McpPort = $McpPort }
  if ($ApprovalPort -gt 0) { $configureArguments.ApprovalPort = $ApprovalPort }
  if ($RemotePort -gt 0) { $configureArguments.RemotePort = $RemotePort }
  if ($DisableTunnel) { $configureArguments.DisableTunnel = $true }
  if ($uiResult -and [bool]$uiResult.TunnelEnabled) {
    $configureArguments.DisableCloud = $true
    $configureArguments.UseLegacySshTunnel = $true
  }
  if ($NonInteractive) { $configureArguments.NonInteractive = $true }
  & (Join-Path $installDir "configure.ps1") @configureArguments

  $serviceArguments = @{
    ConfigPath = (Join-Path $installDir "config.json")
    NonInteractive = $true
  }
  if ($SkipStart) { $serviceArguments.SkipStart = $true }
  & (Join-Path $installDir "install-background-service.ps1") @serviceArguments

  if (-not $SkipUninstallRegistration) {
    $package = Get-Content -LiteralPath (Join-Path $installDir "package.json") -Raw | ConvertFrom-Json
    $uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge"
    New-Item -Path $uninstallKey -Force | Out-Null
    $uninstallCommand = "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File `"$installDir\uninstall-background-service.ps1`" -RemoveInstall -KeepData"
    New-ItemProperty -Path $uninstallKey -Name DisplayName -Value "Hanako Local Bridge" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name DisplayVersion -Value ([string]$package.version) -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name Publisher -Value "Hanako Local Bridge" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name InstallLocation -Value $installDir -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name UninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name QuietUninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null

    $startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "Hanako Local Bridge"
    New-Item -ItemType Directory -Force -Path $startMenuDir | Out-Null
    $shortcutPath = Join-Path $startMenuDir "Hanako Local Bridge Manager.lnk"
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
    $shortcut.TargetPath = Join-Path $env:WINDIR "System32\wscript.exe"
    $shortcut.Arguments = "//B //NoLogo `"$installDir\run-manager.vbs`""
    $shortcut.WorkingDirectory = $installDir
    $shortcut.Description = "Manage, diagnose, repair, and claim Hanako Local Bridge devices"
    $shortcut.Save()
  }

  Write-Host ""
  Write-Host "Hanako Local Bridge installed successfully."
  Write-Host "Install directory: $installDir"
  if ($migrationSource) { Write-Host "Migrated persistent state from: $migrationSource" }
  Write-Host "Use status.ps1 to inspect the background service."
  Write-Host "Use Hanako Local Bridge Manager from the Start menu for graphical diagnostics."
  if ($Gui) {
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.MessageBox]::Show(
      "Hanako Local Bridge was installed successfully.`n`nInstall directory:`n$installDir",
      "Hanako Local Bridge",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Information
    ) | Out-Null
  }
} finally {
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedStage = [System.IO.Path]::GetFullPath($stage)
  if ($resolvedStage.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedStage -Recurse -Force -ErrorAction SilentlyContinue
  }
}
