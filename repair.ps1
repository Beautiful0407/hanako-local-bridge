param(
  [string]$ConfigPath = "",
  [switch]$NonInteractive,
  [switch]$SkipStart
)

$ErrorActionPreference = "Stop"
$arguments = @{
  ConfigPath = $ConfigPath
}
if ($NonInteractive) { $arguments.NonInteractive = $true }
if ($SkipStart) { $arguments.SkipStart = $true }
& (Join-Path $PSScriptRoot "install-background-service.ps1") @arguments
