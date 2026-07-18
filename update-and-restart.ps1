param(
  [string]$InstallRoot = $PSScriptRoot,
  [string]$Manifest = "",
  [string]$AttemptId = "",
  [string]$ExpectedVersion = "",
  [string]$StatePath = ""
)

$ErrorActionPreference = "Stop"
$root = [System.IO.Path]::GetFullPath($InstallRoot)
$updateScript = Join-Path $root "update.ps1"
$managerLauncher = Join-Path $root "open-manager.ps1"
$logDirectory = Join-Path $root "logs"
$logPath = Join-Path $logDirectory "update.log"
if ([string]::IsNullOrWhiteSpace($AttemptId)) {
  $AttemptId = [Guid]::NewGuid().ToString("N")
}
if ([string]::IsNullOrWhiteSpace($StatePath)) {
  $StatePath = Join-Path $root "data\update-state.json"
}
$stateFile = [System.IO.Path]::GetFullPath($StatePath)
$startedAt = (Get-Date).ToUniversalTime().ToString("o")
$exitCode = 1
$installedVersion = ""

function Write-UpdateState {
  param(
    [Parameter(Mandatory = $true)][string]$Status,
    [string]$Message = "",
    [string]$FinishedAt = ""
  )

  $state = [ordered]@{
    schemaVersion = 1
    attemptId = $AttemptId
    status = $Status
    expectedVersion = $ExpectedVersion
    installedVersion = $installedVersion
    message = $Message
    logPath = $logPath
    startedAt = $startedAt
    finishedAt = $FinishedAt
    exitCode = $exitCode
  }
  $parent = Split-Path -Parent $stateFile
  New-Item -ItemType Directory -Force -Path $parent | Out-Null
  $temp = "$stateFile.$PID.tmp"
  try {
    [System.IO.File]::WriteAllText(
      $temp,
      ($state | ConvertTo-Json -Depth 6) + [Environment]::NewLine,
      [System.Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $temp -Destination $stateFile -Force
  } finally {
    Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue
  }
}

try {
  New-Item -ItemType Directory -Force -Path $logDirectory | Out-Null
  [System.IO.File]::AppendAllText(
    $logPath,
    "$([Environment]::NewLine)=== Update $AttemptId started $startedAt; expected $ExpectedVersion ===$([Environment]::NewLine)",
    [System.Text.UTF8Encoding]::new($false)
  )
  Write-UpdateState -Status "running" -Message "Update process accepted the request."

  $arguments = @(
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    $updateScript,
    "-TargetRoot",
    $root
  )
  if (-not [string]::IsNullOrWhiteSpace($Manifest)) {
    $arguments += @("-Manifest", $Manifest)
  }

  $previousErrorActionPreference = $ErrorActionPreference
  try {
    $ErrorActionPreference = "Continue"
    & powershell.exe @arguments *>> $logPath
    $exitCode = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }
  if ($exitCode -ne 0) {
    throw "Update process exited with code $exitCode."
  }

  $packagePath = Join-Path $root "package.json"
  if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
    throw "Installed package metadata is missing after update."
  }
  $installedVersion = [string](Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json).version
  if (
    -not [string]::IsNullOrWhiteSpace($ExpectedVersion) -and
    [version]$installedVersion -lt [version]$ExpectedVersion
  ) {
    throw "Expected version $ExpectedVersion, but installed version is $installedVersion."
  }

  $exitCode = 0
  Write-UpdateState `
    -Status "succeeded" `
    -Message "Update completed successfully." `
    -FinishedAt ((Get-Date).ToUniversalTime().ToString("o"))
} catch {
  if ($exitCode -eq 0) { $exitCode = 1 }
  Write-UpdateState `
    -Status "failed" `
    -Message $_.Exception.Message `
    -FinishedAt ((Get-Date).ToUniversalTime().ToString("o"))
} finally {
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
}

exit $exitCode
