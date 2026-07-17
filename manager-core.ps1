. (Join-Path $PSScriptRoot "bridge-common.ps1")

function ConvertTo-HanakoCloudWebBase {
  param([string]$CloudUrl)

  $value = [string]$CloudUrl
  if ([string]::IsNullOrWhiteSpace($value)) { return "" }
  try {
    $uri = [Uri]$value.Trim()
    $scheme = if ($uri.Scheme -eq "wss") { "https" } elseif ($uri.Scheme -eq "ws") { "http" } else { $uri.Scheme }
    if ([string]::IsNullOrWhiteSpace($scheme)) { return "" }
    $builder = [UriBuilder]::new($uri)
    $builder.Scheme = $scheme
    if (($scheme -eq "http" -and $builder.Port -eq 80) -or
        ($scheme -eq "https" -and $builder.Port -eq 443)) {
      $builder.Port = -1
    }
    $builder.Path = "/"
    $builder.Query = ""
    $builder.Fragment = ""
    return $builder.Uri.AbsoluteUri.TrimEnd("/")
  } catch {
    return ""
  }
}

function Resolve-HanakoBridgePath {
  param(
    [Parameter(Mandatory = $true)][string]$InstallRoot,
    [Parameter(Mandatory = $true)][string]$Path
  )

  if ([string]::IsNullOrWhiteSpace($Path)) { return "" }
  if ([System.IO.Path]::IsPathRooted($Path)) {
    return [System.IO.Path]::GetFullPath($Path)
  }
  return [System.IO.Path]::GetFullPath((Join-Path $InstallRoot $Path))
}

function New-HanakoBridgeCheck {
  param(
    [Parameter(Mandatory = $true)][string]$Code,
    [Parameter(Mandatory = $true)][string]$Status,
    [Parameter(Mandatory = $true)][string]$Detail
  )

  [pscustomobject]@{
    code = $Code
    status = $Status
    detail = $Detail
  }
}

function Get-HanakoBridgeProcessList {
  param([Parameter(Mandatory = $true)][string]$InstallRoot)

  $root = [System.IO.Path]::GetFullPath($InstallRoot)
  $allProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
  @(
    $allProcesses |
      Where-Object {
        $commandLine = [string]$_.CommandLine
        $isBridgeProcess = $commandLine -match "(?i)(run-local-fs-hidden\.vbs|run-local-fs-service\.ps1|server\.cjs|run-reverse-tunnel-hidden\.vbs|run-reverse-tunnel-service\.ps1)"
        -not [string]::IsNullOrWhiteSpace([string]$_.CommandLine) -and
        $commandLine -match [Regex]::Escape($root) -and
        $isBridgeProcess -and
        $_.ProcessId -ne $PID
      } |
      Select-Object ProcessId, Name, ParentProcessId, CommandLine
  )
}

function Get-HanakoBridgeTaskAction {
  param($Task)

  if (-not $Task) { return "" }
  $actions = @($Task.Actions)
  if ($actions.Count -eq 0) { return "" }
  $execute = [string]$actions[0].Execute
  $arguments = [string]$actions[0].Arguments
  if ($arguments) { return "$execute $arguments" }
  return $execute
}

function Get-HanakoBridgeManagerSnapshot {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ConfigPath = ""
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $checks = [System.Collections.Generic.List[object]]::new()
  $runtime = $null
  $runtimeError = ""
  $packageVersion = "unknown"

  try {
    $runtime = Get-BridgeRuntime -InstallRoot $root -ConfigPath $ConfigPath
    $checks.Add((New-HanakoBridgeCheck -Code "config" -Status "pass" -Detail ([string]$runtime.configPath)))
  } catch {
    $runtimeError = $_.Exception.Message
    $checks.Add((New-HanakoBridgeCheck -Code "config" -Status "error" -Detail $runtimeError))
  }

  try {
    $packagePath = Join-Path $root "package.json"
    $packageVersion = [string](Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json).version
    $checks.Add((New-HanakoBridgeCheck -Code "package" -Status "pass" -Detail "Bridge $packageVersion"))
  } catch {
    $checks.Add((New-HanakoBridgeCheck -Code "package" -Status "error" -Detail "package.json is missing or invalid"))
  }

  if (-not $runtime) {
    return [pscustomobject]@{
      capturedAt = (Get-Date).ToString("o")
      overall = "error"
      recommendation = "Configuration could not be loaded. Run repair or reinstall the bridge."
      installRoot = $root
      configPath = if ($ConfigPath) { [System.IO.Path]::GetFullPath($ConfigPath) } else { Join-Path $root "config.json" }
      version = $packageVersion
      device = $null
      local = $null
      cloud = $null
      identity = $null
      tasks = $null
      processes = @()
      checks = @($checks)
      error = $runtimeError
    }
  }

  $config = $runtime.config
  $tasks = Get-BridgeTaskNames -Runtime $runtime
  $mcpTask = Get-ScheduledTask -TaskName $tasks.Mcp -ErrorAction SilentlyContinue | Select-Object -First 1
  $tunnelTask = Get-ScheduledTask -TaskName $tasks.Tunnel -ErrorAction SilentlyContinue | Select-Object -First 1

  if ($mcpTask) {
    $checks.Add((New-HanakoBridgeCheck -Code "mcp_task" -Status "pass" -Detail ([string]$mcpTask.State)))
  } else {
    $checks.Add((New-HanakoBridgeCheck -Code "mcp_task" -Status "error" -Detail "Scheduled task is missing"))
  }

  $hiddenLauncher = [bool]$false
  $mcpAction = Get-HanakoBridgeTaskAction -Task $mcpTask
  if ($mcpAction) {
    $hiddenLauncher = $mcpAction -match "(?i)wscript\.exe"
  }
  if ($hiddenLauncher) {
    $checks.Add((New-HanakoBridgeCheck -Code "hidden_launcher" -Status "pass" -Detail "Task uses hidden wscript launcher"))
  } else {
    $checks.Add((New-HanakoBridgeCheck -Code "hidden_launcher" -Status "warning" -Detail "Task does not use the hidden launcher"))
  }

  $bridgeProcesses = Get-HanakoBridgeProcessList -InstallRoot $root
  $nodeProcess = $bridgeProcesses |
    Where-Object { $_.Name -eq "node.exe" -and [string]$_.CommandLine -like "*server.cjs*" } |
    Select-Object -First 1
  if ($nodeProcess) {
    $checks.Add((New-HanakoBridgeCheck -Code "mcp_process" -Status "pass" -Detail "node.exe PID $($nodeProcess.ProcessId)"))
  } else {
    $checks.Add((New-HanakoBridgeCheck -Code "mcp_process" -Status "error" -Detail "Bridge Node process is not running"))
  }

  $mcpPort = [int]$config.filesystem.port
  $statusPort = [int]$config.filesystem.approvalPort
  $mcpHealth = $null
  $statusHealth = $null
  $mcpError = ""
  $statusError = ""

  try {
    $mcpHealth = Invoke-RestMethod -Uri "http://127.0.0.1:${mcpPort}/health" -TimeoutSec 3
    if ($mcpHealth.ok -eq $true) {
      $checks.Add((New-HanakoBridgeCheck -Code "mcp_health" -Status "pass" -Detail "127.0.0.1:${mcpPort} is healthy"))
    } else {
      $checks.Add((New-HanakoBridgeCheck -Code "mcp_health" -Status "error" -Detail "Health response is not OK"))
    }
  } catch {
    $mcpError = $_.Exception.Message
    $checks.Add((New-HanakoBridgeCheck -Code "mcp_health" -Status "error" -Detail "127.0.0.1:${mcpPort} is unavailable"))
  }

  try {
    $statusHealth = Invoke-RestMethod -Uri "http://127.0.0.1:${statusPort}/health" -TimeoutSec 3
    if ($statusHealth.ok -eq $true) {
      $checks.Add((New-HanakoBridgeCheck -Code "status_health" -Status "pass" -Detail "127.0.0.1:${statusPort} is healthy"))
    } else {
      $checks.Add((New-HanakoBridgeCheck -Code "status_health" -Status "warning" -Detail "Status response is not OK"))
    }
  } catch {
    $statusError = $_.Exception.Message
    $checks.Add((New-HanakoBridgeCheck -Code "status_health" -Status "warning" -Detail "127.0.0.1:${statusPort} is unavailable"))
  }

  $identityPath = Resolve-HanakoBridgePath -InstallRoot $root -Path (Join-Path ([string]$config.storage.dataDir) "cloud-identity.json")
  $identity = [pscustomobject]@{
    path = $identityPath
    credentialPresent = $false
    claimTokenPresent = $false
    publicKeyFingerprint = ""
    updatedAt = ""
  }
  try {
    $rawIdentity = Get-Content -LiteralPath $identityPath -Raw | ConvertFrom-Json
    $identity = [pscustomobject]@{
      path = $identityPath
      credentialPresent = -not [string]::IsNullOrWhiteSpace([string]$rawIdentity.credential)
      claimTokenPresent = -not [string]::IsNullOrWhiteSpace([string]$rawIdentity.claimToken)
      publicKeyFingerprint = [string]$rawIdentity.publicKeyFingerprint
      updatedAt = [string]$rawIdentity.updatedAt
    }
  } catch {}

  $cloud = if ($mcpHealth -and $mcpHealth.cloud) {
    $mcpHealth.cloud
  } else {
    [pscustomobject]@{
      status = if ([bool]$config.cloud.enabled) { "offline" } else { "disabled" }
      claimToken = $null
      publicKeyFingerprint = $identity.publicKeyFingerprint
      cloudUrl = [string]$config.cloud.url
      lastConnectedAt = $null
      lastSeenAt = $null
      lastError = if ($mcpError) { $mcpError } else { $null }
    }
  }

  if (-not [bool]$config.cloud.enabled) {
    $checks.Add((New-HanakoBridgeCheck -Code "cloud" -Status "warning" -Detail "Cloud WebSocket is disabled"))
  } elseif ([string]$cloud.status -eq "active") {
    $checks.Add((New-HanakoBridgeCheck -Code "cloud" -Status "pass" -Detail "Cloud WebSocket is active"))
  } elseif ([string]$cloud.status -eq "pending_claim") {
    $checks.Add((New-HanakoBridgeCheck -Code "cloud" -Status "warning" -Detail "Connected and waiting for this device to be claimed"))
  } else {
    $cloudDetail = [string]$cloud.status
    if ([string]$cloud.lastError) { $cloudDetail = "$cloudDetail - $($cloud.lastError)" }
    $checks.Add((New-HanakoBridgeCheck -Code "cloud" -Status "error" -Detail $cloudDetail))
  }

  if ($identity.credentialPresent) {
    $checks.Add((New-HanakoBridgeCheck -Code "identity" -Status "pass" -Detail "Device credential is present"))
  } elseif ($identity.claimTokenPresent) {
    $checks.Add((New-HanakoBridgeCheck -Code "identity" -Status "warning" -Detail "Waiting for web login and claim"))
  } else {
    $checks.Add((New-HanakoBridgeCheck -Code "identity" -Status "error" -Detail "Device identity is missing"))
  }

  if ([bool]$config.tunnel.enabled) {
    $checks.Add((New-HanakoBridgeCheck -Code "tunnel" -Status "warning" -Detail "Legacy SSH tunnel is enabled"))
  } else {
    $checks.Add((New-HanakoBridgeCheck -Code "tunnel" -Status "pass" -Detail "Legacy SSH tunnel is disabled"))
  }

  $hasError = @($checks | Where-Object { $_.status -eq "error" }).Count -gt 0
  $hasWarning = @($checks | Where-Object { $_.status -eq "warning" }).Count -gt 0
  $overall = if ($hasError) { "error" } elseif ($hasWarning) { "warning" } else { "healthy" }
  $recommendation = if ($overall -eq "healthy") {
    "Local bridge and cloud connection are healthy."
  } elseif ([string]$cloud.status -eq "pending_claim") {
    "This computer is connected but not claimed. Log in on the Cloud devices tab and claim it."
  } elseif (-not $mcpTask -or -not $nodeProcess -or -not $mcpHealth) {
    "The background service is not healthy. Run Detect and Repair."
  } else {
    "Review the diagnostic list for the first warning or error."
  }

  [pscustomobject]@{
    capturedAt = (Get-Date).ToString("o")
    overall = $overall
    recommendation = $recommendation
    installRoot = $root
    configPath = [string]$runtime.configPath
    version = $packageVersion
    device = [pscustomobject]@{
      id = [string]$config.device.id
      name = [string]$config.device.name
      hostname = if ($mcpHealth -and $mcpHealth.device) { [string]$mcpHealth.device.hostname } else { $env:COMPUTERNAME }
    }
    local = [pscustomobject]@{
      mcpPort = $mcpPort
      statusPort = $statusPort
      mcpHealthy = [bool]($mcpHealth -and $mcpHealth.ok -eq $true)
      statusHealthy = [bool]($statusHealth -and $statusHealth.ok -eq $true)
      mcpError = $mcpError
      statusError = $statusError
      trustMode = if ($mcpHealth) { [string]$mcpHealth.trustMode } else { [string]$config.filesystem.trustMode }
    }
    cloud = [pscustomobject]@{
      enabled = [bool]$config.cloud.enabled
      url = [string]$config.cloud.url
      webBaseUrl = ConvertTo-HanakoCloudWebBase -CloudUrl ([string]$config.cloud.url)
      status = [string]$cloud.status
      lastConnectedAt = [string]$cloud.lastConnectedAt
      lastSeenAt = [string]$cloud.lastSeenAt
      lastError = [string]$cloud.lastError
    }
    identity = $identity
    tasks = [pscustomobject]@{
      mcpName = $tasks.Mcp
      mcpState = if ($mcpTask) { [string]$mcpTask.State } else { "Missing" }
      tunnelName = $tasks.Tunnel
      tunnelState = if ($tunnelTask) { [string]$tunnelTask.State } else { "Missing" }
      hiddenLauncher = $hiddenLauncher
      mcpAction = $mcpAction
    }
    processes = $bridgeProcesses
    checks = @($checks)
    error = ""
  }
}

function Wait-HanakoBridgeManagerHealthy {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ConfigPath = "",
    [int]$TimeoutSeconds = 35
  )

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  do {
    $snapshot = Get-HanakoBridgeManagerSnapshot -InstallRoot $InstallRoot -ConfigPath $ConfigPath
    $cloudReady = -not $snapshot.cloud.enabled -or $snapshot.cloud.status -in @("active", "pending_claim")
    if ($snapshot.local.mcpHealthy -and $cloudReady) { return $snapshot }
    Start-Sleep -Milliseconds 700
  } while ((Get-Date) -lt $deadline)
  Get-HanakoBridgeManagerSnapshot -InstallRoot $InstallRoot -ConfigPath $ConfigPath
}

function Invoke-HanakoBridgeManagerAction {
  param(
    [ValidateSet("start", "stop", "restart", "repair")][string]$Action,
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ConfigPath = ""
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $runtime = Get-BridgeRuntime -InstallRoot $root -ConfigPath $ConfigPath
  $tasks = Get-BridgeTaskNames -Runtime $runtime

  switch ($Action) {
    "start" {
      if (-not (Get-ScheduledTask -TaskName $tasks.Mcp -ErrorAction SilentlyContinue)) {
        throw "The MCP scheduled task is missing. Run repair."
      }
      Start-ScheduledTask -TaskName $tasks.Mcp -ErrorAction Stop
      if ([bool]$runtime.config.tunnel.enabled) {
        Start-ScheduledTask -TaskName $tasks.Tunnel -ErrorAction SilentlyContinue
      }
    }
    "stop" {
      & (Join-Path $root "stop.ps1") -ConfigPath $runtime.configPath | Out-Null
    }
    "restart" {
      & (Join-Path $root "stop.ps1") -ConfigPath $runtime.configPath | Out-Null
      Start-Sleep -Seconds 1
      if (-not (Get-ScheduledTask -TaskName $tasks.Mcp -ErrorAction SilentlyContinue)) {
        throw "The MCP scheduled task is missing. Run repair."
      }
      Start-ScheduledTask -TaskName $tasks.Mcp -ErrorAction Stop
      if ([bool]$runtime.config.tunnel.enabled) {
        Start-ScheduledTask -TaskName $tasks.Tunnel -ErrorAction SilentlyContinue
      }
    }
    "repair" {
      & (Join-Path $root "repair.ps1") -ConfigPath $runtime.configPath -NonInteractive | Out-Null
    }
  }

  if ($Action -eq "stop") {
    Start-Sleep -Seconds 1
    return Get-HanakoBridgeManagerSnapshot -InstallRoot $root -ConfigPath $runtime.configPath
  }
  Wait-HanakoBridgeManagerHealthy -InstallRoot $root -ConfigPath $runtime.configPath
}

function Invoke-HanakoBridgeCloudQuery {
  param(
    [string]$BaseUrl,
    [string]$AccessKey,
    [switch]$ClaimCurrentDevice,
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ConfigPath = ""
  )

  $base = ConvertTo-HanakoCloudWebBase -CloudUrl $BaseUrl
  if ([string]::IsNullOrWhiteSpace($base)) { throw "Hana web base URL is invalid." }
  if ([string]::IsNullOrWhiteSpace($AccessKey)) { throw "Enter the Hana web access key." }

  $session = [Microsoft.PowerShell.Commands.WebRequestSession]::new()
  $loginBody = @{
    credential = $AccessKey
    clientKind = "desktop"
  } | ConvertTo-Json
  Invoke-RestMethod `
    -Uri "$base/api/web-auth/login" `
    -Method Post `
    -WebSession $session `
    -ContentType "application/json" `
    -Body $loginBody `
    -TimeoutSec 20 | Out-Null

  $claimed = $false
  $claimMessage = ""
  if ($ClaimCurrentDevice) {
    $snapshot = Get-HanakoBridgeManagerSnapshot -InstallRoot $InstallRoot -ConfigPath $ConfigPath
    if (-not $snapshot.local) { throw "Local bridge status is unavailable." }
    $identity = Invoke-RestMethod `
      -Uri "http://127.0.0.1:$($snapshot.local.statusPort)/api/client-identity" `
      -TimeoutSec 8
    if ($identity.cloud.claimToken) {
      $claimBody = @{
        deviceId = $identity.device.id
        claimToken = $identity.cloud.claimToken
        publicKeyFingerprint = $identity.cloud.publicKeyFingerprint
      } | ConvertTo-Json
      Invoke-RestMethod `
        -Uri "$base/api/local-bridge/claim" `
        -Method Post `
        -WebSession $session `
        -ContentType "application/json" `
        -Body $claimBody `
        -TimeoutSec 20 | Out-Null
      $claimed = $true
      $claimMessage = "Current device claimed."
      Start-Sleep -Seconds 1
    } else {
      $claimMessage = "Current device has no pending claim token."
    }
  }

  $result = Invoke-RestMethod `
    -Uri "$base/api/local-bridge/devices" `
    -WebSession $session `
    -TimeoutSec 20
  [pscustomobject]@{
    claimed = $claimed
    claimMessage = $claimMessage
    devices = @($result.devices)
    baseUrl = $base
  }
}

function Get-HanakoBridgeLogFiles {
  param([string]$InstallRoot = $PSScriptRoot)

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $runtime = Get-BridgeRuntime -InstallRoot $root
  $logDir = Resolve-HanakoBridgePath -InstallRoot $root -Path ([string]$runtime.config.storage.logDir)
  if (-not (Test-Path -LiteralPath $logDir -PathType Container)) { return @() }
  @(
    Get-ChildItem -LiteralPath $logDir -File -Filter "*.log*" -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending |
      Select-Object Name, FullName, Length, LastWriteTime
  )
}

function Get-HanakoBridgeLogTail {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [int]$Lines = 250
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return "" }
  (Get-Content -LiteralPath $Path -Tail ([Math]::Max(1, $Lines)) -ErrorAction SilentlyContinue) -join [Environment]::NewLine
}
