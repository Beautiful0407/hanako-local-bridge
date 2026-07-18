param(
  [string]$InstallerPath = "",
  [string]$ManifestPath = ""
)

$ErrorActionPreference = "Stop"

function Wait-BridgeHealth {
  param(
    [int]$Port,
    [int]$TimeoutSeconds = 40
  )
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  do {
    try {
      $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:${Port}/health" -TimeoutSec 2
      if ($response.StatusCode -eq 200) {
        return ($response.Content | ConvertFrom-Json)
      }
    } catch {}
    Start-Sleep -Milliseconds 400
  } while ((Get-Date) -lt $deadline)
  throw "Timed out waiting for bridge health on port $Port."
}

function Get-TestNodeProcess {
  param([string]$InstallRoot)
  return Get-CimInstance Win32_Process |
    Where-Object {
      $_.Name -eq "node.exe" -and
      -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and
      $_.CommandLine.IndexOf($InstallRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
      $_.CommandLine -like "*server.cjs*"
    } |
    Select-Object -First 1
}

function Get-TestManagerProcess {
  param([string]$InstallRoot)

  $managerRoot = [System.IO.Path]::GetFullPath((Join-Path $InstallRoot "manager")).TrimEnd("\") + "\"
  return Get-CimInstance Win32_Process -Filter "Name = 'HanakoBridgeManager.exe'" |
    Where-Object {
      -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
      [System.IO.Path]::GetFullPath($_.ExecutablePath).StartsWith(
        $managerRoot,
        [System.StringComparison]::OrdinalIgnoreCase
      )
    } |
    Select-Object -First 1
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$package = Get-Content -LiteralPath (Join-Path $projectRoot "package.json") -Raw | ConvertFrom-Json
if ([string]::IsNullOrWhiteSpace($InstallerPath)) {
  $InstallerPath = Join-Path $projectRoot "release\HanakoLocalBridge-Setup-$($package.version).exe"
}
if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
  $ManifestPath = Join-Path $projectRoot "release\update-manifest.json"
}
$installerPath = [System.IO.Path]::GetFullPath($InstallerPath)
$manifestPath = [System.IO.Path]::GetFullPath($ManifestPath)
$releaseZip = Join-Path $projectRoot "release\HanakoLocalBridge-$($package.version)-win-x64.zip"
if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) { throw "Missing installer: $installerPath" }
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Missing manifest: $manifestPath" }
if (-not (Test-Path -LiteralPath $releaseZip -PathType Leaf)) { throw "Missing package: $releaseZip" }

$testId = [Guid]::NewGuid().ToString("N").Substring(0, 10)
$testRoot = Join-Path $env:TEMP "HanakoLocalBridgeSmoke-$testId"
$localManifestPath = Join-Path $testRoot "local-update-manifest.json"
$localPackagePath = Join-Path $testRoot "package.zip"
$installRoot = Join-Path $testRoot "install"
$fileRoot = Join-Path $testRoot "files"
$taskPrefix = "Hanako Local Bridge Smoke $testId"
$migrationTaskPrefix = "$taskPrefix Migration"
$mcpPort = 42000 + (Get-Random -Minimum 0 -Maximum 1500)
$approvalPort = $mcpPort + 1
$remotePort = 47000 + (Get-Random -Minimum 0 -Maximum 1000)
$productionBefore = Get-ScheduledTask -TaskName "Hanako Local FS MCP", "Hanako Local FS Tunnel" -ErrorAction SilentlyContinue |
  Select-Object TaskName, State
$productionDesktopShortcut = Join-Path ([Environment]::GetFolderPath("Desktop")) "Hanako Local Bridge Manager.lnk"
$productionStartShortcut = Join-Path `
  ([Environment]::GetFolderPath("Programs")) `
  "Hanako Local Bridge\Hanako Local Bridge Manager.lnk"
$productionRegistrationPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge"
$productionShellBefore = [pscustomobject]@{
  desktopShortcut = Test-Path -LiteralPath $productionDesktopShortcut
  startShortcut = Test-Path -LiteralPath $productionStartShortcut
  registration = Get-ItemProperty -LiteralPath $productionRegistrationPath -ErrorAction SilentlyContinue
}

try {
  New-Item -ItemType Directory -Force -Path $fileRoot | Out-Null
  $env:HANA_BRIDGE_INSTALL_DIR = $installRoot
  $env:HANA_BRIDGE_TASK_PREFIX = $taskPrefix
  $env:HANA_BRIDGE_ROOT_PATH = $fileRoot
  $env:HANA_BRIDGE_MCP_PORT = [string]$mcpPort
  $env:HANA_BRIDGE_APPROVAL_PORT = [string]$approvalPort
  $env:HANA_BRIDGE_REMOTE_PORT = [string]$remotePort
  $env:HANA_BRIDGE_NONINTERACTIVE = "1"
  $env:HANA_BRIDGE_SKIP_START = "1"
  $env:HANA_BRIDGE_DISABLE_TUNNEL = "1"
  $env:HANA_BRIDGE_NO_MIGRATE = "1"
  $env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION = "1"

  $installerProcess = Start-Process `
    -FilePath $installerPath `
    -ArgumentList "/Q" `
    -Wait `
    -PassThru
  if ($installerProcess.ExitCode -ne 0) {
    throw "Installer exited with code $($installerProcess.ExitCode)."
  }

  foreach ($required in @(
    "server.cjs",
    "config.json",
    "runtime\node.exe",
    "update.ps1",
    "manager-core.ps1",
    "manager-command.ps1",
    "manager-ui.ps1",
    "manager\HanakoBridgeManager.exe",
    "run-manager.vbs",
    "open-manager.ps1"
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $installRoot $required))) {
      throw "Installed file is missing: $required"
    }
  }
  if (Test-Path -LiteralPath (Join-Path $installRoot "CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md")) {
    throw "Private maintenance manual must not be included in the installer."
  }
  & (Join-Path $installRoot "runtime\node.exe") --version | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "Bundled Node runtime did not start." }
  $managerSmoke = Start-Process `
    -FilePath (Join-Path $installRoot "manager\HanakoBridgeManager.exe") `
    -ArgumentList @("--smoke-test", "--install-root", "`"$installRoot`"") `
    -Wait `
    -PassThru
  if ($managerSmoke.ExitCode -ne 0) {
    throw "Installed WinUI manager smoke test failed with code $($managerSmoke.ExitCode)."
  }
  Start-Process `
    -FilePath (Join-Path $env:WINDIR "System32\wscript.exe") `
    -ArgumentList @("//B", "//NoLogo", "`"$(Join-Path $installRoot 'run-manager.vbs')`"") `
    -WindowStyle Hidden
  $managerDeadline = (Get-Date).AddSeconds(20)
  $managerProcess = $null
  do {
    Start-Sleep -Milliseconds 400
    $managerProcess = Get-TestManagerProcess -InstallRoot $installRoot
  } while ((Get-Date) -lt $managerDeadline -and -not $managerProcess)
  if (-not $managerProcess) {
    throw "Installed manager launcher did not open the WinUI manager."
  }

  $mcpTaskName = "$taskPrefix MCP"
  $tunnelTaskName = "$taskPrefix Tunnel"
  $mcpTask = Get-ScheduledTask -TaskName $mcpTaskName -ErrorAction Stop
  $tunnelTask = Get-ScheduledTask -TaskName $tunnelTaskName -ErrorAction Stop
  if ($mcpTask.Actions.Execute -notlike "*wscript.exe") {
    throw "MCP task does not use hidden wscript launcher."
  }
  if ($tunnelTask.Actions.Execute -notlike "*wscript.exe") {
    throw "Tunnel task does not use hidden wscript launcher."
  }

  Start-ScheduledTask -TaskName $mcpTaskName
  $health = Wait-BridgeHealth -Port $mcpPort
  if ($health.ok -ne $true) { throw "Installed bridge health is not OK." }
  if ($health.configPath -ne (Join-Path $installRoot "config.json")) {
    throw "Installed bridge did not load its install-local config."
  }

  $watchdog = Get-CimInstance Win32_Process |
    Where-Object {
      $_.Name -eq "powershell.exe" -and
      -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and
      $_.CommandLine.IndexOf($installRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
      $_.CommandLine -like "*run-local-fs-service.ps1*"
    } |
    Select-Object -First 1
  if (-not $watchdog -or $watchdog.CommandLine -notlike "*-WindowStyle Hidden*") {
    throw "PowerShell watchdog is not running with WindowStyle Hidden."
  }

  $firstNode = Get-TestNodeProcess -InstallRoot $installRoot
  if (-not $firstNode) { throw "Could not find installed bridge Node process." }
  Stop-Process -Id $firstNode.ProcessId -Force
  $restartDeadline = (Get-Date).AddSeconds(20)
  $secondNode = $null
  do {
    Start-Sleep -Milliseconds 500
    $secondNode = Get-TestNodeProcess -InstallRoot $installRoot
  } while (
    (Get-Date) -lt $restartDeadline -and
    (-not $secondNode -or $secondNode.ProcessId -eq $firstNode.ProcessId)
  )
  if (-not $secondNode -or $secondNode.ProcessId -eq $firstNode.ProcessId) {
    throw "Watchdog did not restart Node after a forced exit."
  }
  Wait-BridgeHealth -Port $mcpPort | Out-Null

  $marker = Join-Path $installRoot "data\installer-smoke-preserved.txt"
  [System.IO.File]::WriteAllText($marker, "preserve-me", [System.Text.UTF8Encoding]::new($false))
  Copy-Item -LiteralPath $releaseZip -Destination $localPackagePath -Force
  $localManifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
  $localManifest.packageUrl = "package.zip"
  $localManifest.signatureAlgorithm = ""
  $localManifest.signature = ""
  [System.IO.File]::WriteAllText(
    $localManifestPath,
    ($localManifest | ConvertTo-Json -Depth 8),
    [System.Text.UTF8Encoding]::new($false)
  )
  & powershell.exe `
    -NoLogo `
    -NoProfile `
    -ExecutionPolicy Bypass `
    -File (Join-Path $installRoot "update.ps1") `
    -Manifest $localManifestPath `
    -Force
  if ($LASTEXITCODE -ne 0) { throw "Local manifest update failed with code $LASTEXITCODE." }
  if ((Get-Content -LiteralPath $marker -Raw) -ne "preserve-me") {
    throw "Persistent data was not preserved during update."
  }
  Wait-BridgeHealth -Port $mcpPort | Out-Null

  Remove-Item Env:HANA_BRIDGE_NO_MIGRATE -ErrorAction SilentlyContinue
  $legacyRoot = Join-Path $testRoot "legacy"
  $migrationInstallRoot = Join-Path $testRoot "migration-install"
  New-Item -ItemType Directory -Force -Path `
    (Join-Path $legacyRoot "data"), `
    (Join-Path $legacyRoot "logs"), `
    (Join-Path $migrationInstallRoot "data") | Out-Null
  [System.IO.File]::WriteAllText((Join-Path $legacyRoot "server.cjs"), "// legacy marker")
  [System.IO.File]::WriteAllText((Join-Path $legacyRoot "data\legacy-state.txt"), "legacy-state")
  [System.IO.File]::WriteAllText((Join-Path $legacyRoot "logs\legacy.log"), "legacy-log")
  [System.IO.File]::WriteAllText((Join-Path $migrationInstallRoot "data\preexisting.txt"), "preexisting")
  & powershell.exe `
    -NoLogo `
    -NoProfile `
    -ExecutionPolicy Bypass `
    -File (Join-Path $projectRoot "installer\bootstrap-install.ps1") `
    -PackagePath $releaseZip `
    -InstallDir $migrationInstallRoot `
    -MigrateFrom $legacyRoot `
    -TaskPrefix $migrationTaskPrefix `
    -RootPath $fileRoot `
    -McpPort ($mcpPort + 10) `
    -ApprovalPort ($approvalPort + 10) `
    -RemotePort ($remotePort + 10) `
    -DisableTunnel `
    -NonInteractive `
    -SkipStart `
    -SkipUninstallRegistration
  if ($LASTEXITCODE -ne 0) { throw "Explicit migration installer test failed with code $LASTEXITCODE." }
  if ((Get-Content -LiteralPath (Join-Path $migrationInstallRoot "data\legacy-state.txt") -Raw) -ne "legacy-state") {
    throw "Legacy persistent state was not migrated over an existing target."
  }
  $migrationBackup = Get-ChildItem -LiteralPath $migrationInstallRoot -Directory -Filter "migration-backup-*" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
  if (-not $migrationBackup -or -not (Test-Path -LiteralPath (Join-Path $migrationBackup.FullName "data\preexisting.txt"))) {
    throw "Preexisting target state was not backed up before migration."
  }

  $productionAfter = Get-ScheduledTask -TaskName "Hanako Local FS MCP", "Hanako Local FS Tunnel" -ErrorAction SilentlyContinue |
    Select-Object TaskName, State
  foreach ($before in $productionBefore) {
    $after = $productionAfter | Where-Object TaskName -eq $before.TaskName
    if (-not $after -or $after.State -ne $before.State) {
      throw "Production task state changed during isolated installer test: $($before.TaskName)"
    }
  }
  $productionShellAfter = [pscustomobject]@{
    desktopShortcut = Test-Path -LiteralPath $productionDesktopShortcut
    startShortcut = Test-Path -LiteralPath $productionStartShortcut
    registration = Get-ItemProperty -LiteralPath $productionRegistrationPath -ErrorAction SilentlyContinue
  }
  if ($productionShellAfter.desktopShortcut -ne $productionShellBefore.desktopShortcut) {
    throw "Production desktop shortcut changed during isolated installer test."
  }
  if ($productionShellAfter.startShortcut -ne $productionShellBefore.startShortcut) {
    throw "Production Start menu shortcut changed during isolated installer test."
  }
  $registrationBefore = [string]$productionShellBefore.registration.InstallLocation
  $registrationAfter = [string]$productionShellAfter.registration.InstallLocation
  if ($registrationAfter -ne $registrationBefore) {
    throw "Production uninstall registration changed during isolated installer test."
  }

  Write-Host "installer smoke tests passed"
} finally {
  Remove-Item Env:HANA_BRIDGE_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_TASK_PREFIX -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_ROOT_PATH -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_MCP_PORT -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_APPROVAL_PORT -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_REMOTE_PORT -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_NONINTERACTIVE -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_SKIP_START -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_DISABLE_TUNNEL -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_NO_MIGRATE -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION -ErrorAction SilentlyContinue

  $uninstall = Join-Path $installRoot "uninstall-background-service.ps1"
  if (Test-Path -LiteralPath $uninstall -PathType Leaf) {
    try {
      & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $uninstall
    } catch {}
  } else {
    foreach ($taskName in @(
      "$taskPrefix MCP",
      "$taskPrefix Tunnel",
      "$migrationTaskPrefix MCP",
      "$migrationTaskPrefix Tunnel"
    )) {
      Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
      Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    }
  }
  foreach ($taskName in @("$migrationTaskPrefix MCP", "$migrationTaskPrefix Tunnel")) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 1
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
