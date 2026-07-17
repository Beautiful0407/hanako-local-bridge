param([string]$ConfigPath = "")

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$config = $runtime.config
$tasks = Get-BridgeTaskNames -Runtime $runtime
$localPort = [int]$config.filesystem.port
$approvalPort = [int]$config.filesystem.approvalPort
$remotePort = [int]$config.tunnel.remotePort

Write-Host "== Installation =="
Write-Host "Root:    $installRoot"
Write-Host "Config:  $($runtime.configPath)"
Write-Host "Version: $((Get-Content -LiteralPath (Join-Path $installRoot 'package.json') -Raw | ConvertFrom-Json).version)"
Write-Host "Device:  $($config.device.id) ($($config.device.name))"
Write-Host "Cloud:   $($config.cloud.url)"

Write-Host "== Local MCP health =="
try {
  Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:${localPort}/health" -TimeoutSec 5 |
    Select-Object StatusCode, Content |
    Format-List
} catch {
  Write-Host "Local MCP unavailable: $($_.Exception.Message)"
}

Write-Host "== Local status UI =="
try {
  Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:${approvalPort}/health" -TimeoutSec 5 |
    Select-Object StatusCode, Content |
    Format-List
} catch {
  Write-Host "Status UI unavailable: $($_.Exception.Message)"
}

Write-Host "== Scheduled tasks =="
foreach ($taskName in @($tasks.Mcp, $tasks.Tunnel)) {
  Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue |
    Select-Object TaskName, State |
    Format-Table -AutoSize
}

Write-Host "== Local processes =="
Get-CimInstance Win32_Process |
  Where-Object {
    -not [string]::IsNullOrWhiteSpace($_.CommandLine) -and (
      $_.CommandLine.IndexOf($installRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
      $_.CommandLine.IndexOf("127.0.0.1:${remotePort}:127.0.0.1:${localPort}", [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    )
  } |
  Select-Object ProcessId, Name, CommandLine |
  Format-List

if ([bool]$config.cloud.enabled) {
  Write-Host "== Cloud WebSocket =="
  try {
    $cloudHealth = Invoke-RestMethod -Uri "http://127.0.0.1:${localPort}/health" -TimeoutSec 5
    $cloudHealth.cloud | Format-List
  } catch {
    Write-Host "Cloud connector status unavailable: $($_.Exception.Message)"
  }
} elseif ([bool]$config.tunnel.enabled) {
  Write-Host "== Remote tunnel health =="
  $sshArguments = @(
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=8",
    "$($config.tunnel.user)@$($config.tunnel.server)",
    "ss -tlnp | grep ${remotePort} || true; echo ---; curl -sS --max-time 5 http://127.0.0.1:${remotePort}/health || true"
  )
  if (-not [string]::IsNullOrWhiteSpace([string]$config.tunnel.identityFile)) {
    $sshArguments = @("-i", [string]$config.tunnel.identityFile) + $sshArguments
  }
  $remoteResult = Invoke-BridgeProcessWithTimeout `
    -FilePath "ssh.exe" `
    -ArgumentList $sshArguments `
    -TimeoutSeconds 20
  if ($remoteResult.TimedOut) {
    Write-Host "Remote check timed out after 20 seconds."
  } elseif ($remoteResult.ExitCode -ne 0) {
    Write-Host "Remote check failed with exit code $($remoteResult.ExitCode)."
    if (-not [string]::IsNullOrWhiteSpace($remoteResult.StdErr)) {
      Write-Host $remoteResult.StdErr.Trim()
    }
  } else {
    Write-Host $remoteResult.StdOut.TrimEnd()
  }
}
