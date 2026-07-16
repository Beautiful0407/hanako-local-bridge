param(
  [string]$OutputDir = "",
  [string]$DotNetPath = "",
  [switch]$SkipRestore
)

$ErrorActionPreference = "Stop"

function Resolve-DotNetExecutable {
  param([string]$RequestedPath)

  $candidates = @()
  if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
    $candidates += $RequestedPath
  }
  $candidates += (Join-Path $env:USERPROFILE ".dotnet10\dotnet.exe")
  $command = Get-Command dotnet.exe -ErrorAction SilentlyContinue
  if ($command) {
    $candidates += $command.Source
  }
  $candidates += (Join-Path $env:USERPROFILE ".dotnet\dotnet.exe")

  foreach ($candidate in $candidates | Where-Object { $_ } | Select-Object -Unique) {
    $resolved = [System.IO.Path]::GetFullPath($candidate)
    if (Test-Path -LiteralPath $resolved -PathType Leaf) {
      return $resolved
    }
  }
  throw ".NET SDK 10 was not found. Pass -DotNetPath <path-to-dotnet.exe>."
}

function Assert-BuildPath {
  param(
    [Parameter(Mandatory = $true)][string]$ProjectRoot,
    [Parameter(Mandatory = $true)][string]$Path
  )

  $resolvedRoot = [System.IO.Path]::GetFullPath($ProjectRoot).TrimEnd("\") + "\"
  $resolvedPath = [System.IO.Path]::GetFullPath($Path)
  if (-not $resolvedPath.StartsWith($resolvedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Manager output must stay inside the project directory: $resolvedPath"
  }
  return $resolvedPath
}

$projectRoot = [System.IO.Path]::GetFullPath($PSScriptRoot)
$projectFile = Join-Path $projectRoot "manager-winui\HanakoBridgeManager.csproj"
if (-not (Test-Path -LiteralPath $projectFile -PathType Leaf)) {
  throw "WinUI manager project is missing: $projectFile"
}

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $OutputDir = Join-Path $projectRoot "build\manager-winui\win-x64"
}
$outputDir = Assert-BuildPath -ProjectRoot $projectRoot -Path $OutputDir
$dotnet = Resolve-DotNetExecutable -RequestedPath $DotNetPath
$sdkVersion = (& $dotnet --version).Trim()
if ($LASTEXITCODE -ne 0 -or $sdkVersion -notmatch "^10\.") {
  throw ".NET SDK 10 is required; detected '$sdkVersion'."
}

$package = Get-Content -LiteralPath (Join-Path $projectRoot "package.json") -Raw | ConvertFrom-Json
$version = [string]$package.version

if (Test-Path -LiteralPath $outputDir) {
  Remove-Item -LiteralPath $outputDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null

if (-not $SkipRestore) {
  & $dotnet restore $projectFile --runtime win-x64
  if ($LASTEXITCODE -ne 0) { throw "WinUI manager restore failed." }
}

& $dotnet publish $projectFile `
  --configuration Release `
  --runtime win-x64 `
  --self-contained true `
  --output $outputDir `
  --no-restore `
  "-p:Version=$version" `
  "-p:WindowsAppSDKSelfContained=true" `
  "-p:PublishReadyToRun=false"
if ($LASTEXITCODE -ne 0) { throw "WinUI manager publish failed." }

$requiredFiles = @(
  "HanakoBridgeManager.exe",
  "App.xbf",
  "MainWindow.xbf",
  "HanakoBridgeManager.pri"
)
foreach ($requiredFile in $requiredFiles) {
  if (-not (Test-Path -LiteralPath (Join-Path $outputDir $requiredFile) -PathType Leaf)) {
    throw "Published WinUI manager is missing: $requiredFile"
  }
}
$managerExe = Join-Path $outputDir "HanakoBridgeManager.exe"

$smoke = Start-Process `
  -FilePath $managerExe `
  -ArgumentList @("--smoke-test", "--install-root", "`"$projectRoot`"") `
  -Wait `
  -PassThru
if ($smoke.ExitCode -ne 0) {
  throw "Published WinUI manager smoke test failed with code $($smoke.ExitCode)."
}

Write-Host "Published WinUI manager:"
Write-Host "  SDK:     $sdkVersion"
Write-Host "  Version: $version"
Write-Host "  Output:  $outputDir"
