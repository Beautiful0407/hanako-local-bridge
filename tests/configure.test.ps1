$ErrorActionPreference = "Stop"

function Assert-Configure {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tempRoot = Join-Path $env:TEMP "HanakoConfigureTest-$([Guid]::NewGuid().ToString('N'))"
$primaryRoot = Join-Path $tempRoot "primary"
$secondaryRoot = Join-Path $tempRoot "secondary"
$configPath = Join-Path $tempRoot "config.json"

try {
  New-Item -ItemType Directory -Force -Path $primaryRoot, $secondaryRoot | Out-Null
  $config = Get-Content -LiteralPath (Join-Path $projectRoot "config.example.json") -Raw |
    ConvertFrom-Json
  $config.filesystem.roots = @(
    [pscustomobject]@{
      name = "Primary"
      path = $primaryRoot
      mode = "read_write"
    },
    [pscustomobject]@{
      name = "Secondary"
      path = $secondaryRoot
      mode = "read_write"
    },
    [pscustomobject]@{
      name = "HanakoLocalBridge"
      path = $projectRoot
      mode = "read"
    }
  )
  $config.update.manifest = "https://updates.example/manifest.json"
  [System.IO.File]::WriteAllText(
    $configPath,
    ($config | ConvertTo-Json -Depth 12),
    [System.Text.UTF8Encoding]::new($false)
  )

  & (Join-Path $projectRoot "configure.ps1") `
    -ConfigPath $configPath `
    -DeviceId "configure-test" `
    -DeviceName "Configure Test" `
    -RootPath $primaryRoot `
    -VpsHost "example.invalid" `
    -CloudUrl "wss://example.invalid/local-bridge/connect" `
    -SshUser "root" `
    -McpPort 29887 `
    -ApprovalPort 29888 `
    -RemotePort 29889 `
    -TaskPrefix "Hanako Configure Test" `
    -DisableTunnel `
    -NonInteractive | Out-Null

  $updated = [System.IO.File]::ReadAllText(
    $configPath,
    [System.Text.Encoding]::UTF8
  ) | ConvertFrom-Json
  $roots = @($updated.filesystem.roots)
  Assert-Configure ($roots.Count -eq 3) "Saving settings discarded configured roots."
  Assert-Configure (
    @($roots | Where-Object { $_.path -eq $secondaryRoot -and $_.mode -eq "read_write" }).Count -eq 1
  ) "The secondary writable root was not preserved."
  Assert-Configure (
    @($roots | Where-Object { $_.name -eq "HanakoLocalBridge" -and $_.mode -eq "read" }).Count -eq 1
  ) "The installation read root was not preserved."
  Assert-Configure (
    [string]$updated.update.manifest -eq "https://updates.example/manifest.json"
  ) "Saving settings discarded the update manifest."

  Write-Host "configure tests passed"
} finally {
  if (Test-Path -LiteralPath $tempRoot) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
