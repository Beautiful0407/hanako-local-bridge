<#
Builds the NSIS wizard installer that wraps the Rust HanakoLocalBridge-Setup.exe.

Prereq: the Rust setup exe must already be built (cargo build -p hanako-bootstrap
--release with HANA_INSTALLER_PAYLOAD set, i.e. the normal pack step).

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-wizard-installer.ps1 `
    -Version 2.0.0-alpha.21 `
    -SetupExe "D:\work-cc\hanako--MCP-\build\rust-release-alpha21\HanakoLocalBridge-Setup-2.0.0-alpha.21.exe" `
    -OutFile  "D:\work-cc\hanako--MCP-\build\rust-release-alpha21\HanakoLocalBridge-Wizard-Setup-2.0.0-alpha.21.exe"
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [Parameter(Mandatory = $true)][string]$SetupExe,
  [Parameter(Mandatory = $true)][string]$OutFile
)
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$nsiPath  = Join-Path $repoRoot 'installer\wizard.nsi'

if (-not (Test-Path $nsiPath))  { throw "wizard.nsi not found: $nsiPath" }
if (-not (Test-Path $SetupExe)) { throw "Rust setup exe not found: $SetupExe (build it first)" }

# Locate makensis: prefer a standalone NSIS, fall back to the electron-builder cache.
$candidates = @(
  "$env:ProgramFiles\NSIS\makensis.exe",
  "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
)
$cacheRoot = Join-Path $env:LOCALAPPDATA 'electron-builder\Cache\nsis'
if (Test-Path $cacheRoot) {
  Get-ChildItem $cacheRoot -Recurse -Filter 'makensis.exe' -ErrorAction SilentlyContinue |
    ForEach-Object { $candidates += $_.FullName }
}
$makensis = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $makensis) {
  throw "makensis.exe not found. Install NSIS or ensure electron-builder's NSIS cache exists under $cacheRoot"
}
Write-Host "makensis: $makensis"

$outDir = Split-Path -Parent $OutFile
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir -Force | Out-Null }

# NSIS wants absolute paths for /D defines.
$SetupExe = (Resolve-Path $SetupExe).Path
$args = @(
  "/DVERSION=$Version",
  "/DSETUP_EXE=$SetupExe",
  "/DOUT_FILE=$OutFile",
  $nsiPath
)
Write-Host "compiling wizard: $OutFile"
& $makensis @args
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }

if (-not (Test-Path $OutFile)) { throw "wizard installer was not produced: $OutFile" }
$size = (Get-Item $OutFile).Length
Write-Host "wizard installer built: $OutFile ($size bytes)"
