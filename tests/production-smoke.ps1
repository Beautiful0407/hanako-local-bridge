param([string]$Value = "ok")

[Console]::OutputEncoding = [Text.UTF8Encoding]::new()
Write-Output "HANAKO_SMOKE:$Value"
