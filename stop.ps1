param(
  [switch]$KeepTasks,
  [string]$ConfigPath = ""
)

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$tasks = Get-BridgeTaskNames -Runtime $runtime

if (-not $KeepTasks) {
  foreach ($taskName in @($tasks.Mcp, $tasks.Tunnel)) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  }
}
Stop-BridgeProcesses -InstallRoot $installRoot -Runtime $runtime
