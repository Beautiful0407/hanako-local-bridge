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
      $commandLine = [string]$_.CommandLine
      $inInstall = $commandLine.IndexOf($root, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
      $isTunnel = $commandLine.IndexOf($forwardMarker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
      return $inInstall -or $isTunnel
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
