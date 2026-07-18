param(
  [string]$OutputDir = "",
  [string]$NodePath = "",
  [string]$PublishedAt = ""
)

$ErrorActionPreference = "Stop"

function Assert-ChildPath {
  param(
    [Parameter(Mandatory = $true)][string]$Parent,
    [Parameter(Mandatory = $true)][string]$Child
  )
  $resolvedParent = [System.IO.Path]::GetFullPath($Parent).TrimEnd("\") + "\"
  $resolvedChild = [System.IO.Path]::GetFullPath($Child)
  if (-not $resolvedChild.StartsWith($resolvedParent, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe build path outside parent: $resolvedChild"
  }
  return $resolvedChild
}

$projectRoot = [System.IO.Path]::GetFullPath($PSScriptRoot)
$package = Get-Content -LiteralPath (Join-Path $projectRoot "package.json") -Raw | ConvertFrom-Json
$version = [string]$package.version
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $OutputDir = Join-Path $projectRoot "release"
}
$outputDir = [System.IO.Path]::GetFullPath($OutputDir)
$buildRoot = Assert-ChildPath -Parent $projectRoot -Child (Join-Path $projectRoot "build\installer")
$payloadRoot = Assert-ChildPath -Parent $buildRoot -Child (Join-Path $buildRoot "payload")
$managerPublishRoot = Assert-ChildPath -Parent $projectRoot -Child (Join-Path $projectRoot "build\manager-winui\win-x64")
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
$sfxRoot = [System.IO.Path]::GetFullPath(
  (Join-Path $env:TEMP "HanakoLocalBridgeIExpress-$PID-$([Guid]::NewGuid().ToString('N'))")
)
if (-not $sfxRoot.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Unsafe IExpress temporary path: $sfxRoot"
}

if (Test-Path -LiteralPath $buildRoot) {
  Remove-Item -LiteralPath $buildRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $payloadRoot, $outputDir | Out-Null

if ([string]::IsNullOrWhiteSpace($NodePath)) {
  $NodePath = Join-Path $env:ProgramFiles "nodejs\node.exe"
}
$NodePath = [System.IO.Path]::GetFullPath($NodePath)
if (-not (Test-Path -LiteralPath $NodePath -PathType Leaf)) {
  throw "Node runtime not found: $NodePath"
}

$rootFiles = @(
  "bridge-common.ps1",
  "CHANGELOG.md",
  "cloud-hanako-AGENTS.md",
  "config.example.json",
  "configure.ps1",
  "configuration-ui.ps1",
  "DEVELOPMENT_MANUAL.md",
  "install-background-service.ps1",
  "manager-core.ps1",
  "manager-command.ps1",
  "manager-ui.ps1",
  "open-approval.ps1",
  "open-manager.ps1",
  "OPERATION_MANUAL.md",
  "package.json",
  "README.md",
  "repair.ps1",
  "run-local-fs-hidden.vbs",
  "run-local-fs-service.ps1",
  "run-manager.vbs",
  "run-reverse-tunnel-hidden.vbs",
  "run-reverse-tunnel-service.ps1",
  "server.cjs",
  "start-local-fs-mcp.ps1",
  "start-reverse-tunnel.ps1",
  "status.ps1",
  "stop.ps1",
  "uninstall-background-service.ps1",
  "update-manifest.example.json",
  "update.ps1",
  "WINDOWS_INSTALLER_UPDATE_MANUAL.md"
)
foreach ($file in $rootFiles) {
  Copy-Item -LiteralPath (Join-Path $projectRoot $file) -Destination (Join-Path $payloadRoot $file) -Force
}
Copy-Item -LiteralPath (Join-Path $projectRoot "lib") -Destination (Join-Path $payloadRoot "lib") -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectRoot "cloud") -Destination (Join-Path $payloadRoot "cloud") -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectRoot "scripts") -Destination (Join-Path $payloadRoot "scripts") -Recurse -Force
& (Join-Path $projectRoot "build-manager-winui.ps1") -OutputDir $managerPublishRoot
if ($LASTEXITCODE -ne 0) { throw "WinUI manager build failed." }
Copy-Item -LiteralPath $managerPublishRoot -Destination (Join-Path $payloadRoot "manager") -Recurse -Force
New-Item -ItemType Directory -Force -Path (Join-Path $payloadRoot "runtime") | Out-Null
Copy-Item -LiteralPath $NodePath -Destination (Join-Path $payloadRoot "runtime\node.exe") -Force

foreach ($requiredManagerFile in @(
  "manager-command.ps1",
  "manager\HanakoBridgeManager.exe"
)) {
  if (-not (Test-Path -LiteralPath (Join-Path $payloadRoot $requiredManagerFile) -PathType Leaf)) {
    throw "Installer payload is missing $requiredManagerFile"
  }
}

& (Join-Path $payloadRoot "runtime\node.exe") --check (Join-Path $payloadRoot "server.cjs")
if ($LASTEXITCODE -ne 0) { throw "Bundled Node runtime failed to validate server.cjs." }

$releaseZip = Join-Path $outputDir "HanakoLocalBridge-$version-win-x64.zip"
Remove-Item -LiteralPath $releaseZip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $payloadRoot "*") -DestinationPath $releaseZip -CompressionLevel Optimal
$hash = (Get-FileHash -LiteralPath $releaseZip -Algorithm SHA256).Hash.ToLowerInvariant()
$size = (Get-Item -LiteralPath $releaseZip).Length
$publishedAtValue = if ([string]::IsNullOrWhiteSpace($PublishedAt)) {
  (Get-Date).ToUniversalTime().ToString("o")
} else {
  ([DateTimeOffset]::Parse($PublishedAt)).ToUniversalTime().ToString("o")
}
$manifest = [ordered]@{
  schemaVersion = 1
  channel = "stable"
  version = $version
  publishedAt = $publishedAtValue
  packageUrl = Split-Path -Leaf $releaseZip
  sha256 = $hash
  size = $size
  notes = "Hanako Local Bridge $version"
}
$manifestPath = Join-Path $outputDir "update-manifest.json"
[System.IO.File]::WriteAllText(
  $manifestPath,
  ($manifest | ConvertTo-Json -Depth 5) + [Environment]::NewLine,
  [System.Text.UTF8Encoding]::new($false)
)

$installerPath = Join-Path $outputDir "HanakoLocalBridge-Setup-$version.exe"
$tempInstallerPath = Join-Path $sfxRoot "HanakoLocalBridge-Setup-$version.exe"
try {
  New-Item -ItemType Directory -Force -Path $sfxRoot | Out-Null
  Copy-Item -LiteralPath $releaseZip -Destination (Join-Path $sfxRoot "payload.zip") -Force
  Copy-Item -LiteralPath (Join-Path $projectRoot "installer\bootstrap-install.ps1") `
    -Destination (Join-Path $sfxRoot "bootstrap-install.ps1") `
    -Force
  Copy-Item -LiteralPath (Join-Path $projectRoot "configuration-ui.ps1") `
    -Destination (Join-Path $sfxRoot "configuration-ui.ps1") `
    -Force

  $sedPath = Join-Path $sfxRoot "installer.sed"
  $sourceDirectory = $sfxRoot.TrimEnd("\") + "\"
  $sed = @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgramWindow=0
HideExtractAnimation=1
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=
DisplayLicense=
FinishMessage=
TargetName=$tempInstallerPath
FriendlyName=Hanako Local Bridge $version
AppLaunched=powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File bootstrap-install.ps1 -Gui
PostInstallCmd=<None>
AdminQuietInstCmd=powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File bootstrap-install.ps1 -NonInteractive
UserQuietInstCmd=powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File bootstrap-install.ps1 -NonInteractive
SourceFiles=SourceFiles
[Strings]
FILE0=payload.zip
FILE1=bootstrap-install.ps1
FILE2=configuration-ui.ps1
[SourceFiles]
SourceFiles0=$sourceDirectory
[SourceFiles0]
%FILE0%=
%FILE1%=
%FILE2%=
"@
  [System.IO.File]::WriteAllText($sedPath, $sed, [System.Text.Encoding]::ASCII)
  Remove-Item -LiteralPath $tempInstallerPath -Force -ErrorAction SilentlyContinue
  & (Join-Path $env:WINDIR "System32\iexpress.exe") /N /Q $sedPath
  $installerDeadline = (Get-Date).AddSeconds(300)
  $previousSize = -1
  $stableChecks = 0
  do {
    Start-Sleep -Milliseconds 500
    if (Test-Path -LiteralPath $tempInstallerPath -PathType Leaf) {
      $sizeNow = (Get-Item -LiteralPath $tempInstallerPath).Length
      if ($sizeNow -gt 0 -and $sizeNow -eq $previousSize) {
        $stableChecks++
      } else {
        $stableChecks = 0
      }
      $previousSize = $sizeNow
    }
  } while ((Get-Date) -lt $installerDeadline -and $stableChecks -lt 3)

  if (
    -not (Test-Path -LiteralPath $tempInstallerPath -PathType Leaf) -or
    (Get-Item $tempInstallerPath).Length -le 0
  ) {
    throw "IExpress failed to create the installer."
  }

  Remove-Item -LiteralPath $installerPath -Force -ErrorAction SilentlyContinue
  Copy-Item -LiteralPath $tempInstallerPath -Destination $installerPath -Force
} finally {
  if (
    (Test-Path -LiteralPath $sfxRoot) -and
    $sfxRoot.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)
  ) {
    Remove-Item -LiteralPath $sfxRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

Write-Host "Built release:"
Write-Host "  Installer: $installerPath"
Write-Host "  Package:   $releaseZip"
Write-Host "  Manifest:  $manifestPath"
Write-Host "  SHA256:    $hash"
