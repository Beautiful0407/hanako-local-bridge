param(
  [string]$InstallRoot = $PSScriptRoot,
  [string]$ConfigPath = "",
  [string]$DeviceId = "",
  [string]$DeviceName = "",
  [string]$RootPath = "",
  [string]$VpsHost = "",
  [string]$CloudUrl = "",
  [string]$SshUser = "",
  [string]$IdentityFile = "",
  [int]$McpPort = 0,
  [int]$ApprovalPort = 0,
  [int]$RemotePort = 0,
  [string]$TaskPrefix = "",
  [switch]$DisableTunnel,
  [switch]$CollectOnly
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$installRoot = [System.IO.Path]::GetFullPath($InstallRoot)
if ([string]::IsNullOrWhiteSpace($ConfigPath)) {
  $ConfigPath = Join-Path $installRoot "config.json"
}

$hostname = if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { "Windows Device" }
$defaultDeviceId = ($hostname.ToLowerInvariant() -replace "[^a-z0-9._-]+", "-").Trim("-")
$workspace = Join-Path $env:USERPROFILE "Desktop\OH-WorkSpace"
$defaultRoot = if (Test-Path -LiteralPath $workspace -PathType Container) { $workspace } else { $env:USERPROFILE }
$defaults = [ordered]@{
  DeviceId = $defaultDeviceId
  DeviceName = $hostname
  RootPath = $defaultRoot
  VpsHost = "YOUR_SERVER_IP"
  CloudUrl = "wss://your-server.example.com/local-bridge/connect"
  SshUser = "root"
  IdentityFile = ""
  McpPort = 8787
  ApprovalPort = 8788
  RemotePort = 18787
  TaskPrefix = "Hanako Local FS"
  TunnelEnabled = $false
}

if (Test-Path -LiteralPath $ConfigPath -PathType Leaf) {
  try {
    $existing = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    $writeRoot = $existing.filesystem.roots |
      Where-Object { $_.mode -eq "read_write" } |
      Select-Object -First 1
    if ($existing.device.id) { $defaults.DeviceId = [string]$existing.device.id }
    if ($existing.device.name) { $defaults.DeviceName = [string]$existing.device.name }
    if ($writeRoot.path) { $defaults.RootPath = [string]$writeRoot.path }
    if ($existing.tunnel.server) { $defaults.VpsHost = [string]$existing.tunnel.server }
    if ($existing.cloud.url) {
      $defaults.CloudUrl = [string]$existing.cloud.url
      if ($defaults.CloudUrl -eq "ws://YOUR_SERVER_IP/local-bridge/connect") {
        $defaults.CloudUrl = "wss://your-server.example.com/local-bridge/connect"
      }
    }
    if ($existing.tunnel.user) { $defaults.SshUser = [string]$existing.tunnel.user }
    if ($existing.tunnel.identityFile) { $defaults.IdentityFile = [string]$existing.tunnel.identityFile }
    if ($existing.filesystem.port) { $defaults.McpPort = [int]$existing.filesystem.port }
    if ($existing.filesystem.approvalPort) { $defaults.ApprovalPort = [int]$existing.filesystem.approvalPort }
    if ($existing.tunnel.remotePort) { $defaults.RemotePort = [int]$existing.tunnel.remotePort }
    if ($existing.service.taskPrefix) { $defaults.TaskPrefix = [string]$existing.service.taskPrefix }
    if ($existing.cloud -and $null -ne $existing.cloud.enabled) {
      $defaults.TunnelEnabled = -not [bool]$existing.cloud.enabled -and [bool]$existing.tunnel.enabled
    } else {
      $defaults.TunnelEnabled = $false
    }
  } catch {}
}

foreach ($entry in @{
  DeviceId = $DeviceId
  DeviceName = $DeviceName
  RootPath = $RootPath
  VpsHost = $VpsHost
  CloudUrl = $CloudUrl
  SshUser = $SshUser
  IdentityFile = $IdentityFile
  TaskPrefix = $TaskPrefix
}.GetEnumerator()) {
  if (-not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
    $defaults[$entry.Key] = [string]$entry.Value
  }
}
if ($McpPort -gt 0) { $defaults.McpPort = $McpPort }
if ($ApprovalPort -gt 0) { $defaults.ApprovalPort = $ApprovalPort }
if ($RemotePort -gt 0) { $defaults.RemotePort = $RemotePort }
if ($DisableTunnel) { $defaults.TunnelEnabled = $false }

$form = [System.Windows.Forms.Form]::new()
$form.Text = "Hanako Local Bridge Setup"
$form.StartPosition = "CenterScreen"
$form.FormBorderStyle = "FixedDialog"
$form.ClientSize = [System.Drawing.Size]::new(620, 500)
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.ShowIcon = $false
$form.TopMost = $true
$form.Font = [System.Drawing.Font]::new("Segoe UI", 9)

$title = [System.Windows.Forms.Label]::new()
$title.Text = "Install or repair the local file and execution bridge"
$title.Font = [System.Drawing.Font]::new("Segoe UI Semibold", 13)
$title.AutoSize = $true
$title.Location = [System.Drawing.Point]::new(24, 20)
$form.Controls.Add($title)

$subtitle = [System.Windows.Forms.Label]::new()
$subtitle.Text = "Review the settings below. The service runs silently after installation."
$subtitle.AutoSize = $true
$subtitle.ForeColor = [System.Drawing.Color]::DimGray
$subtitle.Location = [System.Drawing.Point]::new(27, 52)
$form.Controls.Add($subtitle)

$table = [System.Windows.Forms.TableLayoutPanel]::new()
$table.Location = [System.Drawing.Point]::new(24, 82)
$table.Size = [System.Drawing.Size]::new(572, 330)
$table.ColumnCount = 3
$table.RowCount = 9
$table.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Absolute, 150))
$table.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))
$table.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Absolute, 82))
for ($index = 0; $index -lt 9; $index++) {
  $table.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 36))
}
$form.Controls.Add($table)

function Add-Field {
  param(
    [int]$Row,
    [string]$Label,
    [string]$Value,
    [switch]$Browse
  )

  $labelControl = [System.Windows.Forms.Label]::new()
  $labelControl.Text = $Label
  $labelControl.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
  $labelControl.Dock = [System.Windows.Forms.DockStyle]::Fill
  $table.Controls.Add($labelControl, 0, $Row)

  $textControl = [System.Windows.Forms.TextBox]::new()
  $textControl.Text = $Value
  $textControl.Dock = [System.Windows.Forms.DockStyle]::Fill
  $textControl.Margin = [System.Windows.Forms.Padding]::new(3, 7, 6, 4)
  $table.Controls.Add($textControl, 1, $Row)

  if ($Browse) {
    $button = [System.Windows.Forms.Button]::new()
    $button.Text = "Browse..."
    $button.Dock = [System.Windows.Forms.DockStyle]::Fill
    $button.Margin = [System.Windows.Forms.Padding]::new(3, 5, 3, 3)
    $browseHandler = {
      $dialog = [System.Windows.Forms.FolderBrowserDialog]::new()
      $dialog.Description = "Select the writable local root"
      $dialog.SelectedPath = $textControl.Text
      if ($dialog.ShowDialog($form) -eq [System.Windows.Forms.DialogResult]::OK) {
        $textControl.Text = $dialog.SelectedPath
      }
      $dialog.Dispose()
    }.GetNewClosure()
    $button.Add_Click($browseHandler)
    $table.Controls.Add($button, 2, $Row)
  }

  return $textControl
}

$deviceNameBox = Add-Field -Row 0 -Label "Device name" -Value ([string]$defaults.DeviceName)
$deviceIdBox = Add-Field -Row 1 -Label "Device ID" -Value ([string]$defaults.DeviceId)
$rootPathBox = Add-Field -Row 2 -Label "Writable local root" -Value ([string]$defaults.RootPath) -Browse
$vpsHostBox = Add-Field -Row 3 -Label "Cloud WebSocket URL" -Value ([string]$defaults.CloudUrl)
$sshUserBox = Add-Field -Row 4 -Label "Legacy SSH user" -Value ([string]$defaults.SshUser)
$mcpPortBox = Add-Field -Row 5 -Label "Local MCP port" -Value ([string]$defaults.McpPort)
$approvalPortBox = Add-Field -Row 6 -Label "Local status port" -Value ([string]$defaults.ApprovalPort)
$remotePortBox = Add-Field -Row 7 -Label "Legacy tunnel port" -Value ([string]$defaults.RemotePort)

$tunnelCheck = [System.Windows.Forms.CheckBox]::new()
$tunnelCheck.Text = "Use legacy SSH instead of the automatic cloud WebSocket"
$tunnelCheck.Checked = [bool]$defaults.TunnelEnabled
$tunnelCheck.Dock = [System.Windows.Forms.DockStyle]::Fill
$tunnelCheck.Margin = [System.Windows.Forms.Padding]::new(3, 7, 3, 3)
$table.Controls.Add($tunnelCheck, 0, 8)
$table.SetColumnSpan($tunnelCheck, 3)

$cancelButton = [System.Windows.Forms.Button]::new()
$cancelButton.Text = "Cancel"
$cancelButton.Size = [System.Drawing.Size]::new(92, 32)
$cancelButton.Location = [System.Drawing.Point]::new(400, 444)
$cancelButton.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$form.Controls.Add($cancelButton)

$installButton = [System.Windows.Forms.Button]::new()
$installButton.Text = "Install / Repair"
$installButton.Size = [System.Drawing.Size]::new(104, 32)
$installButton.Location = [System.Drawing.Point]::new(500, 444)
$installButton.Add_Click({
  try {
    foreach ($required in @($deviceNameBox, $deviceIdBox, $rootPathBox, $vpsHostBox)) {
      if ([string]::IsNullOrWhiteSpace($required.Text)) {
        throw "All text fields are required."
      }
    }
    if (-not (Test-Path -LiteralPath $rootPathBox.Text -PathType Container)) {
      throw "The writable local root does not exist."
    }
    $ports = @($mcpPortBox.Text, $approvalPortBox.Text)
    if ($tunnelCheck.Checked) { $ports += $remotePortBox.Text }
    foreach ($portText in $ports) {
      $port = 0
      if (-not [int]::TryParse($portText, [ref]$port) -or $port -lt 1 -or $port -gt 65535) {
        throw "Ports must be numbers from 1 to 65535."
      }
    }
    if ($mcpPortBox.Text -eq $approvalPortBox.Text) {
      throw "The MCP and status ports must be different."
    }
    $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
    $form.Close()
  } catch {
    [System.Windows.Forms.MessageBox]::Show(
      $form,
      $_.Exception.Message,
      "Invalid settings",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Warning
    ) | Out-Null
  }
})
$form.Controls.Add($installButton)
$form.AcceptButton = $installButton
$form.CancelButton = $cancelButton

$dialogResult = $form.ShowDialog()
if ($dialogResult -ne [System.Windows.Forms.DialogResult]::OK) {
  $form.Dispose()
  return [pscustomobject]@{ Cancelled = $true }
}

$result = [pscustomobject]@{
  Cancelled = $false
  DeviceId = $deviceIdBox.Text.Trim()
  DeviceName = $deviceNameBox.Text.Trim()
  RootPath = [System.IO.Path]::GetFullPath($rootPathBox.Text.Trim())
  VpsHost = [string]$defaults.VpsHost
  CloudUrl = $vpsHostBox.Text.Trim()
  SshUser = $sshUserBox.Text.Trim()
  IdentityFile = [string]$defaults.IdentityFile
  McpPort = [int]$mcpPortBox.Text
  ApprovalPort = [int]$approvalPortBox.Text
  RemotePort = [int]$remotePortBox.Text
  TaskPrefix = [string]$defaults.TaskPrefix
  TunnelEnabled = [bool]$tunnelCheck.Checked
}
$form.Dispose()

if ($CollectOnly) {
  return $result
}

$configureArguments = @{
  InstallRoot = $installRoot
  ConfigPath = $ConfigPath
  DeviceId = $result.DeviceId
  DeviceName = $result.DeviceName
  RootPath = $result.RootPath
  VpsHost = $result.VpsHost
  CloudUrl = $result.CloudUrl
  SshUser = $result.SshUser
  McpPort = $result.McpPort
  ApprovalPort = $result.ApprovalPort
  RemotePort = $result.RemotePort
  TaskPrefix = $result.TaskPrefix
  NonInteractive = $true
}
if (-not [string]::IsNullOrWhiteSpace($result.IdentityFile)) {
  $configureArguments.IdentityFile = $result.IdentityFile
}
if ($result.TunnelEnabled) {
  $configureArguments.DisableCloud = $true
  $configureArguments.UseLegacySshTunnel = $true
} else {
  $configureArguments.DisableTunnel = $true
}

& (Join-Path $installRoot "configure.ps1") @configureArguments
& (Join-Path $installRoot "repair.ps1") -ConfigPath $ConfigPath -NonInteractive
[System.Windows.Forms.MessageBox]::Show(
  "Hanako Local Bridge settings were saved and the background service was restarted.",
  "Hanako Local Bridge",
  [System.Windows.Forms.MessageBoxButtons]::OK,
  [System.Windows.Forms.MessageBoxIcon]::Information
) | Out-Null
return $result
