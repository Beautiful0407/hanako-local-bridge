$ErrorActionPreference = "Stop"

function Assert-ShellIntegration {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$testRoot = Join-Path $env:TEMP "HanakoShellIntegration-$([Guid]::NewGuid().ToString('N'))"
$installRoot = Join-Path $testRoot "install"
$programsRoot = Join-Path $testRoot "programs"
$desktopRoot = Join-Path $testRoot "desktop"

try {
  New-Item -ItemType Directory -Force -Path `
    $installRoot, `
    (Join-Path $installRoot "manager"), `
    $programsRoot, `
    $desktopRoot | Out-Null
  Copy-Item -LiteralPath (Join-Path $projectRoot "package.json") -Destination $installRoot
  [System.IO.File]::WriteAllText((Join-Path $installRoot "run-manager.vbs"), "' test launcher")
  [System.IO.File]::WriteAllText((Join-Path $installRoot "manager\HanakoBridgeManager.exe"), "test icon")

  . (Join-Path $projectRoot "bridge-common.ps1")
  $result = Install-HanakoBridgeShellIntegration `
    -InstallRoot $installRoot `
    -ProgramsRoot $programsRoot `
    -DesktopRoot $desktopRoot `
    -SkipRegistry

  Assert-ShellIntegration (Test-Path -LiteralPath $result.startMenu -PathType Leaf) "Start menu shortcut was not created."
  Assert-ShellIntegration (Test-Path -LiteralPath $result.desktop -PathType Leaf) "Desktop shortcut was not created."

  $shell = New-Object -ComObject WScript.Shell
  $desktopShortcut = $shell.CreateShortcut($result.desktop)
  Assert-ShellIntegration ($desktopShortcut.TargetPath -like "*\wscript.exe") "Desktop shortcut target is incorrect."
  Assert-ShellIntegration ($desktopShortcut.Arguments -like "*run-manager.vbs*") "Desktop shortcut arguments are incorrect."
  Assert-ShellIntegration ($desktopShortcut.WorkingDirectory -eq $installRoot) "Desktop shortcut working directory is incorrect."
  Assert-ShellIntegration `
    (Test-HanakoBridgeShellIntegrationEligible `
      -InstallRoot $installRoot `
      -DefaultInstallRoot $installRoot) `
    "Default install should repair missing shell integration during update."
  Assert-ShellIntegration `
    (-not (Test-HanakoBridgeShellIntegrationEligible `
      -InstallRoot $installRoot `
      -DefaultInstallRoot (Join-Path $testRoot "other-install"))) `
    "Portable installs must not create shell integration during update."

  $env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION = "1"
  Assert-ShellIntegration `
    (-not (Test-HanakoBridgeShellIntegrationEligible `
      -InstallRoot $installRoot `
      -DefaultInstallRoot $installRoot)) `
    "Explicitly isolated installs must not create shell integration during update."
  Remove-Item Env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION

  Remove-HanakoBridgeShellIntegration `
    -ProgramsRoot $programsRoot `
    -DesktopRoot $desktopRoot `
    -SkipRegistry
  Assert-ShellIntegration (-not (Test-Path -LiteralPath $result.startMenu)) "Start menu shortcut was not removed."
  Assert-ShellIntegration (-not (Test-Path -LiteralPath $result.desktop)) "Desktop shortcut was not removed."
  Write-Host "shell integration tests passed"
} finally {
  Remove-Item Env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION -ErrorAction SilentlyContinue
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
