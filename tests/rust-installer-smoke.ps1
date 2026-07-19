$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$buildRoot = Join-Path $repo "build"
$runId = [Guid]::NewGuid().ToString("N")
$testRoot = Join-Path $buildRoot "rust-installer-smoke-$runId"
$profileRoot = Join-Path $testRoot "profile"
$appDataRoot = Join-Path $profileRoot "AppData\Roaming"
$installer = Join-Path $buildRoot "rust-release-alpha2\HanakoLocalBridge-Setup-2.0.0-alpha.2.exe"
$payload = Join-Path $buildRoot "rust-release-alpha2\HanakoLocalBridge-2.0.0-alpha.2-win-x64.zip"
$registryPath = "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustAlpha2Smoke"
$registrySubKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustAlpha2Smoke"
$taskName = "Hanako Rust Alpha2 Smoke MCP"
$diagnosticLog = Join-Path $buildRoot "rust-installer-smoke-stage.log"
$oldUserProfile = $env:USERPROFILE
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$passed = $false
$stage = "initialization"

function Assert-Path([string]$Path, [string]$Message) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw $Message
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

function Test-BridgeHealth {
  try {
    $health = Invoke-RestMethod "http://127.0.0.1:38887/health"
    return $health.ok -eq $true -and $health.version -eq "2.0.0-alpha.2"
  } catch {
    return $false
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
  Assert-Path $installer "Rust Alpha 2 installer is missing."
  Assert-Path $payload "Rust Alpha 2 payload is missing."

  New-Item -ItemType Directory -Force -Path $profileRoot, $appDataRoot | Out-Null

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
      taskPrefix = "Hanako Rust Alpha2 Smoke"
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
  [System.IO.File]::WriteAllText((Join-Path $testRoot "config.json"), $configJson, $utf8NoBom)
  Set-Content -LiteralPath (Join-Path $data "preinstall.txt") -Value "preserve-me" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $logs "preinstall.log") -Value "preserve-log" -Encoding utf8

  $env:USERPROFILE = $profileRoot
  $env:APPDATA = $appDataRoot
  $env:LOCALAPPDATA = Join-Path $testRoot "LocalAppData"
  $env:HANA_INSTALLER_UNINSTALL_KEY = $registrySubKey
  $env:HANA_INSTALLER_SKIP_MANAGER = "1"
  New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA | Out-Null

  $stage = "first install"
  Set-Stage $stage
  $exitCode = Invoke-Installer @("--install-root", $testRoot)
  if ($exitCode -ne 0) {
    throw "Rust installer first install failed with exit code $exitCode."
  }
  Assert-Path (Join-Path $testRoot "hanako-bridge.exe") "Bridge was not installed."
  Assert-Path (Join-Path $testRoot "hanako-manager.exe") "Manager was not installed."
  Assert-Path (Join-Path $testRoot "hanako-maintenance.exe") "Maintenance was not installed."
  Wait-Until { Test-BridgeHealth } "Rust bridge did not become healthy after first install."
  Wait-Until { Test-TaskExists } "Rust scheduled task was not installed."
  Assert-Path (Join-Path $profileRoot "Desktop\Hanako Local Bridge.lnk") "Desktop shortcut was not created."
  Set-Stage "first install assertions passed"
  Assert-Path (Join-Path $appDataRoot "Microsoft\Windows\Start Menu\Programs\Hanako Local Bridge\Hanako Local Bridge.lnk") "Start menu shortcut was not created."
  if (-not (Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustAlpha2Smoke")) {
    throw "Rust uninstall registry entry was not created."
  }

  $stage = "overwrite install"
  Set-Stage $stage
  Set-Content -LiteralPath (Join-Path $data "overwrite-marker.txt") -Value "keep-data" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $logs "overwrite-marker.log") -Value "keep-log" -Encoding utf8
  $exitCode = Invoke-Installer @("--install-root", $testRoot)
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

  $stage = "uninstall"
  Set-Stage $stage
  $exitCode = Invoke-Installer @("--uninstall", "--install-root", $testRoot)
  if ($exitCode -ne 0) {
    throw "Rust uninstall launch failed with exit code $exitCode."
  }
  Wait-Until { -not (Test-Path -LiteralPath $testRoot) } "Rust uninstall worker did not remove the test installation."
  Wait-Until { -not (Test-TaskExists) } "Rust uninstall worker did not remove the scheduled task."
  if (Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustAlpha2Smoke") {
    throw "Rust uninstall worker did not remove the uninstall registry entry."
  }

  $passed = $true
  Write-Output "Rust installer smoke test passed"
} catch {
  Write-Error "Rust installer smoke test failed during $stage`: $($_.Exception.Message)"
  throw
} finally {
  if ($passed) {
    if (Test-Path -LiteralPath $testRoot) {
      Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
  }
  Remove-Item -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustAlpha2Smoke" -Recurse -Force -ErrorAction SilentlyContinue
  $env:USERPROFILE = $oldUserProfile
  $env:APPDATA = $oldAppData
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:HANA_INSTALLER_UNINSTALL_KEY = $null
  $env:HANA_INSTALLER_SKIP_MANAGER = $null
}
