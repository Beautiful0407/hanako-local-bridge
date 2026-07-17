param(
  [string]$ConfigPath = "",
  [string]$TaskPrefix = "",
  [switch]$NonInteractive,
  [switch]$SkipStart
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ConfigPath)) {
  $ConfigPath = Join-Path $installRoot "config.json"
}

if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf) -or -not [string]::IsNullOrWhiteSpace($TaskPrefix)) {
  $configureArgs = @{
    InstallRoot = $installRoot
    ConfigPath = $ConfigPath
  }
  if ($NonInteractive) { $configureArgs.NonInteractive = $true }
  if (-not [string]::IsNullOrWhiteSpace($TaskPrefix)) { $configureArgs.TaskPrefix = $TaskPrefix }
  & (Join-Path $installRoot "configure.ps1") @configureArgs
}

$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$tasks = Get-BridgeTaskNames -Runtime $runtime
$currentUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$wscript = Join-Path $env:WINDIR "System32\wscript.exe"
$mcpLauncher = Join-Path $installRoot "run-local-fs-hidden.vbs"
$tunnelLauncher = Join-Path $installRoot "run-reverse-tunnel-hidden.vbs"

$mcpAction = New-ScheduledTaskAction `
  -Execute $wscript `
  -Argument "//B //NoLogo `"$mcpLauncher`""
$tunnelAction = New-ScheduledTaskAction `
  -Execute $wscript `
  -Argument "//B //NoLogo `"$tunnelLauncher`""
$logonTrigger = New-ScheduledTaskTrigger -AtLogOn -User $currentUser
$principal = New-ScheduledTaskPrincipal -UserId $currentUser -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet `
  -Hidden `
  -AllowStartIfOnBatteries `
  -DontStopIfGoingOnBatteries `
  -StartWhenAvailable `
  -RestartCount 999 `
  -RestartInterval (New-TimeSpan -Minutes 1) `
  -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -MultipleInstances IgnoreNew

foreach ($taskName in @($tasks.Mcp, $tasks.Tunnel)) {
  Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
}
Stop-BridgeProcesses -InstallRoot $installRoot -Runtime $runtime

Register-ScheduledTask `
  -TaskName $tasks.Mcp `
  -Action $mcpAction `
  -Trigger $logonTrigger `
  -Principal $principal `
  -Settings $settings `
  -Description "Hidden Hanako local read/write and script execution MCP watchdog" `
  -Force | Out-Null

Register-ScheduledTask `
  -TaskName $tasks.Tunnel `
  -Action $tunnelAction `
  -Trigger $logonTrigger `
  -Principal $principal `
  -Settings $settings `
  -Description "Legacy Hanako SSH tunnel watchdog; exits automatically when cloud WebSocket mode is enabled" `
  -Force | Out-Null

if (-not $SkipStart) {
  Start-ScheduledTask -TaskName $tasks.Mcp
  $deadline = (Get-Date).AddSeconds(30)
  do {
    Start-Sleep -Milliseconds 500
    try {
      $health = Invoke-WebRequest `
        -UseBasicParsing `
        -Uri "http://127.0.0.1:$($runtime.config.filesystem.port)/health" `
        -TimeoutSec 2
      if ($health.StatusCode -eq 200) { break }
    } catch {}
  } while ((Get-Date) -lt $deadline)

  if ([bool]$runtime.config.tunnel.enabled) {
    Start-ScheduledTask -TaskName $tasks.Tunnel
  }
}

Write-Host "Installed background tasks:"
Write-Host "  $($tasks.Mcp)"
Write-Host "  $($tasks.Tunnel)"
Write-Host "Both tasks launch through hidden wscript.exe watchdogs."
Write-Host "Configuration: $($runtime.configPath)"
Write-Host "Status: http://127.0.0.1:$($runtime.config.filesystem.approvalPort)/"
