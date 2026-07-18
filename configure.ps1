param(
  [string]$InstallRoot = $PSScriptRoot,
  [string]$ConfigPath = "",
  [string]$DeviceId = "",
  [string]$DeviceName = "",
  [string]$RootPath = "",
  [string]$RootName = "",
  [string]$VpsHost = "",
  [string]$SshUser = "",
  [string]$IdentityFile = "",
  [int]$McpPort = 0,
  [int]$ApprovalPort = 0,
  [int]$RemotePort = 0,
  [string]$TaskPrefix = "",
  [string]$UpdateManifest = "",
  [string]$CloudUrl = "",
  [switch]$DisableCloud,
  [switch]$UseLegacySshTunnel,
  [switch]$DisableTunnel,
  [switch]$NonInteractive
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "bridge-common.ps1")
$officialCloudUrl = "wss://154-201-69-202.sslip.io/local-bridge/connect"
$legacyCloudUrl = "ws://154.201.69.202/local-bridge/connect"
$officialUpdateManifest = "https://154-201-69-202.sslip.io/local-bridge/releases/update-manifest.json"

function Read-BridgeValue {
  param(
    [string]$Label,
    [string]$Default
  )

  $value = Read-Host "$Label [$Default]"
  if ([string]::IsNullOrWhiteSpace($value)) { return $Default }
  return $value.Trim()
}

function ConvertTo-BridgeDeviceId {
  param([string]$Value)

  $id = $Value.Trim().ToLowerInvariant() -replace "[^a-z0-9._-]+", "-"
  return $id.Trim("-").Substring(0, [Math]::Min(64, $id.Trim("-").Length))
}

$installRoot = Get-BridgeInstallRoot -InstallRoot $InstallRoot
if ([string]::IsNullOrWhiteSpace($ConfigPath)) {
  $ConfigPath = Join-Path $installRoot "config.json"
}
$runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
$config = $runtime.config

$identityPath = Join-Path ([string]$config.storage.dataDir) "device.json"
if (-not $runtime.exists -and (Test-Path -LiteralPath $identityPath -PathType Leaf)) {
  try {
    $identity = Get-Content -LiteralPath $identityPath -Raw | ConvertFrom-Json
    if ($identity.id) { $config.device.id = [string]$identity.id }
    if ($identity.name) { $config.device.name = [string]$identity.name }
  } catch {}
}

$currentRoot = $config.filesystem.roots | Where-Object { $_.mode -eq "read_write" } | Select-Object -First 1
if (-not $currentRoot) { $currentRoot = $config.filesystem.roots | Select-Object -First 1 }

if (-not $NonInteractive) {
  Write-Host ""
  Write-Host "Hanako Local Bridge configuration"
  Write-Host "Press Enter to keep the value in brackets."
  Write-Host ""
  $DeviceName = Read-BridgeValue -Label "Device name" -Default ([string]$config.device.name)
  $DeviceId = Read-BridgeValue -Label "Device ID" -Default ([string]$config.device.id)
  $RootPath = Read-BridgeValue -Label "Writable local root" -Default ([string]$currentRoot.path)
  $VpsHost = Read-BridgeValue -Label "VPS host" -Default ([string]$config.tunnel.server)
  $SshUser = Read-BridgeValue -Label "SSH user" -Default ([string]$config.tunnel.user)
  $McpPort = [int](Read-BridgeValue -Label "Local MCP port" -Default ([string]$config.filesystem.port))
  $ApprovalPort = [int](Read-BridgeValue -Label "Local status port" -Default ([string]$config.filesystem.approvalPort))
  $RemotePort = [int](Read-BridgeValue -Label "VPS reverse tunnel port" -Default ([string]$config.tunnel.remotePort))
}

if ([string]::IsNullOrWhiteSpace($DeviceName)) { $DeviceName = [string]$config.device.name }
if ([string]::IsNullOrWhiteSpace($DeviceId)) { $DeviceId = [string]$config.device.id }
$DeviceId = ConvertTo-BridgeDeviceId -Value $DeviceId
if ([string]::IsNullOrWhiteSpace($DeviceId)) { throw "Device ID cannot be empty." }
if ([string]::IsNullOrWhiteSpace($RootPath)) { $RootPath = [string]$currentRoot.path }
$RootPath = [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($RootPath))
if (-not (Test-Path -LiteralPath $RootPath -PathType Container)) {
  throw "Writable root does not exist: $RootPath"
}
if ([string]::IsNullOrWhiteSpace($RootName)) {
  $RootName = Split-Path -Leaf $RootPath
  if ([string]::IsNullOrWhiteSpace($RootName)) { $RootName = "LocalFiles" }
}
if ([string]::IsNullOrWhiteSpace($VpsHost)) { $VpsHost = [string]$config.tunnel.server }
if ([string]::IsNullOrWhiteSpace($SshUser)) { $SshUser = [string]$config.tunnel.user }
if ($McpPort -le 0) { $McpPort = [int]$config.filesystem.port }
if ($ApprovalPort -le 0) { $ApprovalPort = [int]$config.filesystem.approvalPort }
if ($RemotePort -le 0) { $RemotePort = [int]$config.tunnel.remotePort }
if ([string]::IsNullOrWhiteSpace($CloudUrl)) {
  $CloudUrl = [string]$config.cloud.url
}
if ([string]::IsNullOrWhiteSpace($CloudUrl) -or $CloudUrl -eq $legacyCloudUrl) {
  $CloudUrl = $officialCloudUrl
}
if ([string]::IsNullOrWhiteSpace($CloudUrl) -and -not [string]::IsNullOrWhiteSpace($VpsHost)) {
  $CloudUrl = "ws://${VpsHost}/local-bridge/connect"
}

$config.device.id = $DeviceId
$config.device.name = $DeviceName
$config.filesystem.host = "127.0.0.1"
$config.filesystem.port = $McpPort
$config.filesystem.approvalPort = $ApprovalPort
$config.filesystem.trustMode = "full"
$config.filesystem.allowChatAuthorization = $false
$updatedRoots = [System.Collections.Generic.List[object]]::new()
$updatedRoots.Add([pscustomobject]@{
  name = $RootName
  path = $RootPath
  mode = "read_write"
})
$currentRootPath = if ($currentRoot -and $currentRoot.path) {
  [System.IO.Path]::GetFullPath(
    [Environment]::ExpandEnvironmentVariables([string]$currentRoot.path)
  )
} else {
  ""
}
foreach ($existingRoot in @($config.filesystem.roots)) {
  if (-not $existingRoot.path) { continue }
  $existingPath = [System.IO.Path]::GetFullPath(
    [Environment]::ExpandEnvironmentVariables([string]$existingRoot.path)
  )
  if (
    $existingPath.Equals($RootPath, [System.StringComparison]::OrdinalIgnoreCase) -or
    (
      $currentRootPath -and
      $existingPath.Equals($currentRootPath, [System.StringComparison]::OrdinalIgnoreCase)
    ) -or
    $existingPath.Equals($installRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
    [string]$existingRoot.name -eq "HanakoLocalBridge"
  ) {
    continue
  }
  $updatedRoots.Add([pscustomobject]@{
    name = [string]$existingRoot.name
    path = $existingPath
    mode = if ([string]$existingRoot.mode -eq "read_write") { "read_write" } else { "read" }
  })
}
$updatedRoots.Add([pscustomobject]@{
  name = "HanakoLocalBridge"
  path = $installRoot
  mode = "read"
})
$config.filesystem.roots = @($updatedRoots)
$config.storage.dataDir = "data"
$config.storage.logDir = "logs"
$config.cloud.enabled = -not $DisableCloud
$config.cloud.url = $CloudUrl
$config.tunnel.enabled = $UseLegacySshTunnel -and -not $DisableTunnel -and -not $config.cloud.enabled
$config.tunnel.server = $VpsHost
$config.tunnel.user = $SshUser
$config.tunnel.localHost = "127.0.0.1"
$config.tunnel.localPort = $McpPort
$config.tunnel.remoteHost = "127.0.0.1"
$config.tunnel.remotePort = $RemotePort
if (-not [string]::IsNullOrWhiteSpace($IdentityFile)) {
  $config.tunnel.identityFile = [System.IO.Path]::GetFullPath(
    [Environment]::ExpandEnvironmentVariables($IdentityFile)
  )
}
if (-not [string]::IsNullOrWhiteSpace($TaskPrefix)) {
  $config.service.taskPrefix = $TaskPrefix.Trim()
}
if ([string]::IsNullOrWhiteSpace($UpdateManifest)) {
  $existingManifest = [string]$config.update.manifest
  if (
    [string]::IsNullOrWhiteSpace($existingManifest) -or
    $existingManifest -match "(?i)\\Desktop\\Hanako-Local-FS-MCP-Bridge\\release\\update-manifest\.json$"
  ) {
    $UpdateManifest = $officialUpdateManifest
  }
}
if (-not [string]::IsNullOrWhiteSpace($UpdateManifest)) {
  $config.update.manifest = $UpdateManifest.Trim()
}

Write-BridgeJson -Value $config -Path $ConfigPath
Write-Host "Configuration saved: $ConfigPath"
Write-Host "Device: $DeviceId ($DeviceName)"
Write-Host "Local MCP: http://127.0.0.1:${McpPort}/mcp"
Write-Host "Local status: http://127.0.0.1:${ApprovalPort}/"
if ($config.cloud.enabled) {
  Write-Host "Cloud WebSocket: $($config.cloud.url)"
} elseif ($config.tunnel.enabled) {
  Write-Host "Tunnel: ${SshUser}@${VpsHost} -> 127.0.0.1:${RemotePort}"
} else {
  Write-Host "Tunnel: disabled"
}
