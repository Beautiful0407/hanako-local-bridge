$ErrorActionPreference = "Stop"

$launcher = Join-Path $PSScriptRoot "run-reverse-tunnel-hidden.vbs"
Start-Process -FilePath (Join-Path $env:WINDIR "System32\wscript.exe") `
  -ArgumentList @("//B", "//NoLogo", "`"$launcher`"") `
  -WorkingDirectory $PSScriptRoot `
  -WindowStyle Hidden

Write-Host "[reverse-tunnel] hidden watchdog requested"
