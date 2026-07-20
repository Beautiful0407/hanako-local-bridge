$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$buildRoot = Join-Path $repo "build"
$runId = [Guid]::NewGuid().ToString("N")
$installRoot = Join-Path $buildRoot "rust-update-smoke-$runId"
$profileRoot = Join-Path $installRoot "profile"
$appDataRoot = Join-Path $profileRoot "AppData\Roaming"
$localAppDataRoot = Join-Path $profileRoot "AppData\Local"
$registrySubKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustUpdateSmoke-$runId"
$rollbackInstallRoot = Join-Path $buildRoot "rust-update-rollback-smoke-$runId"
$rollbackProfileRoot = Join-Path $rollbackInstallRoot "profile"
$rollbackAppDataRoot = Join-Path $rollbackProfileRoot "AppData\Roaming"
$rollbackLocalAppDataRoot = Join-Path $rollbackProfileRoot "AppData\Local"
$rollbackRegistrySubKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge-RustUpdateRollbackSmoke-$runId"
$installer = Join-Path $buildRoot "rust-release-alpha11\HanakoLocalBridge-Setup-2.0.0-alpha.11.exe"
$alpha10Package = Join-Path $buildRoot "rust-release-alpha10\HanakoLocalBridge-2.0.0-alpha.10-win-x64.zip"
$alpha11Manifest = Join-Path $buildRoot "rust-release-alpha11\update-manifest.json"
$currentMaintenance = Join-Path $repo "target\release\hanako-maintenance.exe"
$oldUserProfile = $env:USERPROFILE
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldUninstallKey = $env:HANA_INSTALLER_UNINSTALL_KEY
$passed = $false

function Assert-Path([string]$Path, [string]$Message) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw $Message
  }
}

function Set-ShortcutTarget([string]$Path, [string]$Target) {
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
  $shell = New-Object -ComObject WScript.Shell
  try {
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = $Target
    $shortcut.IconLocation = $Target
    $shortcut.Save()
  } finally {
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
  }
}

function Get-ShortcutTarget([string]$Path) {
  $shell = New-Object -ComObject WScript.Shell
  try {
    return $shell.CreateShortcut($Path).TargetPath
  } finally {
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
  }
}

function Invoke-GuiProcess([string]$FilePath, [string[]]$Arguments) {
  $quoted = $Arguments | ForEach-Object {
    if ($_ -match '[\s"]') {
      '"' + $_.Replace('"', '\"') + '"'
    } else {
      $_
    }
  }
  $process = Start-Process -FilePath $FilePath -ArgumentList ($quoted -join " ") -Wait -PassThru -WindowStyle Hidden
  return $process.ExitCode
}

function Wait-UpdateState([string]$StatePath, [string]$Description) {
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  $state = $null
  $lastStateError = ""
  while ([DateTime]::UtcNow -lt $deadline) {
    if (Test-Path -LiteralPath $StatePath) {
      try {
        $state = Get-Content -LiteralPath $StatePath -Raw -Encoding utf8 | ConvertFrom-Json
        if (@("succeeded", "failed") -contains [string]$state.status) {
          return $state
        }
      } catch {
        $lastStateError = $_.Exception.Message
      }
    }
    Start-Sleep -Milliseconds 200
  }
  throw "$Description did not finish at $StatePath. Last read error: $lastStateError. State: $($state | ConvertTo-Json -Compress)"
}

try {
  Assert-Path $installer "Rust Alpha 11 installer is missing."
  Assert-Path $alpha10Package "Rust Alpha 10 payload is missing."
  Assert-Path $alpha11Manifest "Rust Alpha 11 update manifest is missing."
  Assert-Path $currentMaintenance "Current Rust maintenance binary is missing."
  New-Item -ItemType Directory -Force -Path $installRoot, $profileRoot, $appDataRoot, $localAppDataRoot | Out-Null
  $env:USERPROFILE = $profileRoot
  $env:APPDATA = $appDataRoot
  $env:LOCALAPPDATA = $localAppDataRoot
  $env:HANA_INSTALLER_UNINSTALL_KEY = $registrySubKey

  $exitCode = Invoke-GuiProcess $installer @(
    "--payload",
    $alpha10Package,
    "--test-mode",
    "--install-root",
    $installRoot
  )
  if ($exitCode -ne 0) {
    throw "Installing the Alpha 10 payload failed with exit code $exitCode."
  }
  $alpha10Payload = Get-Content -LiteralPath (Join-Path $installRoot "payload-manifest.json") -Raw | ConvertFrom-Json
  if ($alpha10Payload.version -ne "2.0.0-alpha.10") {
    throw "The update fixture was not installed at Alpha 10."
  }

  # Exercise the current updater against an older installed payload.
  $maintenance = Join-Path $installRoot "hanako-maintenance.exe"
  $oldMaintenanceHash = (Get-FileHash -LiteralPath $maintenance -Algorithm SHA256).Hash
  Copy-Item -LiteralPath $currentMaintenance -Destination $maintenance -Force
  $currentMaintenanceHash = (Get-FileHash -LiteralPath $currentMaintenance -Algorithm SHA256).Hash
  if ((Get-FileHash -LiteralPath $maintenance -Algorithm SHA256).Hash -ne $currentMaintenanceHash) {
    throw "The current maintenance binary was not injected into the Alpha 10 fixture."
  }
  if ($oldMaintenanceHash -eq $currentMaintenanceHash) {
    throw "The Alpha 10 fixture already contains the current maintenance binary."
  }

  New-Item -ItemType Directory -Force -Path `
    (Join-Path $installRoot "data"), `
    (Join-Path $installRoot "logs") | Out-Null
  Set-Content -LiteralPath (Join-Path $installRoot "data\preserve-data.txt") -Value "keep-data" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $installRoot "logs\preserve-log.txt") -Value "keep-log" -Encoding utf8
  Set-Content -LiteralPath (Join-Path $installRoot "unknown-user-file.txt") -Value "keep-unknown" -Encoding utf8
  $configBefore = Get-FileHash -LiteralPath (Join-Path $installRoot "config.json") -Algorithm SHA256
  $bridgeBefore = Get-FileHash -LiteralPath (Join-Path $installRoot "hanako-bridge.exe") -Algorithm SHA256
  $managerPath = Join-Path $installRoot "hanako-manager.exe"
  $bridgePath = Join-Path $installRoot "hanako-bridge.exe"
  $desktopShortcut = Join-Path $profileRoot "Desktop\Hanako Local Bridge.lnk"
  $startMenuShortcut = Join-Path $appDataRoot "Microsoft\Windows\Start Menu\Programs\Hanako Local Bridge\Hanako Local Bridge.lnk"
  Set-ShortcutTarget $desktopShortcut $managerPath
  Set-ShortcutTarget $startMenuShortcut $managerPath
  $uninstallKey = "HKCU:\$registrySubKey"
  New-Item -Path $uninstallKey -Force | Out-Null
  Set-ItemProperty -Path $uninstallKey -Name "DisplayIcon" -Value $managerPath
  Set-ItemProperty -Path $uninstallKey -Name "DisplayVersion" -Value "2.0.0-alpha.10"

  $output = & $maintenance apply `
    --install-root $installRoot `
    --manifest $alpha11Manifest `
    --expected-version "2.0.0-alpha.11" `
    --test-mode
  if ($LASTEXITCODE -ne 0) {
    throw "Alpha 10 maintenance launcher failed with exit code $LASTEXITCODE."
  }
  $handoff = $output | ConvertFrom-Json
  if (-not $handoff.started) {
    throw "Alpha 10 maintenance launcher did not confirm worker handoff."
  }

  $statePath = [string]$handoff.statePath
  $state = Wait-UpdateState $statePath "Alpha 10 to Alpha 11 update"
  if ($state.status -ne "succeeded") {
    throw "Alpha 10 to Alpha 11 update failed: $($state | ConvertTo-Json -Compress)"
  }
  if ($state.installedVersion -ne "2.0.0-alpha.11") {
    throw "Update state did not report Alpha 11."
  }

  $payload = Get-Content -LiteralPath (Join-Path $installRoot "payload-manifest.json") -Raw | ConvertFrom-Json
  if ($payload.version -ne "2.0.0-alpha.11") {
    throw "Installed payload did not advance to Alpha 11."
  }
  $bridgeAfter = Get-FileHash -LiteralPath (Join-Path $installRoot "hanako-bridge.exe") -Algorithm SHA256
  if ($bridgeAfter.Hash -eq $bridgeBefore.Hash) {
    throw "The bridge binary was not replaced during the update."
  }
  if ((Get-FileHash -LiteralPath (Join-Path $installRoot "config.json") -Algorithm SHA256).Hash -ne $configBefore.Hash) {
    throw "The update changed config.json."
  }
  foreach ($relative in @(
    "data\preserve-data.txt",
    "logs\preserve-log.txt",
    "unknown-user-file.txt"
  )) {
    Assert-Path (Join-Path $installRoot $relative) "The update removed persistent file $relative."
  }
  foreach ($shortcutPath in @($desktopShortcut, $startMenuShortcut)) {
    $shortcutTarget = Get-ShortcutTarget $shortcutPath
    if (-not [string]::Equals($shortcutTarget, $bridgePath, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Online update left product shortcut '$shortcutPath' pointing to '$shortcutTarget' instead of '$bridgePath'."
    }
  }
  $uninstallProperties = Get-ItemProperty -Path $uninstallKey
  if (-not [string]::Equals([string]$uninstallProperties.DisplayIcon, $bridgePath, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Online update left DisplayIcon pointing to '$($uninstallProperties.DisplayIcon)' instead of '$bridgePath'."
  }
  if ([string]$uninstallProperties.DisplayVersion -ne "2.0.0-alpha.11") {
    throw "Online update did not refresh DisplayVersion."
  }

  New-Item -ItemType Directory -Force -Path `
    $rollbackInstallRoot, `
    $rollbackProfileRoot, `
    $rollbackAppDataRoot, `
    $rollbackLocalAppDataRoot | Out-Null
  $exitCode = Invoke-GuiProcess $installer @(
    "--payload",
    $alpha10Package,
    "--test-mode",
    "--install-root",
    $rollbackInstallRoot
  )
  if ($exitCode -ne 0) {
    throw "Installing the rollback Alpha 10 fixture failed with exit code $exitCode."
  }
  $rollbackMaintenance = Join-Path $rollbackInstallRoot "hanako-maintenance.exe"
  Copy-Item -LiteralPath $currentMaintenance -Destination $rollbackMaintenance -Force
  $rollbackBridge = Join-Path $rollbackInstallRoot "hanako-bridge.exe"
  $rollbackBridgeBefore = (Get-FileHash -LiteralPath $rollbackBridge -Algorithm SHA256).Hash

  Set-Content -LiteralPath (Join-Path $rollbackProfileRoot "Desktop") -Value "block-shortcut-directory" -Encoding utf8
  $env:USERPROFILE = $rollbackProfileRoot
  $env:APPDATA = $rollbackAppDataRoot
  $env:LOCALAPPDATA = $rollbackLocalAppDataRoot
  $env:HANA_INSTALLER_UNINSTALL_KEY = $rollbackRegistrySubKey
  $rollbackOutput = & $rollbackMaintenance apply `
    --install-root $rollbackInstallRoot `
    --manifest $alpha11Manifest `
    --expected-version "2.0.0-alpha.11" `
    --test-mode
  if ($LASTEXITCODE -ne 0) {
    throw "Rollback maintenance launcher failed with exit code $LASTEXITCODE."
  }
  $rollbackHandoff = $rollbackOutput | ConvertFrom-Json
  $rollbackState = Wait-UpdateState ([string]$rollbackHandoff.statePath) "Shell integration rollback update"
  if ($rollbackState.status -ne "failed") {
    throw "Shell integration failure was not reported as a failed update: $($rollbackState | ConvertTo-Json -Compress)"
  }
  $rollbackPayload = Get-Content -LiteralPath (Join-Path $rollbackInstallRoot "payload-manifest.json") -Raw | ConvertFrom-Json
  if ($rollbackPayload.version -ne "2.0.0-alpha.10") {
    throw "Shell integration failure did not roll the payload back to Alpha 10."
  }
  if ((Get-FileHash -LiteralPath $rollbackBridge -Algorithm SHA256).Hash -ne $rollbackBridgeBefore) {
    throw "Shell integration failure did not restore the Alpha 10 bridge binary."
  }

  $passed = $true
  Write-Output "Rust Alpha 10 to Alpha 11 update smoke test passed"
} finally {
  if ($passed -and (Test-Path -LiteralPath $installRoot)) {
    Remove-Item -LiteralPath $installRoot -Recurse -Force
  }
  if ($passed -and (Test-Path -LiteralPath $rollbackInstallRoot)) {
    Remove-Item -LiteralPath $rollbackInstallRoot -Recurse -Force
  }
  Remove-Item -LiteralPath "HKCU:\$registrySubKey" -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "HKCU:\$rollbackRegistrySubKey" -Recurse -Force -ErrorAction SilentlyContinue
  $env:USERPROFILE = $oldUserProfile
  $env:APPDATA = $oldAppData
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:HANA_INSTALLER_UNINSTALL_KEY = $oldUninstallKey
}
