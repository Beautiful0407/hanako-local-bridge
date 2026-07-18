param(
  [string]$InstallRoot = $PSScriptRoot,
  [string]$Manifest = ""
)

$ErrorActionPreference = "Continue"
$root = [System.IO.Path]::GetFullPath($InstallRoot)
$updateScript = Join-Path $root "update.ps1"
$managerLauncher = Join-Path $root "open-manager.ps1"
$logDirectory = Join-Path $root "logs"
$logPath = Join-Path $logDirectory "update.log"

New-Item -ItemType Directory -Force -Path $logDirectory | Out-Null
$arguments = @(
  "-NoLogo",
  "-NoProfile",
  "-NonInteractive",
  "-ExecutionPolicy",
  "Bypass",
  "-File",
  $updateScript
)
if (-not [string]::IsNullOrWhiteSpace($Manifest)) {
  $arguments += @("-Manifest", $Manifest)
}

& powershell.exe @arguments *>> $logPath
$exitCode = $LASTEXITCODE

if (Test-Path -LiteralPath $managerLauncher -PathType Leaf) {
  Start-Process `
    -FilePath (Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe") `
    -ArgumentList @(
      "-NoLogo",
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-WindowStyle",
      "Hidden",
      "-File",
      $managerLauncher,
      "-InstallRoot",
      $root
    ) `
    -WindowStyle Hidden
}

exit $exitCode
