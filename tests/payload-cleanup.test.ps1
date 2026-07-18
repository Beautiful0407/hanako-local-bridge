$ErrorActionPreference = "Stop"

function Assert-PayloadCleanup {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$testRoot = Join-Path $env:TEMP "HanakoPayloadCleanup-$([Guid]::NewGuid().ToString('N'))"

try {
  New-Item -ItemType Directory -Force -Path `
    (Join-Path $testRoot "manager\nested"), `
    (Join-Path $testRoot "lib"), `
    (Join-Path $testRoot "data"), `
    (Join-Path $testRoot "logs"), `
    (Join-Path $testRoot "custom-root") | Out-Null

  [System.IO.File]::WriteAllText((Join-Path $testRoot "manager\keep.dll"), "keep")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "manager\stale.dll"), "stale")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "manager\nested\stale.dll"), "stale")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "lib\keep.cjs"), "keep")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "lib\stale.cjs"), "stale")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "config.json"), "{}")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "data\state.json"), "{}")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "logs\bridge.log"), "log")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "custom-root\user.txt"), "user")
  [System.IO.File]::WriteAllText((Join-Path $testRoot "payload-cleanup.pending"), "test")
  [System.IO.File]::WriteAllText(
    (Join-Path $testRoot "payload-manifest.json"),
    (@{
      schemaVersion = 1
      version = "test"
      managedDirectories = @("manager", "lib")
      files = @("manager/keep.dll", "lib/keep.cjs", "payload-manifest.json")
    } | ConvertTo-Json -Depth 5)
  )

  . (Join-Path $projectRoot "bridge-common.ps1")
  $result = Invoke-HanakoBridgePayloadCleanup -InstallRoot $testRoot

  Assert-PayloadCleanup (-not $result.skipped) "Pending payload cleanup was skipped."
  Assert-PayloadCleanup ($result.removedFiles -eq 3) "Unexpected stale payload removal count."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "manager\keep.dll")) "Expected manager file was removed."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "lib\keep.cjs")) "Expected library file was removed."
  Assert-PayloadCleanup (-not (Test-Path -LiteralPath (Join-Path $testRoot "manager\stale.dll"))) "Stale manager file was preserved."
  Assert-PayloadCleanup (-not (Test-Path -LiteralPath (Join-Path $testRoot "lib\stale.cjs"))) "Stale library file was preserved."
  Assert-PayloadCleanup (-not (Test-Path -LiteralPath (Join-Path $testRoot "manager\nested"))) "Empty stale directory was preserved."
  Assert-PayloadCleanup (-not (Test-Path -LiteralPath (Join-Path $testRoot "payload-cleanup.pending"))) "Cleanup marker was preserved."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "config.json")) "Configuration was removed."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "data\state.json")) "Device data was removed."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "logs\bridge.log")) "Logs were removed."
  Assert-PayloadCleanup (Test-Path -LiteralPath (Join-Path $testRoot "custom-root\user.txt")) "User-created root files were removed."

  $second = Invoke-HanakoBridgePayloadCleanup -InstallRoot $testRoot
  Assert-PayloadCleanup ($second.skipped) "Completed payload cleanup should be idempotent."
  Write-Host "payload cleanup tests passed"
} finally {
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
