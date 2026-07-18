function Get-BridgeInstallRoot {
  param([string]$InstallRoot = $PSScriptRoot)

  return [System.IO.Path]::GetFullPath($InstallRoot)
}

function Get-BridgeNodePath {
  param([string]$InstallRoot = $PSScriptRoot)

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $candidates = @(
    (Join-Path $root "runtime\node.exe"),
    (Join-Path $env:ProgramFiles "nodejs\node.exe")
  )

  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      return [System.IO.Path]::GetFullPath($candidate)
    }
  }

  $command = Get-Command node.exe -ErrorAction SilentlyContinue
  if ($command) { return $command.Source }
  throw "Node.js runtime was not found. Run repair.ps1 or reinstall Hanako Local Bridge."
}

function Get-BridgeRuntime {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ConfigPath = ""
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $node = Get-BridgeNodePath -InstallRoot $root
  $helper = Join-Path $root "scripts\runtime-config-cli.cjs"
  if (-not (Test-Path -LiteralPath $helper -PathType Leaf)) {
    throw "Runtime config helper is missing: $helper"
  }

  $arguments = @($helper, "--install-dir", $root)
  if (-not [string]::IsNullOrWhiteSpace($ConfigPath)) {
    $arguments += @("--config", [System.IO.Path]::GetFullPath($ConfigPath))
  }
  $json = & $node @arguments
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace(($json -join ""))) {
    throw "Failed to load bridge configuration."
  }
  return (($json -join [Environment]::NewLine) | ConvertFrom-Json)
}

function Write-BridgeJson {
  param(
    [Parameter(Mandatory = $true)]$Value,
    [Parameter(Mandatory = $true)][string]$Path
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path)
  $parent = Split-Path -Parent $fullPath
  New-Item -ItemType Directory -Force -Path $parent | Out-Null
  $temp = "$fullPath.$PID.tmp"
  try {
    $json = $Value | ConvertTo-Json -Depth 20
    [System.IO.File]::WriteAllText(
      $temp,
      $json + [Environment]::NewLine,
      [System.Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $temp -Destination $fullPath -Force
  } finally {
    Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue
  }
}

function Remove-HanakoBridgeStalePayloadFiles {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ManifestPath = ""
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $rootPrefix = $root.TrimEnd("\") + "\"
  if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
    $ManifestPath = Join-Path $root "payload-manifest.json"
  }
  $manifestFile = [System.IO.Path]::GetFullPath($ManifestPath)
  if (-not (Test-Path -LiteralPath $manifestFile -PathType Leaf)) {
    throw "Payload manifest is missing: $manifestFile"
  }

  $manifest = Get-Content -LiteralPath $manifestFile -Raw | ConvertFrom-Json
  if ([int]$manifest.schemaVersion -ne 1) {
    throw "Unsupported payload manifest schema: $($manifest.schemaVersion)"
  }

  $expectedFiles = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
  )
  foreach ($entry in @($manifest.files)) {
    $relative = ([string]$entry).Replace("\", "/").TrimStart("/")
    if (
      [string]::IsNullOrWhiteSpace($relative) -or
      [System.IO.Path]::IsPathRooted($relative) -or
      @($relative.Split("/")) -contains ".."
    ) {
      throw "Unsafe payload manifest path: $entry"
    }
    [void]$expectedFiles.Add($relative)
  }

  $removedFiles = 0
  $removedDirectories = 0
  foreach ($directoryName in @($manifest.managedDirectories)) {
    $managedName = ([string]$directoryName).Trim()
    if (
      [string]::IsNullOrWhiteSpace($managedName) -or
      $managedName -match "[\\/]" -or
      $managedName -in @(".", "..")
    ) {
      throw "Unsafe managed payload directory: $directoryName"
    }

    $managedRoot = [System.IO.Path]::GetFullPath((Join-Path $root $managedName))
    if (-not ($managedRoot + "\").StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Managed payload directory escapes the install root: $managedRoot"
    }
    if (-not (Test-Path -LiteralPath $managedRoot -PathType Container)) {
      continue
    }

    foreach ($file in Get-ChildItem -LiteralPath $managedRoot -Recurse -File -Force) {
      $relative = $file.FullName.Substring($rootPrefix.Length).Replace("\", "/")
      if (-not $expectedFiles.Contains($relative)) {
        Remove-Item -LiteralPath $file.FullName -Force
        $removedFiles++
      }
    }

    $directories = @(
      Get-ChildItem -LiteralPath $managedRoot -Recurse -Directory -Force |
        Sort-Object @{ Expression = { $_.FullName.Length }; Descending = $true }
    )
    foreach ($directory in $directories) {
      if (-not (Get-ChildItem -LiteralPath $directory.FullName -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $directory.FullName -Force
        $removedDirectories++
      }
    }
  }

  return [pscustomobject]@{
    manifestPath = $manifestFile
    removedFiles = $removedFiles
    removedDirectories = $removedDirectories
  }
}

function Invoke-HanakoBridgePayloadCleanup {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [switch]$Force
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $markerPath = Join-Path $root "payload-cleanup.pending"
  if (-not $Force -and -not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    return [pscustomobject]@{
      skipped = $true
      removedFiles = 0
      removedDirectories = 0
    }
  }

  $result = Remove-HanakoBridgeStalePayloadFiles -InstallRoot $root
  Remove-Item -LiteralPath $markerPath -Force -ErrorAction SilentlyContinue
  return [pscustomobject]@{
    skipped = $false
    removedFiles = $result.removedFiles
    removedDirectories = $result.removedDirectories
  }
}

function Install-HanakoBridgeShellIntegration {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$ProgramsRoot = "",
    [string]$DesktopRoot = "",
    [switch]$SkipRegistry
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $package = Get-Content -LiteralPath (Join-Path $root "package.json") -Raw | ConvertFrom-Json
  if ([string]::IsNullOrWhiteSpace($ProgramsRoot)) {
    $ProgramsRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
  }
  if ([string]::IsNullOrWhiteSpace($DesktopRoot)) {
    $DesktopRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
  }

  if (-not $SkipRegistry) {
    $uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge"
    New-Item -Path $uninstallKey -Force | Out-Null
    $uninstallCommand = "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File `"$root\uninstall-background-service.ps1`" -RemoveInstall -KeepData"
    New-ItemProperty -Path $uninstallKey -Name DisplayName -Value "Hanako Local Bridge" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name DisplayVersion -Value ([string]$package.version) -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name Publisher -Value "Hanako Local Bridge" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name InstallLocation -Value $root -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name UninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name QuietUninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallKey -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null
  }

  $shortcutTarget = Join-Path $env:WINDIR "System32\wscript.exe"
  $shortcutArguments = "//B //NoLogo `"$root\run-manager.vbs`""
  $managerIcon = Join-Path $root "manager\HanakoBridgeManager.exe"
  $startMenuDir = Join-Path $ProgramsRoot "Hanako Local Bridge"
  $desktopShortcut = Join-Path $DesktopRoot "Hanako Local Bridge Manager.lnk"
  New-Item -ItemType Directory -Force -Path $startMenuDir, $DesktopRoot | Out-Null

  foreach ($shortcutPath in @(
    (Join-Path $startMenuDir "Hanako Local Bridge Manager.lnk"),
    $desktopShortcut
  )) {
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $shortcutTarget
    $shortcut.Arguments = $shortcutArguments
    $shortcut.WorkingDirectory = $root
    $shortcut.Description = "Manage, diagnose, repair, and claim Hanako Local Bridge devices"
    if (Test-Path -LiteralPath $managerIcon -PathType Leaf) {
      $shortcut.IconLocation = "$managerIcon,0"
    }
    $shortcut.Save()
  }

  [pscustomobject]@{
    startMenu = Join-Path $startMenuDir "Hanako Local Bridge Manager.lnk"
    desktop = $desktopShortcut
  }
}

function Test-HanakoBridgeShellIntegrationEligible {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    [string]$DefaultInstallRoot = ""
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  if ([string]$env:HANA_BRIDGE_SKIP_UNINSTALL_REGISTRATION -match "^(1|true|yes|on)$") {
    return $false
  }

  $registration = Get-ItemProperty `
    -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge" `
    -ErrorAction SilentlyContinue
  if ($registration -and -not [string]::IsNullOrWhiteSpace([string]$registration.InstallLocation)) {
    $registeredRoot = [System.IO.Path]::GetFullPath([string]$registration.InstallLocation).TrimEnd("\")
    if ($registeredRoot -eq $root.TrimEnd("\")) {
      return $true
    }
  }

  if ([string]::IsNullOrWhiteSpace($DefaultInstallRoot)) {
    $DefaultInstallRoot = Join-Path $env:LOCALAPPDATA "HanakoLocalBridge"
  }
  return [System.IO.Path]::GetFullPath($DefaultInstallRoot).TrimEnd("\") -eq $root.TrimEnd("\")
}

function Remove-HanakoBridgeShellIntegration {
  param(
    [string]$ProgramsRoot = "",
    [string]$DesktopRoot = "",
    [switch]$SkipRegistry
  )

  if ([string]::IsNullOrWhiteSpace($ProgramsRoot)) {
    $ProgramsRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
  }
  if ([string]::IsNullOrWhiteSpace($DesktopRoot)) {
    $DesktopRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
  }
  if (-not $SkipRegistry) {
    Remove-Item `
      -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanakoLocalBridge" `
      -Recurse `
      -Force `
      -ErrorAction SilentlyContinue
  }
  Remove-Item `
    -LiteralPath (Join-Path $ProgramsRoot "Hanako Local Bridge") `
    -Recurse `
    -Force `
    -ErrorAction SilentlyContinue
  Remove-Item `
    -LiteralPath (Join-Path $DesktopRoot "Hanako Local Bridge Manager.lnk") `
    -Force `
    -ErrorAction SilentlyContinue
}

function Get-BridgeTaskNames {
  param([Parameter(Mandatory = $true)]$Runtime)

  $prefix = [string]$Runtime.config.service.taskPrefix
  if ([string]::IsNullOrWhiteSpace($prefix)) { $prefix = "Hanako Local FS" }
  return [pscustomobject]@{
    Mcp = "$prefix MCP"
    Tunnel = "$prefix Tunnel"
  }
}

function Get-BridgeMutexName {
  param(
    [Parameter(Mandatory = $true)][string]$InstallRoot,
    [Parameter(Mandatory = $true)][string]$Role
  )

  $normalized = [System.IO.Path]::GetFullPath($InstallRoot).ToLowerInvariant()
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($normalized)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $hash = -join ($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString("x2") })
  } finally {
    $sha.Dispose()
  }
  return "Local\HanakoLocalBridge-$Role-$($hash.Substring(0, 20))"
}

function Rotate-BridgeLogFile {
  param(
    [string]$Path,
    [long]$MaxBytes = 10MB,
    [int]$Backups = 5
  )

  if (-not (Test-Path -LiteralPath $Path)) { return }
  $item = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
  if (-not $item -or $item.Length -lt $MaxBytes) { return }

  Remove-Item -LiteralPath "${Path}.${Backups}" -Force -ErrorAction SilentlyContinue
  for ($index = $Backups - 1; $index -ge 1; $index--) {
    $source = "${Path}.${index}"
    $destination = "${Path}.$($index + 1)"
    if (Test-Path -LiteralPath $source) {
      Move-Item -LiteralPath $source -Destination $destination -Force
    }
  }
  Move-Item -LiteralPath $Path -Destination "${Path}.1" -Force
}

function ConvertTo-BridgeNativeArgument {
  param([AllowEmptyString()][string]$Value)

  if ($null -eq $Value -or $Value.Length -eq 0) { return '""' }
  if ($Value -notmatch '[\s"]') { return $Value }

  $builder = [System.Text.StringBuilder]::new()
  [void]$builder.Append('"')
  $backslashes = 0

  foreach ($character in $Value.ToCharArray()) {
    if ([int]$character -eq 92) {
      $backslashes += 1
      continue
    }

    if ([int]$character -eq 34) {
      if ($backslashes -gt 0) {
        [void]$builder.Append([string]::new([char]92, ($backslashes * 2) + 1))
      } else {
        [void]$builder.Append('\')
      }
      [void]$builder.Append('"')
      $backslashes = 0
      continue
    }

    if ($backslashes -gt 0) {
      [void]$builder.Append([string]::new([char]92, $backslashes))
      $backslashes = 0
    }
    [void]$builder.Append($character)
  }

  if ($backslashes -gt 0) {
    [void]$builder.Append([string]::new([char]92, $backslashes * 2))
  }
  [void]$builder.Append('"')
  return $builder.ToString()
}

function Invoke-BridgeProcessWithTimeout {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$ArgumentList = @(),
    [int]$TimeoutSeconds = 20,
    [string]$StdOutPath = "",
    [string]$StdErrPath = ""
  )

  $timeoutMilliseconds = [Math]::Max(1, $TimeoutSeconds) * 1000
  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $FilePath
  $startInfo.Arguments = (($ArgumentList | ForEach-Object {
    ConvertTo-BridgeNativeArgument -Value ([string]$_)
  }) -join " ")
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true

  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  $startedAt = Get-Date
  $started = $false
  $timedOut = $false
  $exitCode = -1
  $stdout = ""
  $stderr = ""

  try {
    $started = $process.Start()
    if (-not $started) {
      throw "Failed to start process: $FilePath"
    }

    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($timeoutMilliseconds)) {
      $timedOut = $true
      try { Stop-Process -Id $process.Id -Force -ErrorAction Stop } catch {}
      try { $process.WaitForExit(5000) | Out-Null } catch {}
    } else {
      $process.WaitForExit()
      $exitCode = $process.ExitCode
    }

    if ($stdoutTask.Wait(5000)) { $stdout = [string]$stdoutTask.Result }
    if ($stderrTask.Wait(5000)) { $stderr = [string]$stderrTask.Result }
  } catch {
    $stderr = if ($stderr) { "$stderr`r`n$($_.Exception.Message)" } else { $_.Exception.Message }
  } finally {
    $process.Dispose()
  }

  if (-not [string]::IsNullOrWhiteSpace($StdOutPath) -and -not [string]::IsNullOrEmpty($stdout)) {
    Add-Content -LiteralPath $StdOutPath -Value $stdout.TrimEnd()
  }
  if (-not [string]::IsNullOrWhiteSpace($StdErrPath) -and -not [string]::IsNullOrEmpty($stderr)) {
    Add-Content -LiteralPath $StdErrPath -Value $stderr.TrimEnd()
  }

  return [pscustomobject]@{
    Started = $started
    TimedOut = $timedOut
    ExitCode = $exitCode
    StdOut = $stdout
    StdErr = $stderr
    DurationSeconds = [Math]::Max(0, ((Get-Date) - $startedAt).TotalSeconds)
  }
}

function Test-BridgeManagedProcessCommandLine {
  param(
    [string]$CommandLine,
    [Parameter(Mandatory = $true)][string]$InstallRoot,
    [string]$ForwardMarker = ""
  )

  if ([string]::IsNullOrWhiteSpace($CommandLine)) { return $false }
  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  $inInstall = $CommandLine.IndexOf($root, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
  $isBridgeProcess = $CommandLine -match (
    "(?i)(run-local-fs-hidden\.vbs|run-local-fs-service\.ps1|server\.cjs|" +
    "run-reverse-tunnel-hidden\.vbs|run-reverse-tunnel-service\.ps1)"
  )
  $isTunnel =
    -not [string]::IsNullOrWhiteSpace($ForwardMarker) -and
    $CommandLine.IndexOf($ForwardMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
  return ($inInstall -and $isBridgeProcess) -or $isTunnel
}

function Stop-BridgeProcesses {
  param(
    [string]$InstallRoot = $PSScriptRoot,
    $Runtime = $null
  )

  $root = Get-BridgeInstallRoot -InstallRoot $InstallRoot
  if (-not $Runtime) { $Runtime = Get-BridgeRuntime -InstallRoot $root }
  $remotePort = [int]$Runtime.config.tunnel.remotePort
  $localPort = [int]$Runtime.config.tunnel.localPort
  $forwardMarker = "127.0.0.1:${remotePort}:127.0.0.1:${localPort}"
  $currentPid = $PID
  $allProcesses = @(Get-CimInstance Win32_Process)
  $processById = @{}
  foreach ($process in $allProcesses) {
    $processById[[int]$process.ProcessId] = $process
  }
  $protectedPids = [System.Collections.Generic.HashSet[int]]::new()
  $cursor = $currentPid
  while ($cursor -gt 0 -and $protectedPids.Add([int]$cursor)) {
    $current = $processById[$cursor]
    if (-not $current) { break }
    $cursor = [int]$current.ParentProcessId
  }

  $allProcesses |
    Where-Object {
      if ($protectedPids.Contains([int]$_.ProcessId) -or [string]::IsNullOrWhiteSpace($_.CommandLine)) {
        return $false
      }
      if ([string]$_.Name -ieq "HanakoBridgeManager.exe") {
        return $false
      }
      return Test-BridgeManagedProcessCommandLine `
        -CommandLine ([string]$_.CommandLine) `
        -InstallRoot $root `
        -ForwardMarker $forwardMarker
    } |
    Sort-Object ProcessId -Descending |
    ForEach-Object {
      try {
        Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop
        Write-Host "Stopped $($_.Name) pid=$($_.ProcessId)"
      } catch {
        Write-Host "Failed to stop pid=$($_.ProcessId): $($_.Exception.Message)"
      }
    }
}
