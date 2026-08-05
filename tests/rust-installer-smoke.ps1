$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$buildRoot = Join-Path $repo "build"
$runId = [Guid]::NewGuid().ToString("N")
$testRoot = Join-Path $buildRoot "rust-installer-smoke-$runId"
$installRoot = Join-Path $testRoot "install"
$profileRoot = Join-Path $testRoot "profile"
$appDataRoot = Join-Path $profileRoot "AppData\Roaming"
$smokeVersion = $env:HANA_SMOKE_VERSION
if (-not $smokeVersion) { $smokeVersion = "2.0.0" }
$smokeLabel = ($smokeVersion -replace '[^a-zA-Z0-9]', '')
$installer = $env:HANA_SMOKE_INSTALLER
$payload = $env:HANA_SMOKE_PAYLOAD
$registrySubKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-Rust${smokeLabel}Smoke"
$taskName = "Hanako Rust $smokeLabel Smoke MCP"
$actionTaskName = "Hanako Rust $smokeLabel Smoke Manager Action"
$diagnosticLog = Join-Path $buildRoot "rust-installer-smoke-stage.log"
$legacyServer = Join-Path $installRoot "legacy-server.cjs"
$legacyLauncher = Join-Path $installRoot "run-legacy-hidden.vbs"
$legacyTaskXml = Join-Path $buildRoot "rust-installer-legacy-$runId.xml"
$oldUserProfile = $env:USERPROFILE
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$passed = $false
$stage = "initialization"
$managerProcess = $null

function Assert-Path([string]$Path, [string]$Message) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw $Message
  }
}

function Get-PeSubsystem([string]$Path) {
  $bytes = [IO.File]::ReadAllBytes($Path)
  $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
  return [BitConverter]::ToUInt16($bytes, $peOffset + 24 + 68)
}

function Get-ShortcutTarget([string]$Path) {
  $shell = New-Object -ComObject WScript.Shell
  try {
    return $shell.CreateShortcut($Path).TargetPath
  } finally {
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
  }
}

function Set-Stage([string]$Value) {
  $line = "{0} {1}" -f (Get-Date -Format o), $Value
  Add-Content -LiteralPath $diagnosticLog -Value $line -Encoding utf8
  Write-Output $line
}

function Wait-Until([scriptblock]$Condition, [string]$Message, [int]$TimeoutSeconds = 20) {
  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  while ([DateTime]::UtcNow -lt $deadline) {
    if (& $Condition) {
      return
    }
    Start-Sleep -Milliseconds 250
  }
  throw $Message
}

function Test-TaskExists {
  & cmd.exe /d /c "schtasks.exe /Query /TN `"$taskName`" >nul 2>&1"
  return $LASTEXITCODE -eq 0
}

function Test-ActionTaskExists {
  & cmd.exe /d /c "schtasks.exe /Query /TN `"$actionTaskName`" >nul 2>&1"
  return $LASTEXITCODE -eq 0
}

function Test-BridgeHealth {
  try {
    $health = Invoke-RestMethod "http://127.0.0.1:38887/health"
    $approvalHealth = Invoke-RestMethod "http://127.0.0.1:38888/health"
    return (
      $health.ok -eq $true -and
      $health.version -eq $smokeVersion -and
      $approvalHealth.ok -eq $true -and
      $approvalHealth.runtime -eq "rust" -and
      $approvalHealth.version -eq $smokeVersion
    )
  } catch {
    return $false
  }
}

function Get-ManagerToken {
  $page = Invoke-WebRequest "http://127.0.0.1:38888/manager/" -UseBasicParsing
  if ($page.Content -notmatch 'const TOKEN = "([^"]+)";') {
    throw "Manager page did not expose the approval token."
  }
  return $Matches[1]
}

function Invoke-ManagerAction([string]$Action) {
  $token = Get-ManagerToken
  return Invoke-RestMethod `
    -Uri "http://127.0.0.1:38888/api/manager/action" `
    -Method Post `
    -Headers @{ "X-Approval-Token" = $token } `
    -ContentType "application/json" `
    -Body (@{ action = $Action } | ConvertTo-Json -Compress)
}

function Test-LegacyApprovalServer {
  try {
    $health = Invoke-RestMethod "http://127.0.0.1:38888/health"
    if ($health.ok -ne $true -or $health.PSObject.Properties.Name -contains "runtime") {
      return $false
    }
    $client = New-Object System.Net.Sockets.TcpClient
    try {
      $client.Connect("127.0.0.1", 38888)
      $stream = $client.GetStream()
      $request = [System.Text.Encoding]::ASCII.GetBytes(
        "GET /manager/ HTTP/1.1`r`nHost: 127.0.0.1:38888`r`nConnection: close`r`n`r`n"
      )
      $stream.Write($request, 0, $request.Length)
      $reader = New-Object System.IO.StreamReader($stream)
      $response = $reader.ReadToEnd()
      return $response -match "HTTP/1.1 403" -and $response -match "invalid approval token"
    } finally {
      $client.Dispose()
    }
  } catch {
    return $false
  }
}

function Start-LegacyTask {
  & cmd.exe /d /c "schtasks.exe /End /TN `"$taskName`" >nul 2>&1"
  & cmd.exe /d /c "schtasks.exe /Delete /TN `"$taskName`" /F >nul 2>&1"
  $node = (Get-Command node.exe -ErrorAction Stop).Source
  $wscript = Join-Path $env:WINDIR "System32\wscript.exe"
  @'
const http = require("http");
http.createServer((request, response) => {
  response.setHeader("content-type", "application/json");
  if (request.url === "/health") {
    response.end(JSON.stringify({ ok: true, trustMode: "full", approvalRequired: false }));
    return;
  }
  response.statusCode = 403;
  response.end(JSON.stringify({ error: "invalid approval token" }));
}).listen(38888, "127.0.0.1");
'@ | Set-Content -LiteralPath $legacyServer -Encoding utf8
  $legacyCommand = 'CreateObject("WScript.Shell").Run """{0}"" ""{1}""", 0, False' -f $node, $legacyServer
  [System.IO.File]::WriteAllText(
    $legacyLauncher,
    $legacyCommand,
    [System.Text.Encoding]::Unicode
  )

  $user = (& whoami.exe).Trim()
  $xml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>$user</UserId></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>$user</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>$wscript</Command>
      <Arguments>//B //NoLogo "$legacyLauncher"</Arguments>
      <WorkingDirectory>$installRoot</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"@
  $encoding = [System.Text.Encoding]::Unicode
  [System.IO.File]::WriteAllText($legacyTaskXml, $xml, $encoding)
  & schtasks.exe /Create /TN $taskName /XML $legacyTaskXml /F | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "Could not create the legacy scheduled task fixture."
  }
  & schtasks.exe /Run /TN $taskName | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "Could not start the legacy scheduled task fixture."
  }
}

function Get-InstalledManagerProcesses([string]$ManagerPath) {
  return @(
    Get-Process -Name "hanako-manager" -ErrorAction SilentlyContinue | Where-Object {
      try {
        [string]::Equals($_.Path, $ManagerPath, [System.StringComparison]::OrdinalIgnoreCase)
      } catch {
        $false
      }
    }
  )
}

function Get-InstalledBridgeProcesses([string]$BridgePath) {
  return @(
    Get-Process -Name "hanako-bridge" -ErrorAction SilentlyContinue | Where-Object {
      try {
        [string]::Equals($_.Path, $BridgePath, [System.StringComparison]::OrdinalIgnoreCase)
      } catch {
        $false
      }
    }
  )
}

function Stop-LegacyFixtureProcesses {
  Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
      $_.CommandLine -and
      $_.CommandLine.IndexOf($legacyServer, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    } |
    ForEach-Object {
      Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

function Get-ServiceDiagnostics {
  $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  $taskInfo = Get-ScheduledTaskInfo -TaskName $taskName -ErrorAction SilentlyContinue
  $connections = @(
    Get-NetTCPConnection -LocalPort 38887, 38888 -ErrorAction SilentlyContinue |
      Select-Object LocalAddress, LocalPort, State, OwningProcess
  )
  $owners = @(
    $connections |
      Where-Object OwningProcess -gt 0 |
      ForEach-Object { Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue } |
      Select-Object Id, ProcessName, Path
  )
  return [ordered]@{
    taskState = if ($task) { [string]$task.State } else { "missing" }
    lastTaskResult = if ($taskInfo) { $taskInfo.LastTaskResult } else { $null }
    connections = $connections
    owners = $owners
  }
}

function Invoke-Installer([string[]]$Arguments) {
  $quoted = $Arguments | ForEach-Object {
    if ($_ -match '[\s"]') {
      '"' + $_.Replace('"', '\"') + '"'
    } else {
      $_
    }
  }
  $process = Start-Process -FilePath $installer -ArgumentList ($quoted -join " ") -Wait -PassThru -WindowStyle Hidden
  return $process.ExitCode
}

try {
  $stage = "artifact validation"
  Set-Stage $stage
  Assert-Path $installer "Rust $smokeVersion installer is missing."
  Assert-Path $payload "Rust $smokeVersion payload is missing."

  New-Item -ItemType Directory -Force -Path $installRoot, $profileRoot, $appDataRoot | Out-Null

  $root = Join-Path $testRoot "files"
  $data = Join-Path $testRoot "data"
  $logs = Join-Path $testRoot "logs"
  New-Item -ItemType Directory -Force -Path $root, $data, $logs | Out-Null
  Set-Stage "configuration prepared"
  $config = [ordered]@{
    schemaVersion = 1
    device = [ordered]@{ id = "rust-installer-smoke"; name = "Rust Installer Smoke" }
    filesystem = [ordered]@{
      host = "127.0.0.1"
      port = 38887
      approvalPort = 38888
      trustMode = "full"
      allowChatAuthorization = $false
      chatGrantMinutes = 120
      roots = @([ordered]@{ name = "SmokeRoot"; path = $root; mode = "read_write" })
    }
    storage = [ordered]@{ dataDir = $data; logDir = $logs }
    cloud = [ordered]@{
      enabled = $false
      url = "wss://example.invalid/local-bridge/connect"
      reconnectMinSeconds = 3
      reconnectMaxSeconds = 60
      heartbeatSeconds = 25
    }
    tunnel = [ordered]@{
      enabled = $false
      server = ""
      user = ""
      localHost = "127.0.0.1"
      localPort = 0
      remoteHost = "127.0.0.1"
      remotePort = 0
      identityFile = ""
    }
    service = [ordered]@{
      taskPrefix = "Hanako Rust $smokeLabel Smoke"
      restartDelaySeconds = 3
      tunnelRetryMinSeconds = 3
      tunnelRetryMaxSeconds = 60
      tunnelHealthSeconds = 10
    }
    update = [ordered]@{
      manifest = ""
      channel = "alpha"
    }
  }
  $configJson = $config | ConvertTo-Json -Depth 10
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText((Join-Path $installRoot "config.json"), $configJson, $utf8NoBom)
  Set-Content -LiteralPath (Join-Path $data "preinstall.txt") -Value "preserve-me" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $logs "preinstall.log") -Value "preserve-log" -Encoding utf8

  $env:USERPROFILE = $profileRoot
  $env:APPDATA = $appDataRoot
  $env:LOCALAPPDATA = Join-Path $testRoot "LocalAppData"
  $env:HANA_INSTALLER_UNINSTALL_KEY = $registrySubKey
  $env:HANA_INSTALLER_SKIP_MANAGER = "1"
  New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA | Out-Null

  $stage = "legacy service fixture"
  Set-Stage $stage
  Start-LegacyTask
  Wait-Until { Test-LegacyApprovalServer } "Legacy approval server did not reproduce invalid approval token."

  $stage = "first install"
  Set-Stage $stage
  $exitCode = Invoke-Installer @("--install-root", $installRoot)
  if ($exitCode -ne 0) {
    throw "Rust installer first install failed with exit code $exitCode."
  }
  Assert-Path (Join-Path $installRoot "hanako-bridge.exe") "Bridge was not installed."
  Assert-Path (Join-Path $installRoot "hanako-manager.exe") "Manager was not installed."
  Assert-Path (Join-Path $installRoot "hanako-maintenance.exe") "Maintenance was not installed."
  if ((Get-PeSubsystem (Join-Path $installRoot "hanako-bridge.exe")) -ne 2) {
    throw "Release bridge is not a Windows GUI subsystem executable and will open a console window."
  }
  try {
    Wait-Until { Test-BridgeHealth } "Rust bridge did not become healthy after first install."
  } catch {
    $diagnostics = Get-ServiceDiagnostics | ConvertTo-Json -Depth 6 -Compress
    throw "$($_.Exception.Message) Diagnostics: $diagnostics"
  }
  Wait-Until { Test-TaskExists } "Rust scheduled task was not installed."
  $taskXml = Export-ScheduledTask -TaskName $taskName
  if ($taskXml -notmatch "<TimeTrigger>" -or $taskXml -notmatch "<Interval>PT1M</Interval>") {
    throw "Rust scheduled task does not contain the periodic self-healing trigger."
  }
  $desktopShortcut = Join-Path $profileRoot "Desktop\Hanako Local Bridge.lnk"
  Assert-Path $desktopShortcut "Desktop shortcut was not created."
  Set-Stage "first install assertions passed"
  $startMenuShortcut = Join-Path $appDataRoot "Microsoft\Windows\Start Menu\Programs\Hanako Local Bridge\Hanako Local Bridge.lnk"
  Assert-Path $startMenuShortcut "Start menu shortcut was not created."
  $bridgePath = Join-Path $installRoot "hanako-bridge.exe"
  foreach ($shortcutPath in @($desktopShortcut, $startMenuShortcut)) {
    $shortcutTarget = Get-ShortcutTarget $shortcutPath
    if (-not [string]::Equals($shortcutTarget, $bridgePath, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Product shortcut points to '$shortcutTarget' instead of the unified entry '$bridgePath'."
    }
  }
  if (-not (Test-Path "HKCU:\$registrySubKey")) {
    throw "Rust uninstall registry entry was not created."
  }

  $stage = "manager repair action"
  Set-Stage $stage
  $repair = Invoke-ManagerAction "repair"
  if ($repair.ok -ne $true -or $repair.action -ne "repair") {
    throw "Manager repair action was not accepted."
  }
  Wait-Until { Test-BridgeHealth } "Rust bridge did not become healthy after manager repair."
  Wait-Until { -not (Test-ActionTaskExists) } "Manager repair action task was not cleaned up."

  $stage = "overwrite install"
  Set-Stage $stage
  Set-Content -LiteralPath (Join-Path $data "overwrite-marker.txt") -Value "keep-data" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $logs "overwrite-marker.log") -Value "keep-log" -Encoding utf8
  $exitCode = Invoke-Installer @("--install-root", $installRoot)
  if ($exitCode -ne 0) {
    throw "Rust installer overwrite failed with exit code $exitCode."
  }
  Wait-Until { Test-BridgeHealth } "Rust bridge did not become healthy after overwrite install."
  if ((Get-Content -LiteralPath (Join-Path $data "overwrite-marker.txt") -Raw) -notmatch "keep-data") {
    throw "Overwrite install did not preserve data."
  }
  if ((Get-Content -LiteralPath (Join-Path $logs "overwrite-marker.log") -Raw) -notmatch "keep-log") {
    throw "Overwrite install did not preserve logs."
  }

  $stage = "manager single instance"
  Set-Stage $stage
  $managerPath = Join-Path $installRoot "hanako-manager.exe"
  $entryProcess = Start-Process -FilePath $bridgePath -PassThru -WindowStyle Hidden
  if (-not $entryProcess.WaitForExit(5000)) {
    throw "The unified product entry did not return after launching the internal manager."
  }
  if ($entryProcess.ExitCode -ne 0) {
    throw "The unified product entry exited with code $($entryProcess.ExitCode)."
  }
  Wait-Until {
    @(Get-InstalledManagerProcesses $managerPath).Count -eq 1
  } "The unified product entry did not open the internal manager."
  $managerProcess = @(Get-InstalledManagerProcesses $managerPath)[0]
  $secondEntry = Start-Process -FilePath $bridgePath -PassThru -WindowStyle Hidden
  if (-not $secondEntry.WaitForExit(5000)) {
    throw "The repeated unified product entry did not return after activating the manager."
  }
  if ($secondEntry.ExitCode -ne 0) {
    throw "The second unified product launch exited with code $($secondEntry.ExitCode)."
  }
  Start-Sleep -Milliseconds 500
  if (@(Get-InstalledManagerProcesses $managerPath).Count -ne 1) {
    throw "Repeated product launches created more than one installed manager process."
  }
  Stop-Process -Id $managerProcess.Id -Force
  $managerProcess = $null
  Wait-Until { Test-BridgeHealth } "Closing the manager stopped the background bridge service."

  $stage = "periodic service recovery"
  Set-Stage $stage
  $bridgeProcesses = @(Get-InstalledBridgeProcesses $bridgePath)
  if ($bridgeProcesses.Count -ne 1) {
    throw "Expected one installed bridge before the recovery test, found $($bridgeProcesses.Count)."
  }
  $oldBridgeId = $bridgeProcesses[0].Id
  Stop-Process -Id $oldBridgeId -Force
  Wait-Until {
    if (-not (Test-BridgeHealth)) {
      return $false
    }
    $current = @(Get-InstalledBridgeProcesses $bridgePath)
    return $current.Count -eq 1 -and $current[0].Id -ne $oldBridgeId
  } "The periodic task trigger did not recover the terminated bridge." 90
  $recoveredBridge = @(Get-InstalledBridgeProcesses $bridgePath)[0]
  if ($recoveredBridge.MainWindowHandle -ne 0) {
    throw "The recovered bridge opened a visible window."
  }

  $stage = "uninstall"
  Set-Stage $stage
  $exitCode = Invoke-Installer @("--uninstall", "--install-root", $installRoot)
  if ($exitCode -ne 0) {
    throw "Rust uninstall launch failed with exit code $exitCode."
  }
  Wait-Until { -not (Test-Path -LiteralPath $installRoot) } "Rust uninstall worker did not remove the test installation."
  Wait-Until { -not (Test-TaskExists) } "Rust uninstall worker did not remove the scheduled task."
  if (Test-Path "HKCU:\$registrySubKey") {
    throw "Rust uninstall worker did not remove the uninstall registry entry."
  }

  $passed = $true
  Write-Output "Rust installer smoke test passed"
} catch {
  Write-Error "Rust installer smoke test failed during $stage`: $($_.Exception.Message)"
  throw
} finally {
  if ($managerProcess -and -not $managerProcess.HasExited) {
    Stop-Process -Id $managerProcess.Id -Force -ErrorAction SilentlyContinue
  }
  & cmd.exe /d /c "schtasks.exe /End /TN `"$taskName`" >nul 2>&1"
  & cmd.exe /d /c "schtasks.exe /Delete /TN `"$taskName`" /F >nul 2>&1"
  & cmd.exe /d /c "schtasks.exe /End /TN `"$actionTaskName`" >nul 2>&1"
  & cmd.exe /d /c "schtasks.exe /Delete /TN `"$actionTaskName`" /F >nul 2>&1"
  Stop-LegacyFixtureProcesses
  if ($passed) {
    if (Test-Path -LiteralPath $testRoot) {
      Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
  Remove-Item -LiteralPath "HKCU:\$registrySubKey" -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $legacyTaskXml -Force -ErrorAction SilentlyContinue
  $env:USERPROFILE = $oldUserProfile
  $env:APPDATA = $oldAppData
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:HANA_INSTALLER_UNINSTALL_KEY = $null
  $env:HANA_INSTALLER_SKIP_MANAGER = $null
}
