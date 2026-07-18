$ErrorActionPreference = "Stop"

function Assert-Tray {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$source = Get-Content `
  -LiteralPath (Join-Path $projectRoot "manager-winui\TrayMessageDecoder.cs") `
  -Raw `
  -Encoding UTF8
Add-Type -TypeDefinition $source -Language CSharp

$doubleClick = [HanakoBridgeManager.TrayMessageDecoder]::GetNotificationCode(
  [IntPtr]::new([int64]((0x1234 -shl 16) -bor 0x0203))
)
Assert-Tray ($doubleClick -eq 0x0203) "Tray double-click notification code was not decoded from the low word."

$contextMenu = [HanakoBridgeManager.TrayMessageDecoder]::GetNotificationCode(
  [IntPtr]::new([int64]((0x1234 -shl 16) -bor 0x007B))
)
Assert-Tray ($contextMenu -eq 0x007B) "Tray context-menu notification code was not decoded from the low word."

Write-Host "system tray tests passed"
