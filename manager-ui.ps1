param(
  [string]$InstallRoot = $PSScriptRoot,
  [string]$ConfigPath = ""
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

[System.Windows.Forms.Application]::EnableVisualStyles()
try {
  [System.Windows.Forms.Application]::SetHighDpiMode(
    [System.Windows.Forms.HighDpiMode]::PerMonitorV2
  ) | Out-Null
} catch {}

$installRoot = [System.IO.Path]::GetFullPath($InstallRoot)
. (Join-Path $installRoot "manager-core.ps1")

$script:Snapshot = $null
$script:Busy = $false
$script:CheckNames = @{
  config = "配置文件"
  package = "程序文件"
  mcp_task = "后台计划任务"
  hidden_launcher = "无感启动"
  mcp_process = "MCP 进程"
  mcp_health = "本地 MCP"
  status_health = "状态服务"
  cloud = "云端 WebSocket"
  identity = "设备凭证"
  tunnel = "旧 SSH 隧道"
}

function Get-ManagerFont {
  param(
    [float]$Size = 9,
    [System.Drawing.FontStyle]$Style = [System.Drawing.FontStyle]::Regular
  )
  [System.Drawing.Font]::new("Microsoft YaHei UI", $Size, $Style)
}

function New-ManagerButton {
  param(
    [string]$Text,
    [int]$Width = 96
  )
  $button = [System.Windows.Forms.Button]::new()
  $button.Text = $Text
  $button.Width = $Width
  $button.Height = 32
  $button.Margin = [System.Windows.Forms.Padding]::new(0, 0, 8, 0)
  $button.FlatStyle = [System.Windows.Forms.FlatStyle]::System
  $button
}

function New-ManagerValueLabel {
  $label = [System.Windows.Forms.Label]::new()
  $label.AutoEllipsis = $true
  $label.Dock = [System.Windows.Forms.DockStyle]::Fill
  $label.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
  $label.Font = Get-ManagerFont
  $label
}

function Add-ManagerField {
  param(
    [System.Windows.Forms.TableLayoutPanel]$Table,
    [int]$Row,
    [string]$Caption,
    [System.Windows.Forms.Control]$ValueControl
  )
  $captionLabel = [System.Windows.Forms.Label]::new()
  $captionLabel.Text = $Caption
  $captionLabel.Dock = [System.Windows.Forms.DockStyle]::Fill
  $captionLabel.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
  $captionLabel.ForeColor = [System.Drawing.Color]::FromArgb(90, 90, 90)
  $captionLabel.Font = Get-ManagerFont
  $captionLabel.Padding = [System.Windows.Forms.Padding]::new(4, 0, 0, 0)
  $Table.Controls.Add($captionLabel, 0, $Row)
  $Table.Controls.Add($ValueControl, 1, $Row)
}

function Format-ManagerTime {
  param([string]$Value)
  if ([string]::IsNullOrWhiteSpace($Value)) { return "-" }
  try {
    ([DateTimeOffset]::Parse($Value)).ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss")
  } catch {
    $Value
  }
}

function Get-ManagerStatusText {
  param([string]$Status)
  switch ($Status) {
    "healthy" { "正常" }
    "warning" { "需要处理" }
    "error" { "异常" }
    "active" { "已连接" }
    "pending_claim" { "等待认领" }
    "offline" { "离线" }
    "disabled" { "已停用" }
    "pass" { "通过" }
    default { if ($Status) { $Status } else { "-" } }
  }
}

function Get-ManagerStatusColor {
  param([string]$Status)
  switch ($Status) {
    { $_ -in @("healthy", "active", "pass", "online") } {
      [System.Drawing.Color]::FromArgb(24, 128, 72)
      break
    }
    { $_ -in @("warning", "pending_claim", "pending") } {
      [System.Drawing.Color]::FromArgb(176, 104, 0)
      break
    }
    default {
      [System.Drawing.Color]::FromArgb(185, 42, 42)
    }
  }
}

$form = [System.Windows.Forms.Form]::new()
$form.Text = "Hanako Local Bridge 管理器"
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.Size = [System.Drawing.Size]::new(1040, 760)
$form.MinimumSize = [System.Drawing.Size]::new(900, 650)
$form.BackColor = [System.Drawing.Color]::FromArgb(245, 246, 248)
$form.Font = Get-ManagerFont

$header = [System.Windows.Forms.Panel]::new()
$header.Dock = [System.Windows.Forms.DockStyle]::Top
$header.Height = 76
$header.Padding = [System.Windows.Forms.Padding]::new(18, 12, 18, 10)
$header.BackColor = [System.Drawing.Color]::White

$titleLabel = [System.Windows.Forms.Label]::new()
$titleLabel.Text = "Hanako Local Bridge"
$titleLabel.AutoSize = $true
$titleLabel.Location = [System.Drawing.Point]::new(18, 12)
$titleLabel.Font = Get-ManagerFont -Size 15 -Style ([System.Drawing.FontStyle]::Bold)

$deviceSummaryLabel = [System.Windows.Forms.Label]::new()
$deviceSummaryLabel.Text = "正在检测..."
$deviceSummaryLabel.AutoSize = $true
$deviceSummaryLabel.Location = [System.Drawing.Point]::new(20, 45)
$deviceSummaryLabel.ForeColor = [System.Drawing.Color]::FromArgb(90, 90, 90)

$overallLabel = [System.Windows.Forms.Label]::new()
$overallLabel.Text = "检测中"
$overallLabel.AutoSize = $false
$overallLabel.Size = [System.Drawing.Size]::new(116, 34)
$overallLabel.Anchor = [System.Windows.Forms.AnchorStyles]::Top -bor [System.Windows.Forms.AnchorStyles]::Right
$overallLabel.Location = [System.Drawing.Point]::new($form.ClientSize.Width - 150, 20)
$overallLabel.TextAlign = [System.Drawing.ContentAlignment]::MiddleCenter
$overallLabel.Font = Get-ManagerFont -Size 10 -Style ([System.Drawing.FontStyle]::Bold)
$overallLabel.ForeColor = [System.Drawing.Color]::White
$overallLabel.BackColor = [System.Drawing.Color]::FromArgb(95, 99, 104)
$header.add_Resize({
  $overallLabel.Left = $header.ClientSize.Width - $overallLabel.Width - 18
})

$header.Controls.AddRange(@($titleLabel, $deviceSummaryLabel, $overallLabel))

$footer = [System.Windows.Forms.StatusStrip]::new()
$footer.SizingGrip = $false
$footer.BackColor = [System.Drawing.Color]::White
$statusText = [System.Windows.Forms.ToolStripStatusLabel]::new()
$statusText.Spring = $true
$statusText.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
$statusText.Text = "就绪"
$refreshTimeText = [System.Windows.Forms.ToolStripStatusLabel]::new()
$refreshTimeText.Text = ""
[void]$footer.Items.Add($statusText)
[void]$footer.Items.Add($refreshTimeText)

$tabs = [System.Windows.Forms.TabControl]::new()
$tabs.Dock = [System.Windows.Forms.DockStyle]::Fill
$tabs.Padding = [System.Drawing.Point]::new(18, 6)

$overviewTab = [System.Windows.Forms.TabPage]::new("概览")
$diagnosticsTab = [System.Windows.Forms.TabPage]::new("诊断与修复")
$cloudTab = [System.Windows.Forms.TabPage]::new("云端设备")
$logsTab = [System.Windows.Forms.TabPage]::new("日志")
foreach ($tab in @($overviewTab, $diagnosticsTab, $cloudTab, $logsTab)) {
  $tab.BackColor = [System.Drawing.Color]::FromArgb(245, 246, 248)
  $tab.Padding = [System.Windows.Forms.Padding]::new(12)
  [void]$tabs.TabPages.Add($tab)
}

$overviewLayout = [System.Windows.Forms.TableLayoutPanel]::new()
$overviewLayout.Dock = [System.Windows.Forms.DockStyle]::Fill
$overviewLayout.ColumnCount = 2
$overviewLayout.RowCount = 2
[void]$overviewLayout.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Percent, 50))
[void]$overviewLayout.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Percent, 50))
[void]$overviewLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))
[void]$overviewLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 58))

$localGroup = [System.Windows.Forms.GroupBox]::new()
$localGroup.Text = "本机服务"
$localGroup.Dock = [System.Windows.Forms.DockStyle]::Fill
$localGroup.Margin = [System.Windows.Forms.Padding]::new(0, 0, 6, 8)
$localGroup.Padding = [System.Windows.Forms.Padding]::new(10, 14, 10, 10)

$cloudGroup = [System.Windows.Forms.GroupBox]::new()
$cloudGroup.Text = "云端连接"
$cloudGroup.Dock = [System.Windows.Forms.DockStyle]::Fill
$cloudGroup.Margin = [System.Windows.Forms.Padding]::new(6, 0, 0, 8)
$cloudGroup.Padding = [System.Windows.Forms.Padding]::new(10, 14, 10, 10)

function New-ManagerDetailsTable {
  param([int]$Rows)
  $table = [System.Windows.Forms.TableLayoutPanel]::new()
  $table.Dock = [System.Windows.Forms.DockStyle]::Fill
  $table.ColumnCount = 2
  $table.RowCount = $Rows
  [void]$table.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Absolute, 112))
  [void]$table.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))
  for ($i = 0; $i -lt $Rows; $i++) {
    [void]$table.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, (100 / $Rows)))
  }
  $table
}

$localTable = New-ManagerDetailsTable -Rows 8
$deviceIdValue = New-ManagerValueLabel
$deviceNameValue = New-ManagerValueLabel
$versionValue = New-ManagerValueLabel
$mcpPortValue = New-ManagerValueLabel
$statusPortValue = New-ManagerValueLabel
$trustModeValue = New-ManagerValueLabel
$taskValue = New-ManagerValueLabel
$processValue = New-ManagerValueLabel
Add-ManagerField $localTable 0 "设备 ID" $deviceIdValue
Add-ManagerField $localTable 1 "设备名称" $deviceNameValue
Add-ManagerField $localTable 2 "版本" $versionValue
Add-ManagerField $localTable 3 "MCP 端口" $mcpPortValue
Add-ManagerField $localTable 4 "状态端口" $statusPortValue
Add-ManagerField $localTable 5 "访问模式" $trustModeValue
Add-ManagerField $localTable 6 "计划任务" $taskValue
Add-ManagerField $localTable 7 "服务进程" $processValue
$localGroup.Controls.Add($localTable)

$cloudTable = New-ManagerDetailsTable -Rows 8
$cloudStatusValue = New-ManagerValueLabel
$cloudUrlValue = New-ManagerValueLabel
$credentialValue = New-ManagerValueLabel
$lastConnectedValue = New-ManagerValueLabel
$lastSeenValue = New-ManagerValueLabel
$cloudErrorValue = New-ManagerValueLabel
$fingerprintValue = New-ManagerValueLabel
$recommendationValue = New-ManagerValueLabel
Add-ManagerField $cloudTable 0 "连接状态" $cloudStatusValue
Add-ManagerField $cloudTable 1 "云端地址" $cloudUrlValue
Add-ManagerField $cloudTable 2 "设备凭证" $credentialValue
Add-ManagerField $cloudTable 3 "连接时间" $lastConnectedValue
Add-ManagerField $cloudTable 4 "最后心跳" $lastSeenValue
Add-ManagerField $cloudTable 5 "最近错误" $cloudErrorValue
Add-ManagerField $cloudTable 6 "公钥指纹" $fingerprintValue
Add-ManagerField $cloudTable 7 "建议" $recommendationValue
$cloudGroup.Controls.Add($cloudTable)

$overviewActions = [System.Windows.Forms.FlowLayoutPanel]::new()
$overviewActions.Dock = [System.Windows.Forms.DockStyle]::Fill
$overviewActions.FlowDirection = [System.Windows.Forms.FlowDirection]::LeftToRight
$overviewActions.WrapContents = $false
$overviewActions.Padding = [System.Windows.Forms.Padding]::new(0, 8, 0, 0)

$refreshButton = New-ManagerButton "刷新"
$startButton = New-ManagerButton "启动"
$stopButton = New-ManagerButton "停止"
$restartButton = New-ManagerButton "重启"
$repairButton = New-ManagerButton "检测并修复" 112
$settingsButton = New-ManagerButton "设置" 82
$statusPageButton = New-ManagerButton "本地状态页" 108
$webButton = New-ManagerButton "Hana 网页端" 108
$overviewActions.Controls.AddRange(@(
  $refreshButton, $startButton, $stopButton, $restartButton,
  $repairButton, $settingsButton, $statusPageButton, $webButton
))

$overviewLayout.Controls.Add($localGroup, 0, 0)
$overviewLayout.Controls.Add($cloudGroup, 1, 0)
$overviewLayout.Controls.Add($overviewActions, 0, 1)
$overviewLayout.SetColumnSpan($overviewActions, 2)
$overviewTab.Controls.Add($overviewLayout)

$diagnosticsLayout = [System.Windows.Forms.TableLayoutPanel]::new()
$diagnosticsLayout.Dock = [System.Windows.Forms.DockStyle]::Fill
$diagnosticsLayout.RowCount = 2
$diagnosticsLayout.ColumnCount = 1
[void]$diagnosticsLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))
[void]$diagnosticsLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 54))

$checksList = [System.Windows.Forms.ListView]::new()
$checksList.Dock = [System.Windows.Forms.DockStyle]::Fill
$checksList.View = [System.Windows.Forms.View]::Details
$checksList.FullRowSelect = $true
$checksList.GridLines = $true
$checksList.HideSelection = $false
[void]$checksList.Columns.Add("检查项", 180)
[void]$checksList.Columns.Add("状态", 100)
[void]$checksList.Columns.Add("详情", 590)
$checksList.add_Resize({
  $remaining = $checksList.ClientSize.Width - $checksList.Columns[0].Width - $checksList.Columns[1].Width - 6
  if ($remaining -gt 220) { $checksList.Columns[2].Width = $remaining }
})

$diagnosticsActions = [System.Windows.Forms.FlowLayoutPanel]::new()
$diagnosticsActions.Dock = [System.Windows.Forms.DockStyle]::Fill
$diagnosticsActions.Padding = [System.Windows.Forms.Padding]::new(0, 8, 0, 0)
$diagnoseRefreshButton = New-ManagerButton "重新检测" 104
$diagnoseRepairButton = New-ManagerButton "一键修复" 104
$copyReportButton = New-ManagerButton "复制诊断报告" 122
$diagnosticsActions.Controls.AddRange(@($diagnoseRefreshButton, $diagnoseRepairButton, $copyReportButton))
$diagnosticsLayout.Controls.Add($checksList, 0, 0)
$diagnosticsLayout.Controls.Add($diagnosticsActions, 0, 1)
$diagnosticsTab.Controls.Add($diagnosticsLayout)

$cloudLayout = [System.Windows.Forms.TableLayoutPanel]::new()
$cloudLayout.Dock = [System.Windows.Forms.DockStyle]::Fill
$cloudLayout.ColumnCount = 1
$cloudLayout.RowCount = 3
[void]$cloudLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 102))
[void]$cloudLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 44))
[void]$cloudLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))

$cloudLoginTable = [System.Windows.Forms.TableLayoutPanel]::new()
$cloudLoginTable.Dock = [System.Windows.Forms.DockStyle]::Fill
$cloudLoginTable.ColumnCount = 2
$cloudLoginTable.RowCount = 2
[void]$cloudLoginTable.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Absolute, 120))
[void]$cloudLoginTable.ColumnStyles.Add([System.Windows.Forms.ColumnStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))
[void]$cloudLoginTable.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 50))
[void]$cloudLoginTable.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 50))
$cloudBaseText = [System.Windows.Forms.TextBox]::new()
$cloudBaseText.Dock = [System.Windows.Forms.DockStyle]::Fill
$cloudBaseText.Margin = [System.Windows.Forms.Padding]::new(4, 8, 4, 6)
$accessKeyText = [System.Windows.Forms.TextBox]::new()
$accessKeyText.Dock = [System.Windows.Forms.DockStyle]::Fill
$accessKeyText.UseSystemPasswordChar = $true
$accessKeyText.Margin = [System.Windows.Forms.Padding]::new(4, 6, 4, 8)
Add-ManagerField $cloudLoginTable 0 "Hana 网页地址" $cloudBaseText
Add-ManagerField $cloudLoginTable 1 "访问密钥" $accessKeyText

$cloudActions = [System.Windows.Forms.FlowLayoutPanel]::new()
$cloudActions.Dock = [System.Windows.Forms.DockStyle]::Fill
$cloudActions.Padding = [System.Windows.Forms.Padding]::new(0, 5, 0, 5)
$queryDevicesButton = New-ManagerButton "查询云端设备" 122
$claimDeviceButton = New-ManagerButton "登录并认领本机" 138
$cloudHintLabel = [System.Windows.Forms.Label]::new()
$cloudHintLabel.AutoSize = $true
$cloudHintLabel.Margin = [System.Windows.Forms.Padding]::new(12, 8, 0, 0)
$cloudHintLabel.ForeColor = [System.Drawing.Color]::FromArgb(90, 90, 90)
$cloudHintLabel.Text = "访问密钥只用于本次操作，不会保存。"
$cloudActions.Controls.AddRange(@($queryDevicesButton, $claimDeviceButton, $cloudHintLabel))

$devicesGrid = [System.Windows.Forms.DataGridView]::new()
$devicesGrid.Dock = [System.Windows.Forms.DockStyle]::Fill
$devicesGrid.ReadOnly = $true
$devicesGrid.AllowUserToAddRows = $false
$devicesGrid.AllowUserToDeleteRows = $false
$devicesGrid.AllowUserToResizeRows = $false
$devicesGrid.MultiSelect = $false
$devicesGrid.SelectionMode = [System.Windows.Forms.DataGridViewSelectionMode]::FullRowSelect
$devicesGrid.AutoSizeColumnsMode = [System.Windows.Forms.DataGridViewAutoSizeColumnsMode]::Fill
$devicesGrid.BackgroundColor = [System.Drawing.Color]::White
$devicesGrid.BorderStyle = [System.Windows.Forms.BorderStyle]::FixedSingle
[void]$devicesGrid.Columns.Add("DeviceId", "设备 ID")
[void]$devicesGrid.Columns.Add("DeviceName", "设备名称")
[void]$devicesGrid.Columns.Add("Version", "版本")
[void]$devicesGrid.Columns.Add("Status", "状态")
[void]$devicesGrid.Columns.Add("LastSeen", "最后在线")

$cloudLayout.Controls.Add($cloudLoginTable, 0, 0)
$cloudLayout.Controls.Add($cloudActions, 0, 1)
$cloudLayout.Controls.Add($devicesGrid, 0, 2)
$cloudTab.Controls.Add($cloudLayout)

$logsLayout = [System.Windows.Forms.TableLayoutPanel]::new()
$logsLayout.Dock = [System.Windows.Forms.DockStyle]::Fill
$logsLayout.RowCount = 2
$logsLayout.ColumnCount = 1
[void]$logsLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Absolute, 48))
[void]$logsLayout.RowStyles.Add([System.Windows.Forms.RowStyle]::new([System.Windows.Forms.SizeType]::Percent, 100))

$logsToolbar = [System.Windows.Forms.FlowLayoutPanel]::new()
$logsToolbar.Dock = [System.Windows.Forms.DockStyle]::Fill
$logsToolbar.Padding = [System.Windows.Forms.Padding]::new(0, 6, 0, 4)
$logCombo = [System.Windows.Forms.ComboBox]::new()
$logCombo.DropDownStyle = [System.Windows.Forms.ComboBoxStyle]::DropDownList
$logCombo.Width = 360
$logCombo.Height = 30
$logRefreshButton = New-ManagerButton "刷新日志" 96
$openLogFolderButton = New-ManagerButton "打开日志目录" 118
$logsToolbar.Controls.AddRange(@($logCombo, $logRefreshButton, $openLogFolderButton))

$logText = [System.Windows.Forms.TextBox]::new()
$logText.Dock = [System.Windows.Forms.DockStyle]::Fill
$logText.Multiline = $true
$logText.ReadOnly = $true
$logText.ScrollBars = [System.Windows.Forms.ScrollBars]::Both
$logText.WordWrap = $false
$logText.BackColor = [System.Drawing.Color]::FromArgb(26, 28, 31)
$logText.ForeColor = [System.Drawing.Color]::FromArgb(222, 226, 230)
$logText.Font = [System.Drawing.Font]::new("Consolas", 9)

$logsLayout.Controls.Add($logsToolbar, 0, 0)
$logsLayout.Controls.Add($logText, 0, 1)
$logsTab.Controls.Add($logsLayout)

$form.Controls.Add($tabs)
$form.Controls.Add($header)
$form.Controls.Add($footer)

function Set-ManagerBusy {
  param(
    [bool]$Busy,
    [string]$Message = ""
  )
  $script:Busy = $Busy
  $form.UseWaitCursor = $Busy
  foreach ($control in @(
    $refreshButton, $startButton, $stopButton, $restartButton, $repairButton,
    $diagnoseRefreshButton, $diagnoseRepairButton, $queryDevicesButton, $claimDeviceButton
  )) {
    $control.Enabled = -not $Busy
  }
  if ($Message) { $statusText.Text = $Message }
  [System.Windows.Forms.Application]::DoEvents()
}

function Update-ManagerChecks {
  param($Snapshot)
  $checksList.BeginUpdate()
  try {
    $checksList.Items.Clear()
    foreach ($check in @($Snapshot.checks)) {
      $name = if ($script:CheckNames.ContainsKey([string]$check.code)) {
        $script:CheckNames[[string]$check.code]
      } else {
        [string]$check.code
      }
      $item = [System.Windows.Forms.ListViewItem]::new($name)
      [void]$item.SubItems.Add((Get-ManagerStatusText ([string]$check.status)))
      [void]$item.SubItems.Add([string]$check.detail)
      $item.ForeColor = Get-ManagerStatusColor ([string]$check.status)
      [void]$checksList.Items.Add($item)
    }
  } finally {
    $checksList.EndUpdate()
  }
}

function Update-ManagerSnapshot {
  param($Snapshot)
  $script:Snapshot = $Snapshot
  $overallLabel.Text = Get-ManagerStatusText ([string]$Snapshot.overall)
  $overallLabel.BackColor = Get-ManagerStatusColor ([string]$Snapshot.overall)
  $deviceSummaryLabel.Text = if ($Snapshot.device) {
    "$($Snapshot.device.name)  |  $($Snapshot.device.id)  |  Bridge $($Snapshot.version)"
  } else {
    "配置无法加载"
  }

  if (-not $Snapshot.device) {
    $statusText.Text = [string]$Snapshot.recommendation
    Update-ManagerChecks $Snapshot
    return
  }

  $deviceIdValue.Text = [string]$Snapshot.device.id
  $deviceNameValue.Text = [string]$Snapshot.device.name
  $versionValue.Text = [string]$Snapshot.version
  $mcpPortValue.Text = [string]$Snapshot.local.mcpPort
  $statusPortValue.Text = [string]$Snapshot.local.statusPort
  $trustModeValue.Text = if ($Snapshot.local.trustMode -eq "full") { "全部权限" } else { [string]$Snapshot.local.trustMode }
  $taskValue.Text = "$($Snapshot.tasks.mcpState) / 隐藏启动: $(if ($Snapshot.tasks.hiddenLauncher) { '是' } else { '否' })"
  $node = @($Snapshot.processes | Where-Object { $_.Name -eq "node.exe" }) | Select-Object -First 1
  $processValue.Text = if ($node) { "node.exe PID $($node.ProcessId)" } else { "未运行" }

  $cloudStatusValue.Text = Get-ManagerStatusText ([string]$Snapshot.cloud.status)
  $cloudStatusValue.ForeColor = Get-ManagerStatusColor ([string]$Snapshot.cloud.status)
  $cloudUrlValue.Text = [string]$Snapshot.cloud.url
  $credentialValue.Text = if ($Snapshot.identity.credentialPresent) {
    "已保存"
  } elseif ($Snapshot.identity.claimTokenPresent) {
    "等待认领"
  } else {
    "缺失"
  }
  $lastConnectedValue.Text = Format-ManagerTime ([string]$Snapshot.cloud.lastConnectedAt)
  $lastSeenValue.Text = Format-ManagerTime ([string]$Snapshot.cloud.lastSeenAt)
  $cloudErrorValue.Text = if ($Snapshot.cloud.lastError) { [string]$Snapshot.cloud.lastError } else { "-" }
  $fingerprintValue.Text = if ($Snapshot.identity.publicKeyFingerprint) { [string]$Snapshot.identity.publicKeyFingerprint } else { "-" }
  $recommendationValue.Text = if ($Snapshot.overall -eq "healthy") {
    "本地 MCP 与云端连接正常"
  } elseif ($Snapshot.cloud.status -eq "pending_claim") {
    "请进入 [云端设备] 登录并认领本机"
  } elseif (-not $Snapshot.local.mcpHealthy) {
    "请点击 [检测并修复]"
  } else {
    "请查看 [诊断与修复] 中的异常项"
  }

  if ([string]::IsNullOrWhiteSpace($cloudBaseText.Text)) {
    $cloudBaseText.Text = [string]$Snapshot.cloud.webBaseUrl
  }
  $statusText.Text = if ($Snapshot.overall -eq "healthy") {
    "本地 MCP 与云端连接正常"
  } elseif ($Snapshot.cloud.status -eq "pending_claim") {
    "本机等待认领，请打开 [云端设备]"
  } else {
    [string]$Snapshot.recommendation
  }
  $refreshTimeText.Text = "更新于 " + (Get-Date).ToString("HH:mm:ss")
  Update-ManagerChecks $Snapshot
}

function Refresh-ManagerSnapshot {
  if ($script:Busy) { return }
  try {
    Set-ManagerBusy $true "正在检测本地服务..."
    $snapshot = Get-HanakoBridgeManagerSnapshot -InstallRoot $installRoot -ConfigPath $ConfigPath
    Update-ManagerSnapshot $snapshot
  } catch {
    $statusText.Text = "检测失败: $($_.Exception.Message)"
    $overallLabel.Text = "检测失败"
    $overallLabel.BackColor = Get-ManagerStatusColor "error"
  } finally {
    Set-ManagerBusy $false
  }
}

function Invoke-ManagerAction {
  param(
    [string]$Action,
    [string]$Label
  )
  try {
    Set-ManagerBusy $true "$Label..."
    $snapshot = Invoke-HanakoBridgeManagerAction `
      -Action $Action `
      -InstallRoot $installRoot `
      -ConfigPath $ConfigPath
    Update-ManagerSnapshot $snapshot
    $statusText.Text = "$Label完成"
  } catch {
    [System.Windows.Forms.MessageBox]::Show(
      $_.Exception.Message,
      "$Label失败",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Error
    ) | Out-Null
    $statusText.Text = "$Label失败"
  } finally {
    Set-ManagerBusy $false
  }
}

function Refresh-ManagerLogs {
  try {
    $selectedPath = ""
    if ($logCombo.SelectedItem) { $selectedPath = [string]$logCombo.SelectedItem.FullName }
    $files = @(Get-HanakoBridgeLogFiles -InstallRoot $installRoot)
    $logCombo.BeginUpdate()
    try {
      $logCombo.Items.Clear()
      foreach ($file in $files) { [void]$logCombo.Items.Add($file) }
      $logCombo.DisplayMember = "Name"
    } finally {
      $logCombo.EndUpdate()
    }
    if ($files.Count -gt 0) {
      $selectedIndex = 0
      for ($i = 0; $i -lt $files.Count; $i++) {
        if ([string]$files[$i].FullName -eq $selectedPath) { $selectedIndex = $i; break }
      }
      $logCombo.SelectedIndex = $selectedIndex
      $logText.Text = Get-HanakoBridgeLogTail -Path ([string]$files[$selectedIndex].FullName)
      $logText.SelectionStart = $logText.TextLength
      $logText.ScrollToCaret()
    } else {
      $logText.Text = "暂无日志文件。"
    }
  } catch {
    $logText.Text = "读取日志失败: $($_.Exception.Message)"
  }
}

function Update-CloudDevices {
  param($Result)
  $devicesGrid.Rows.Clear()
  foreach ($device in @($Result.devices)) {
    $lastSeen = if ($device.lastSeenAt) { Format-ManagerTime ([string]$device.lastSeenAt) } else { "-" }
    [void]$devicesGrid.Rows.Add(
      [string]$device.id,
      [string]$device.name,
      [string]$device.version,
      (Get-ManagerStatusText ([string]$device.status)),
      $lastSeen
    )
  }
}

function Invoke-CloudDeviceAction {
  param([bool]$Claim)
  try {
    Set-ManagerBusy $true $(if ($Claim) { "正在登录并认领本机..." } else { "正在查询云端设备..." })
    $arguments = @{
      BaseUrl = $cloudBaseText.Text
      AccessKey = $accessKeyText.Text
      InstallRoot = $installRoot
      ConfigPath = $ConfigPath
    }
    if ($Claim) { $arguments.ClaimCurrentDevice = $true }
    $result = Invoke-HanakoBridgeCloudQuery @arguments
    Update-CloudDevices $result
    $accessKeyText.Clear()
    if ($Claim) {
      $statusText.Text = if ($result.claimed) { "本机已认领并加入云端设备列表" } else { [string]$result.claimMessage }
      Start-Sleep -Milliseconds 500
      $snapshot = Get-HanakoBridgeManagerSnapshot -InstallRoot $installRoot -ConfigPath $ConfigPath
      Update-ManagerSnapshot $snapshot
    } else {
      $statusText.Text = "云端设备列表已更新，共 $(@($result.devices).Count) 台"
    }
  } catch {
    [System.Windows.Forms.MessageBox]::Show(
      $_.Exception.Message,
      "云端设备操作失败",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Error
    ) | Out-Null
    $statusText.Text = "云端设备操作失败"
  } finally {
    $accessKeyText.Clear()
    Set-ManagerBusy $false
  }
}

$refreshButton.add_Click({ Refresh-ManagerSnapshot })
$diagnoseRefreshButton.add_Click({ Refresh-ManagerSnapshot })
$startButton.add_Click({ Invoke-ManagerAction "start" "启动服务" })
$stopButton.add_Click({ Invoke-ManagerAction "stop" "停止服务" })
$restartButton.add_Click({ Invoke-ManagerAction "restart" "重启服务" })
$repairButton.add_Click({ Invoke-ManagerAction "repair" "检测并修复" })
$diagnoseRepairButton.add_Click({ Invoke-ManagerAction "repair" "检测并修复" })
$copyReportButton.add_Click({
  if ($script:Snapshot) {
    [System.Windows.Forms.Clipboard]::SetText(($script:Snapshot | ConvertTo-Json -Depth 8))
    $statusText.Text = "诊断报告已复制"
  }
})
$settingsButton.add_Click({
  try {
    Set-ManagerBusy $true "正在打开设置..."
    & (Join-Path $installRoot "configuration-ui.ps1") `
      -InstallRoot $installRoot `
      -ConfigPath $(if ($ConfigPath) { $ConfigPath } else { Join-Path $installRoot "config.json" }) | Out-Null
    Invoke-ManagerAction "repair" "应用设置"
  } catch {
    [System.Windows.Forms.MessageBox]::Show(
      $_.Exception.Message,
      "设置失败",
      [System.Windows.Forms.MessageBoxButtons]::OK,
      [System.Windows.Forms.MessageBoxIcon]::Error
    ) | Out-Null
  } finally {
    Set-ManagerBusy $false
  }
})
$statusPageButton.add_Click({
  if ($script:Snapshot -and $script:Snapshot.local) {
    Start-Process "http://127.0.0.1:$($script:Snapshot.local.statusPort)/"
  }
})
$webButton.add_Click({
  if ($script:Snapshot -and $script:Snapshot.cloud.webBaseUrl) {
    Start-Process "$($script:Snapshot.cloud.webBaseUrl)/desktop/"
  }
})
$queryDevicesButton.add_Click({ Invoke-CloudDeviceAction $false })
$claimDeviceButton.add_Click({ Invoke-CloudDeviceAction $true })
$logRefreshButton.add_Click({ Refresh-ManagerLogs })
$logCombo.add_SelectedIndexChanged({
  if ($logCombo.SelectedItem) {
    $logText.Text = Get-HanakoBridgeLogTail -Path ([string]$logCombo.SelectedItem.FullName)
    $logText.SelectionStart = $logText.TextLength
    $logText.ScrollToCaret()
  }
})
$openLogFolderButton.add_Click({
  try {
    $runtime = Get-BridgeRuntime -InstallRoot $installRoot -ConfigPath $ConfigPath
    $logDir = Resolve-HanakoBridgePath -InstallRoot $installRoot -Path ([string]$runtime.config.storage.logDir)
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
    Start-Process explorer.exe -ArgumentList "`"$logDir`""
  } catch {
    $statusText.Text = "无法打开日志目录: $($_.Exception.Message)"
  }
})
$tabs.add_SelectedIndexChanged({
  if ($tabs.SelectedTab -eq $logsTab) { Refresh-ManagerLogs }
})

$refreshTimer = [System.Windows.Forms.Timer]::new()
$refreshTimer.Interval = 5000
$refreshTimer.add_Tick({
  if (-not $script:Busy -and $form.WindowState -ne [System.Windows.Forms.FormWindowState]::Minimized) {
    Refresh-ManagerSnapshot
  }
})

$form.add_Shown({
  Refresh-ManagerSnapshot
  $refreshTimer.Start()
})
$form.add_FormClosed({
  $refreshTimer.Stop()
  $refreshTimer.Dispose()
})

[void]$form.ShowDialog()
