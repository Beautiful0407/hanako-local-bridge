[CmdletBinding()]
param(
  [string]$Destination = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$source = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if ([string]::IsNullOrWhiteSpace($Destination)) {
  $Destination = Join-Path $env:USERPROFILE ".codex\skills\hanako-local-bridge-dev"
}
$destinationPath = [System.IO.Path]::GetFullPath($Destination)

if ([string]::Equals($source, $destinationPath, [System.StringComparison]::OrdinalIgnoreCase)) {
  Write-Output "Skill is already running from the installed location: $destinationPath"
  exit 0
}

[System.IO.Directory]::CreateDirectory($destinationPath) | Out-Null
Copy-Item -LiteralPath (Join-Path $source "SKILL.md") `
  -Destination (Join-Path $destinationPath "SKILL.md") -Force

foreach ($directoryName in @("agents", "references", "scripts")) {
  $sourceDirectory = Join-Path $source $directoryName
  if (-not (Test-Path -LiteralPath $sourceDirectory)) {
    continue
  }
  $destinationDirectory = Join-Path $destinationPath $directoryName
  [System.IO.Directory]::CreateDirectory($destinationDirectory) | Out-Null
  Copy-Item -Path (Join-Path $sourceDirectory "*") `
    -Destination $destinationDirectory -Recurse -Force
}

$sourceFiles = Get-ChildItem -LiteralPath $source -Recurse -File |
  ForEach-Object {
    $relative = $_.FullName.Substring($source.Length).TrimStart("\")
    [ordered]@{
      path = $relative
      sha256 = (
        Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
      ).Hash.ToLowerInvariant()
    }
  }
$destinationFiles = Get-ChildItem -LiteralPath $destinationPath -Recurse -File |
  ForEach-Object {
    $relative = $_.FullName.Substring($destinationPath.Length).TrimStart("\")
    [ordered]@{
      path = $relative
      sha256 = (
        Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
      ).Hash.ToLowerInvariant()
    }
  }

$mismatches = @()
foreach ($sourceFile in $sourceFiles) {
  $installed = @($destinationFiles | Where-Object path -eq $sourceFile.path) |
    Select-Object -First 1
  if (-not $installed -or $installed.sha256 -ne $sourceFile.sha256) {
    $mismatches += $sourceFile.path
  }
}
if ($mismatches.Count -gt 0) {
  throw "Installed skill verification failed: $($mismatches -join ', ')"
}

[ordered]@{
  ok = $true
  source = $source
  destination = $destinationPath
  files = @($sourceFiles).Count
  restartCodexRequired = $true
} | ConvertTo-Json -Depth 4
