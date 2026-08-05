$ErrorActionPreference = "Stop"

function Assert-Manager {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
. (Join-Path $projectRoot "manager-core.ps1")

$managerXaml = Get-Content -LiteralPath (Join-Path $projectRoot "manager-winui\MainWindow.xaml") -Raw -Encoding UTF8
$managerSource = Get-Content -LiteralPath (Join-Path $projectRoot "manager-winui\MainWindow.xaml.cs") -Raw -Encoding UTF8
$managerServiceSource = Get-Content -LiteralPath (Join-Path $projectRoot "manager-winui\BridgeCommandService.cs") -Raw -Encoding UTF8
Assert-Manager ($managerXaml.Contains('x:Name="QueryDevicesButton"')) "Cloud query button is not addressable by busy state."
Assert-Manager ($managerXaml.Contains('x:Name="ClaimDeviceButton"')) "Cloud claim button is not addressable by busy state."
Assert-Manager ($managerSource.Contains("QueryDevicesButton.IsEnabled = !busy;")) "Busy state does not disable cloud query."
Assert-Manager ($managerSource.Contains("ClaimDeviceButton.IsEnabled = !busy;")) "Busy state does not disable cloud claim."
Assert-Manager ($managerSource.Contains("if (!IsPollingPausedPage(_selectedPageTag))")) "Initial load can restart polling on a paused page."
Assert-Manager ($managerSource.Contains('tag is "devices" or "settings"')) "Cloud devices page does not pause polling."
Assert-Manager `
  ($managerSource -match 'if \(_busy\)\s*\{\s*ShowInfo\([^;]+InfoBarSeverity\.Informational\);\s*return;\s*\}') `
  "Busy cloud actions can still fail silently."
Assert-Manager ($managerSource.Contains("await ShowPendingUpdateResultAsync();")) "Manager does not display detached update results."
Assert-Manager ($managerSource.Contains("_updateStatus.LatestVersion")) "Update launch does not pass the expected version."
Assert-Manager ($managerServiceSource.Contains('RunAsync<UpdateLaunchResult>')) "Update launch bypasses the verified manager command."
Assert-Manager ($managerServiceSource.Contains('"update-result"')) "Manager cannot consume the final update result."

Assert-Manager `
  ((ConvertTo-HanakoCloudWebBase "wss://your-server.example.com/local-bridge/connect") -eq "https://your-server.example.com") `
  "WebSocket URL conversion failed."
Assert-Manager `
  ((ConvertTo-HanakoCloudWebBase "https://example.test/desktop/?x=1") -eq "https://example.test") `
  "Web URL normalization failed."

$script:managerCloudCalls = @()
function Invoke-RestMethod {
  param(
    [string]$Uri,
    [string]$Method,
    [object]$WebSession,
    [string]$SessionVariable,
    [string]$ContentType,
    [string]$Body,
    [int]$TimeoutSec
  )

  $script:managerCloudCalls += $Uri
  if ($Uri -eq "https://example.test/api/web-auth/login") {
    if ($SessionVariable -ne "session") {
      throw "Cloud login did not request a reusable web session."
    }
    Set-Variable `
      -Name $SessionVariable `
      -Value ([pscustomobject]@{ marker = "manager-cloud-session" }) `
      -Scope 1
    return [pscustomobject]@{ ok = $true }
  }
  if ($Uri -eq "https://example.test/api/local-bridge/devices") {
    if ($WebSession.marker -ne "manager-cloud-session") {
      throw "Cloud device query did not reuse the login session."
    }
    return [pscustomobject]@{
      devices = @(
        [pscustomobject]@{
          id = "manager-cloud-device"
          name = "Manager Cloud Device"
          version = "test"
          status = "online"
          lastSeenAt = "2026-07-18T00:00:00Z"
        }
      )
    }
  }
  throw "Unexpected cloud test request: $Uri"
}

try {
  $cloudResult = Invoke-HanakoBridgeCloudQuery `
    -BaseUrl "https://example.test" `
    -AccessKey "manager-cloud-test-key"
  Assert-Manager ($cloudResult.devices.Count -eq 1) "Cloud device query returned the wrong device count."
  Assert-Manager ($cloudResult.devices[0].id -eq "manager-cloud-device") "Cloud device query returned the wrong device."
  Assert-Manager ($script:managerCloudCalls.Count -eq 2) "Cloud device query made an unexpected number of requests."
} finally {
  Remove-Item Function:\Invoke-RestMethod -Force -ErrorAction SilentlyContinue
}

$processTestRoot = "C:\Users\Test\HanakoLocalBridge"
Assert-Manager `
  (Test-BridgeManagedProcessCommandLine `
    -CommandLine "node.exe C:\Users\Test\HanakoLocalBridge\server.cjs" `
    -InstallRoot $processTestRoot) `
  "Bridge server process detection failed."
Assert-Manager `
  (-not (Test-BridgeManagedProcessCommandLine `
    -CommandLine "powershell.exe -Command Get-Content C:\Users\Test\HanakoLocalBridge\manager-command.ps1" `
    -InstallRoot $processTestRoot)) `
  "Unrelated process referencing the install directory was misidentified."

$testId = [Guid]::NewGuid().ToString("N").Substring(0, 10)
$testRoot = Join-Path $env:TEMP "HanakoBridgeManagerCore-$testId"
$dataDir = Join-Path $testRoot "data"
$logDir = Join-Path $testRoot "logs"
$mcpPort = 43000 + (Get-Random -Minimum 0 -Maximum 1000)
$statusPort = $mcpPort + 1
$secret = "hana_dev_MANAGER_CORE_TEST_SECRET_$testId"

try {
  New-Item -ItemType Directory -Force -Path $testRoot, $dataDir, $logDir | Out-Null
  Copy-Item -LiteralPath (Join-Path $projectRoot "package.json") -Destination $testRoot
  Copy-Item -LiteralPath (Join-Path $projectRoot "scripts") -Destination $testRoot -Recurse
  Copy-Item -LiteralPath (Join-Path $projectRoot "lib") -Destination $testRoot -Recurse

  $config = [ordered]@{
    schemaVersion = 1
    device = [ordered]@{
      id = "manager-test-$testId"
      name = "Manager Test"
    }
    filesystem = [ordered]@{
      host = "127.0.0.1"
      port = $mcpPort
      approvalPort = $statusPort
      trustMode = "full"
      allowChatAuthorization = $false
      chatGrantMinutes = 120
      roots = @(
        [ordered]@{
          name = "Test"
          path = $testRoot
          mode = "read_write"
        }
      )
    }
    storage = [ordered]@{
      dataDir = $dataDir
      logDir = $logDir
    }
    cloud = [ordered]@{
      enabled = $true
      url = "ws://127.0.0.1:9/local-bridge/connect"
      reconnectMinSeconds = 3
      reconnectMaxSeconds = 60
      heartbeatSeconds = 25
    }
    tunnel = [ordered]@{
      enabled = $false
      server = "127.0.0.1"
      user = "test"
      localHost = "127.0.0.1"
      localPort = $mcpPort
      remoteHost = "127.0.0.1"
      remotePort = 47000
      identityFile = ""
    }
    service = [ordered]@{
      taskPrefix = "Hanako Manager Core Test $testId"
      restartDelaySeconds = 3
      tunnelRetryMinSeconds = 5
      tunnelRetryMaxSeconds = 60
      tunnelHealthSeconds = 30
    }
    update = [ordered]@{
      manifest = ""
      channel = "stable"
    }
  }
  Write-BridgeJson -Value $config -Path (Join-Path $testRoot "config.json")
  Write-BridgeJson -Value ([ordered]@{
    schemaVersion = 1
    deviceId = "manager-test-$testId"
    publicKeyFingerprint = "manager-test-fingerprint"
    claimToken = ""
    credential = $secret
    updatedAt = (Get-Date).ToUniversalTime().ToString("o")
  }) -Path (Join-Path $dataDir "cloud-identity.json")

  $snapshot = Get-HanakoBridgeManagerSnapshot `
    -InstallRoot $testRoot `
    -ConfigPath (Join-Path $testRoot "config.json")

  Assert-Manager ($snapshot.device.id -eq "manager-test-$testId") "Snapshot device ID is incorrect."
  Assert-Manager (-not $snapshot.local.mcpHealthy) "Unexpected live MCP service in isolated test."
  Assert-Manager ($snapshot.cloud.status -eq "offline") "Offline cloud state was not detected."
  Assert-Manager ($snapshot.tasks.mcpState -eq "Missing") "Missing scheduled task was not detected."
  Assert-Manager ($snapshot.identity.credentialPresent -eq $true) "Credential presence was not detected."
  Assert-Manager ($snapshot.identity.PSObject.Properties.Name -notcontains "credential") "Snapshot exposed a credential field."
  Assert-Manager ($snapshot.processes -is [array]) "Snapshot processes must always be an array."
  Assert-Manager (
    @($snapshot.checks | Where-Object { $_.code -eq "mcp_task" -and $_.status -eq "error" }).Count -eq 1
  ) "Scheduled task diagnostic is missing."
  Assert-Manager (
    @($snapshot.checks | Where-Object { $_.code -eq "cloud" -and $_.status -eq "error" }).Count -eq 1
  ) "Cloud offline diagnostic is missing."

  $serialized = $snapshot | ConvertTo-Json -Depth 10
  Assert-Manager (-not $serialized.Contains($secret)) "Serialized snapshot leaked the test credential."
  Assert-Manager (-not $serialized.Contains("privateKey")) "Serialized snapshot exposed private key material."

  $utf16Log = Join-Path $logDir "utf16-no-bom.log"
  [System.IO.File]::WriteAllBytes(
    $utf16Log,
    [System.Text.Encoding]::Unicode.GetBytes("first`r`nsecond`r`nthird")
  )
  $logTail = Get-HanakoBridgeLogTail -Path $utf16Log -Lines 2
  Assert-Manager ($logTail -eq "second`r`nthird") "UTF-16LE log detection failed."
  Assert-Manager (-not $logTail.Contains([char]0)) "Log tail contains NUL characters."

  $mixedLog = Join-Path $logDir "mixed-encoding.log"
  $utf8Part = [System.Text.UTF8Encoding]::new($false).GetBytes("utf8-first`n")
  $utf16Part = [System.Text.Encoding]::Unicode.GetBytes("utf16-second`r`n")
  $mixedBytes = [byte[]]::new($utf8Part.Length + $utf16Part.Length)
  [System.Array]::Copy($utf8Part, 0, $mixedBytes, 0, $utf8Part.Length)
  [System.Array]::Copy($utf16Part, 0, $mixedBytes, $utf8Part.Length, $utf16Part.Length)
  [System.IO.File]::WriteAllBytes($mixedLog, $mixedBytes)
  $mixedTail = Get-HanakoBridgeLogTail -Path $mixedLog -Lines 2
  Assert-Manager ($mixedTail -eq "utf8-first`r`nutf16-second") "Mixed log encoding detection failed."

  Write-Host "manager core tests passed"
} finally {
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
