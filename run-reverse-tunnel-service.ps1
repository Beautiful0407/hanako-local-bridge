param([string]$ConfigPath = "")

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "bridge-common.ps1")

$installRoot = Get-BridgeInstallRoot -InstallRoot $PSScriptRoot
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$config = $runtime.config
$tunnel = $config.tunnel
$logDir = [string]$config.storage.logDir
$watchdogLog = Join-Path $logDir "ssh-tunnel-watchdog.log"
$outLog = Join-Path $logDir "ssh-tunnel.out.log"
$errLog = Join-Path $logDir "ssh-tunnel.err.log"
$mutexName = Get-BridgeMutexName -InstallRoot $installRoot -Role "Tunnel"
$mutex = [System.Threading.Mutex]::new($false, $mutexName)
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

if ([bool]$config.cloud.enabled) {
  Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) cloud websocket mode enabled; SSH tunnel is not required"
  $mutex.Dispose()
  exit 0
}

if (-not $mutex.WaitOne(0, $false)) {
  $mutex.Dispose()
  exit 0
}

try {
  if (-not [bool]$tunnel.enabled) {
    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) tunnel disabled by config"
    exit 0
  }

  $server = if ($env:HANA_TUNNEL_SERVER) { $env:HANA_TUNNEL_SERVER } else { [string]$tunnel.server }
  $user = if ($env:HANA_TUNNEL_USER) { $env:HANA_TUNNEL_USER } else { [string]$tunnel.user }
  $localHost = [string]$tunnel.localHost
  $localPort = if ($env:HANA_TUNNEL_LOCAL_PORT) { [int]$env:HANA_TUNNEL_LOCAL_PORT } else { [int]$tunnel.localPort }
  $remoteHost = if ($env:HANA_TUNNEL_REMOTE_HOST) { $env:HANA_TUNNEL_REMOTE_HOST } else { [string]$tunnel.remoteHost }
  $remotePort = if ($env:HANA_TUNNEL_REMOTE_PORT) { [int]$env:HANA_TUNNEL_REMOTE_PORT } else { [int]$tunnel.remotePort }
  $retryMin = [Math]::Max(2, [int]$config.service.tunnelRetryMinSeconds)
  $retryMax = [Math]::Max($retryMin, [int]$config.service.tunnelRetryMaxSeconds)
  $healthSeconds = [Math]::Max(10, [int]$config.service.tunnelHealthSeconds)
  $commandTimeoutSeconds = 20
  $retrySeconds = $retryMin

  $ssh = Join-Path $env:WINDIR "System32\OpenSSH\ssh.exe"
  if (-not (Test-Path -LiteralPath $ssh -PathType Leaf)) { $ssh = "ssh.exe" }
  $target = "${user}@${server}"
  $forward = "${remoteHost}:${remotePort}:${localHost}:${localPort}"
  $sshCommon = @(
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=10",
    "-o", "NumberOfPasswordPrompts=0",
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", "ServerAliveInterval=30",
    "-o", "ServerAliveCountMax=3"
  )
  if (-not [string]::IsNullOrWhiteSpace([string]$tunnel.identityFile)) {
    $sshCommon += @("-i", [string]$tunnel.identityFile)
  }

  $deviceId = [string]$config.device.id
  $deviceName = [string]$config.device.name
  $mcpTokenPath = Join-Path ([string]$config.storage.dataDir) "approval-token.txt"
  $mcpToken = if (Test-Path -LiteralPath $mcpTokenPath -PathType Leaf) {
    (Get-Content -LiteralPath $mcpTokenPath -Raw).Trim()
  } else {
    ""
  }
  if (-not [string]::IsNullOrWhiteSpace($deviceId)) {
    try {
      $registrationJson = @{
        id = $deviceId
        name = $deviceName
        remotePort = $remotePort
        mcpToken = $mcpToken
      } | ConvertTo-Json -Compress
      $registrationBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($registrationJson))
      $registrationCommand = "printf '%s' '$registrationBase64' | base64 -d | curl -fsS --max-time 10 -X POST -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:18786/devices/register"
      $registrationResult = Invoke-BridgeProcessWithTimeout `
        -FilePath $ssh `
        -ArgumentList (@($sshCommon) + @($target, $registrationCommand)) `
        -TimeoutSeconds $commandTimeoutSeconds
      if ($registrationResult.ExitCode -eq 0 -and -not [string]::IsNullOrWhiteSpace($registrationResult.StdOut)) {
        $registration = $registrationResult.StdOut | ConvertFrom-Json
        if ([int]$registration.remotePort -gt 0) {
          $remotePort = [int]$registration.remotePort
          if ([int]$config.tunnel.remotePort -ne $remotePort) {
            $config.tunnel.remotePort = $remotePort
            Write-BridgeJson -Value $config -Path ([string]$runtime.configPath)
          }
          Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) registered device=$deviceId remotePort=$remotePort"
        }
      } else {
        Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) device registration unavailable; using configured remotePort=$remotePort"
      }
    } catch {
      Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) device registration failed: $($_.Exception.Message); using configured remotePort=$remotePort"
    }
  }

  $cleanupCommand = 'pid=$(ss -ltnp | grep ":{0} " | grep -o "pid=[0-9]*" | head -n 1 | cut -d= -f2); if [ -n "$pid" ]; then kill "$pid" >/dev/null 2>&1 || true; fi' -f $remotePort

  while ($true) {
    Rotate-BridgeLogFile -Path $watchdogLog
    Rotate-BridgeLogFile -Path $outLog
    Rotate-BridgeLogFile -Path $errLog

    try {
      Invoke-WebRequest -UseBasicParsing -Uri "http://${localHost}:${localPort}/health" -TimeoutSec 4 | Out-Null
    } catch {
      Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) local MCP unavailable; retrying in $retryMin seconds"
      Start-Sleep -Seconds $retryMin
      continue
    }

    $healthResult = Invoke-BridgeProcessWithTimeout `
      -FilePath $ssh `
      -ArgumentList (@($sshCommon) + @($target, "curl -fsS --max-time 4 http://127.0.0.1:${remotePort}/health")) `
      -TimeoutSeconds $commandTimeoutSeconds
    if ($healthResult.ExitCode -eq 0 -and $healthResult.StdOut -match '"ok"\s*:\s*true') {
      Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) remote tunnel healthy; rechecking in $healthSeconds seconds"
      $retrySeconds = $retryMin
      Start-Sleep -Seconds $healthSeconds
      continue
    }
    if ($healthResult.TimedOut) {
      Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) remote health check timed out after ${commandTimeoutSeconds}s; forcing reconnect"
    }

    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) clearing stale remote listener on ${remoteHost}:${remotePort}"
    $cleanupResult = Invoke-BridgeProcessWithTimeout `
      -FilePath $ssh `
      -ArgumentList (@($sshCommon) + @($target, $cleanupCommand)) `
      -TimeoutSeconds $commandTimeoutSeconds `
      -StdOutPath $outLog `
      -StdErrPath $errLog
    if ($cleanupResult.TimedOut) {
      Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) stale-listener cleanup timed out after ${commandTimeoutSeconds}s; continuing"
    }
    Start-Sleep -Seconds 1

    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) connecting $target $forward"
    $connectedAt = Get-Date
    & $ssh `
      @sshCommon `
      -N `
      -o ExitOnForwardFailure=yes `
      -R $forward `
      $target 1>> $outLog 2>> $errLog
    $exitCode = $LASTEXITCODE
    $connectedSeconds = [Math]::Max(0, ((Get-Date) - $connectedAt).TotalSeconds)
    if ($connectedSeconds -ge 60) {
      $retrySeconds = $retryMin
    } else {
      $retrySeconds = [Math]::Min($retryMax, [Math]::Max($retryMin, $retrySeconds * 2))
    }
    Add-Content -LiteralPath $watchdogLog -Value "$(Get-Date -Format o) tunnel exited code=$exitCode after=$([Math]::Round($connectedSeconds, 1))s; reconnecting in ${retrySeconds}s"
    Start-Sleep -Seconds $retrySeconds
  }
} finally {
  try { $mutex.ReleaseMutex() } catch {}
  $mutex.Dispose()
}
