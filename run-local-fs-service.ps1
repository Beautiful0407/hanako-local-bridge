param([string]$ConfigPath = "")

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$config = $runtime.config
$node = Get-BridgeNodePath -InstallRoot $installRoot
$script = Join-Path $installRoot "server.cjs"
$dataDir = [string]$config.storage.dataDir
$logDir = [string]$config.storage.logDir
$watchdogLog = Join-Path $logDir "local-fs-watchdog.log"
$outLog = Join-Path $logDir "local-fs-mcp.out.log"
$errLog = Join-Path $logDir "local-fs-mcp.err.log"
$restartDelay = [Math]::Max(1, [int]$config.service.restartDelaySeconds)
$mutexName = Get-BridgeMutexName -InstallRoot $installRoot -Role "Mcp"
$mutex = [System.Threading.Mutex]::new($false, $mutexName)

if (-not $mutex.WaitOne(0, $false)) {
  $mutex.Dispose()
  exit 0
}

New-Item -ItemType Directory -Force -Path $dataDir, $logDir | Out-Null
$env:HANA_LOCAL_BRIDGE_CONFIG = [string]$runtime.configPath

try {
  while ($true) {
    Rotate-BridgeLogFile -Path $watchdogLog
    Rotate-BridgeLogFile -Path $outLog
    Rotate-BridgeLogFile -Path $errLog
    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) starting local MCP with $node"
    & $node $script 1>> $outLog 2>> $errLog
    $exitCode = $LASTEXITCODE
    Rotate-BridgeLogFile -Path $watchdogLog
    Rotate-BridgeLogFile -Path $outLog
    Rotate-BridgeLogFile -Path $errLog
    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) local MCP exited code=$exitCode; restarting in $restartDelay seconds"
    Start-Sleep -Seconds $restartDelay
  }
} finally {
  try { $mutex.ReleaseMutex() } catch {}
  $mutex.Dispose()
}
