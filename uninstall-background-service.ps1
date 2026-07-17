param(
  [string]$ConfigPath = "",
  [switch]$KeepData,
  [switch]$RemoveInstall
)

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$tasks = Get-BridgeTaskNames -Runtime $runtime

foreach ($taskName in @($tasks.Mcp, $tasks.Tunnel)) {
  Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
}
Stop-BridgeProcesses -InstallRoot $installRoot -Runtime $runtime
Remove-Item -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge" `
  -Recurse `
  -Force `
  -ErrorAction SilentlyContinue
$startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "Hanako Local Bridge"
Remove-Item -LiteralPath $startMenuDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "Hanako Local Bridge background tasks were removed."

if ($RemoveInstall) {
  $expectedBase = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd("\") + "\"
  $resolvedRoot = [System.IO.Path]::GetFullPath($installRoot).TrimEnd("\")
  if (-not $resolvedRoot.StartsWith($expectedBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to remove install directory outside LOCALAPPDATA: $resolvedRoot"
  }

  $cleanupScript = Join-Path $env:TEMP "hanako-local-bridge-uninstall-$PID.ps1"
  $preserveData = [bool]$KeepData
  $dataDir = [string]$runtime.config.storage.dataDir
  $logDir = [string]$runtime.config.storage.logDir
  $configFile = [string]$runtime.configPath
  @"
Start-Sleep -Seconds 2
`$root = '$($resolvedRoot.Replace("'", "''"))'
`$keepData = `$$preserveData
if (`$keepData) {
  `$backup = Join-Path `$env:USERPROFILE 'Documents\HanakoLocalBridgeBackup'
  New-Item -ItemType Directory -Force -Path `$backup | Out-Null
  foreach (`$item in @('$($dataDir.Replace("'", "''"))', '$($logDir.Replace("'", "''"))', '$($configFile.Replace("'", "''"))')) {
    if (Test-Path -LiteralPath `$item) { Copy-Item -LiteralPath `$item -Destination `$backup -Recurse -Force }
  }
}
Remove-Item -LiteralPath `$root -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath `$PSCommandPath -Force -ErrorAction SilentlyContinue
"@ | Set-Content -LiteralPath $cleanupScript -Encoding UTF8

  Start-Process -FilePath "powershell.exe" `
    -ArgumentList @("-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File", "`"$cleanupScript`"") `
    -WindowStyle Hidden
}
