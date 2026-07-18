param(
  [ValidateSet("snapshot", "action", "cloud-query", "logs", "log-tail", "update-check", "update-launch", "update-result")]
  [string]$Operation = "snapshot",
  [string]$InstallRoot = $PSScriptRoot,
  [string]$ConfigPath = ""
)

$ErrorActionPreference = "Stop"
$utf8 = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8

. (Join-Path $InstallRoot "manager-core.ps1")

try {
  $result = & {
    switch ($Operation) {
      "snapshot" {
        Get-HanakoBridgeManagerSnapshot -InstallRoot $InstallRoot -ConfigPath $ConfigPath
      }
      "action" {
        $action = [string]$env:HANA_MANAGER_ACTION
        if ($action -notin @("start", "stop", "restart", "repair")) {
          throw "Invalid manager action."
        }
        Invoke-HanakoBridgeManagerAction `
          -Action $action `
          -InstallRoot $InstallRoot `
          -ConfigPath $ConfigPath
      }
      "cloud-query" {
        $arguments = @{
          BaseUrl = [string]$env:HANA_MANAGER_BASE_URL
          AccessKey = [string]$env:HANA_MANAGER_ACCESS_KEY
          InstallRoot = $InstallRoot
          ConfigPath = $ConfigPath
        }
        if ([string]$env:HANA_MANAGER_CLAIM -eq "1") {
          $arguments.ClaimCurrentDevice = $true
        }
        Invoke-HanakoBridgeCloudQuery @arguments
      }
      "logs" {
        [pscustomobject]@{
          logs = @(Get-HanakoBridgeLogFiles -InstallRoot $InstallRoot)
        }
      }
      "log-tail" {
        [pscustomobject]@{
          content = Get-HanakoBridgeLogTail -Path ([string]$env:HANA_MANAGER_LOG_PATH)
        }
      }
      "update-check" {
        Get-HanakoBridgeUpdateStatus `
          -InstallRoot $InstallRoot `
          -ConfigPath $ConfigPath `
          -Manifest ([string]$env:HANA_MANAGER_UPDATE_MANIFEST)
      }
      "update-launch" {
        Start-HanakoBridgeUpdate `
          -InstallRoot $InstallRoot `
          -Manifest ([string]$env:HANA_MANAGER_UPDATE_MANIFEST) `
          -ExpectedVersion ([string]$env:HANA_MANAGER_UPDATE_EXPECTED_VERSION)
      }
      "update-result" {
        $arguments = @{
          InstallRoot = $InstallRoot
        }
        if ([string]$env:HANA_MANAGER_UPDATE_CONSUME -eq "1") {
          $arguments.Consume = $true
        }
        Get-HanakoBridgeUpdateResult @arguments
      }
    }
  } 3>$null 4>$null 5>$null 6>$null

  [Console]::Out.WriteLine(($result | ConvertTo-Json -Depth 12 -Compress))
  exit 0
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 1
} finally {
  Remove-Item Env:HANA_MANAGER_ACCESS_KEY -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_MANIFEST -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_EXPECTED_VERSION -ErrorAction SilentlyContinue
  Remove-Item Env:HANA_MANAGER_UPDATE_CONSUME -ErrorAction SilentlyContinue
}
