param([string]$InstallRoot = $PSScriptRoot)

$ErrorActionPreference = "Stop"
$root = [System.IO.Path]::GetFullPath($InstallRoot)
Start-Process `
  -FilePath (Join-Path $env:WINDIR "System32\wscript.exe") `
  -ArgumentList @("//B", "//NoLogo", "`"$(Join-Path $root 'run-manager.vbs')`"") `
  -WindowStyle Hidden
