using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Collections.ObjectModel;
using System.Text.Json;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;

namespace HanakoBridgeManager;

public sealed partial class MainWindow : Window
{
    private readonly BridgeCommandService _service;
    private readonly DispatcherTimer _refreshTimer;
    private readonly bool _smokeTest;
    private ManagerSnapshot? _snapshot;
    private AppWindow? _appWindow;
    private bool _busy;
    private bool _loaded;

    public ObservableCollection<DiagnosticItemViewModel> Diagnostics { get; } = [];
    public ObservableCollection<CloudDeviceViewModel> CloudDevices { get; } = [];
    public ObservableCollection<LogFileItem> LogFiles { get; } = [];

    public MainWindow()
    {
        InitializeComponent();
        var arguments = Environment.GetCommandLineArgs();
        _smokeTest = arguments.Contains("--smoke-test", StringComparer.OrdinalIgnoreCase);
        _service = new BridgeCommandService(ResolveInstallRoot(arguments));
        _refreshTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(5) };
        _refreshTimer.Tick += RefreshTimer_Tick;

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        ConfigureWindow();
        Navigation.SelectedItem = OverviewNavItem;
        RootGrid.Loaded += RootGrid_Loaded;
        Closed += MainWindow_Closed;
    }

    private static string ResolveInstallRoot(string[] arguments)
    {
        for (var index = 0; index < arguments.Length - 1; index++)
        {
            if (arguments[index].Equals("--install-root", StringComparison.OrdinalIgnoreCase))
            {
                return Path.GetFullPath(arguments[index + 1]);
            }
        }

        var baseDirectory = Path.GetFullPath(AppContext.BaseDirectory);
        var parent = Directory.GetParent(baseDirectory)?.FullName ?? baseDirectory;
        if (File.Exists(Path.Combine(parent, "manager-command.ps1")))
        {
            return parent;
        }
        return baseDirectory;
    }

    private void ConfigureWindow()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
        _appWindow = AppWindow.GetFromWindowId(windowId);
        _appWindow.Title = "Hanako Local Bridge 管理器";

        var displayArea = DisplayArea.GetFromWindowId(windowId, DisplayAreaFallback.Primary);
        if (displayArea is not null)
        {
            var width = Math.Min(1180, Math.Max(1, displayArea.WorkArea.Width));
            var height = Math.Min(780, Math.Max(1, displayArea.WorkArea.Height));
            _appWindow.Resize(new SizeInt32(width, height));
            var x = displayArea.WorkArea.X + Math.Max(0, (displayArea.WorkArea.Width - width) / 2);
            var y = displayArea.WorkArea.Y + Math.Max(0, (displayArea.WorkArea.Height - height) / 2);
            _appWindow.Move(new PointInt32(x, y));
        }
        else
        {
            _appWindow.Resize(new SizeInt32(1180, 780));
        }

        if (AppWindowTitleBar.IsCustomizationSupported())
        {
            _appWindow.TitleBar.ButtonBackgroundColor = Colors.Transparent;
            _appWindow.TitleBar.ButtonInactiveBackgroundColor = Colors.Transparent;
        }
    }

    private async void RootGrid_Loaded(object sender, RoutedEventArgs e)
    {
        if (_loaded) return;
        _loaded = true;
        try
        {
            await RefreshSnapshotAsync(showErrors: !_smokeTest);
            if (_smokeTest)
            {
                Close();
                Environment.Exit(0);
                return;
            }
            _refreshTimer.Start();
        }
        catch
        {
            if (_smokeTest)
            {
                Close();
                Environment.Exit(1);
            }
        }
    }

    private void MainWindow_Closed(object sender, WindowEventArgs args)
    {
        _refreshTimer.Stop();
    }

    private async void RefreshTimer_Tick(object? sender, object e)
    {
        if (!_busy)
        {
            await RefreshSnapshotAsync(showErrors: false);
        }
    }

    private void Navigation_SelectionChanged(
        NavigationView sender,
        NavigationViewSelectionChangedEventArgs args)
    {
        var tag = (args.SelectedItemContainer?.Tag as string) ?? "overview";
        OverviewPage.Visibility = tag == "overview" ? Visibility.Visible : Visibility.Collapsed;
        DiagnosticsPage.Visibility = tag == "diagnostics" ? Visibility.Visible : Visibility.Collapsed;
        DevicesPage.Visibility = tag == "devices" ? Visibility.Visible : Visibility.Collapsed;
        LogsPage.Visibility = tag == "logs" ? Visibility.Visible : Visibility.Collapsed;
        if (tag == "logs" && LogFiles.Count == 0)
        {
            _ = RefreshLogsAsync();
        }
    }

    private async Task RefreshSnapshotAsync(bool showErrors = true)
    {
        if (_busy) return;
        SetBusy(true, "正在检测本地服务...");
        try
        {
            _snapshot = await _service.RunAsync<ManagerSnapshot>("snapshot");
            ApplySnapshot(_snapshot);
        }
        catch (Exception ex)
        {
            SetOverall("error");
            FooterStatusText.Text = $"检测失败：{ex.Message}";
            if (showErrors) ShowInfo("检测失败", ex.Message, InfoBarSeverity.Error);
            if (_smokeTest) throw;
        }
        finally
        {
            SetBusy(false);
        }
    }

    private void ApplySnapshot(ManagerSnapshot snapshot)
    {
        SetOverall(snapshot.Overall);
        var device = snapshot.Device;
        var local = snapshot.Local;
        var cloud = snapshot.Cloud;
        var identity = snapshot.Identity;
        var tasks = snapshot.Tasks;

        TitleDeviceText.Text = device is null
            ? "配置无法加载"
            : $"{device.Name}  ·  {device.Id}  ·  {snapshot.Version}";
        OverviewSubtitle.Text = device is null
            ? "配置无法加载，请执行检测并修复"
            : $"{device.Name} · Bridge {snapshot.Version}";

        McpTileText.Text = local?.McpHealthy == true ? "正常" : "异常";
        CloudTileText.Text = StatusText(cloud?.Status ?? "offline");
        CredentialTileText.Text = identity?.CredentialPresent == true
            ? "已保存"
            : identity?.ClaimTokenPresent == true ? "待认领" : "缺失";
        TaskTileText.Text = tasks?.McpState == "Running" ? "运行中" : StatusText(tasks?.McpState ?? "error");

        DeviceIdText.Text = device?.Id ?? "-";
        DeviceNameText.Text = device?.Name ?? "-";
        VersionText.Text = snapshot.Version;
        McpPortText.Text = local?.McpPort.ToString() ?? "-";
        StatusPortText.Text = local?.StatusPort.ToString() ?? "-";
        TrustModeText.Text = local?.TrustMode == "full" ? "全部权限" : local?.TrustMode ?? "-";
        var node = snapshot.Processes.FirstOrDefault(p => p.Name.Equals("node.exe", StringComparison.OrdinalIgnoreCase));
        ProcessText.Text = node is null ? "未运行" : $"node.exe PID {node.ProcessId}";

        CloudStatusText.Text = StatusText(cloud?.Status ?? "offline");
        CloudStatusText.Foreground = DiagnosticItemViewModel.StatusColor(cloud?.Status ?? "offline");
        CloudUrlText.Text = cloud?.Url ?? "-";
        LastConnectedText.Text = FormatTime(cloud?.LastConnectedAt);
        LastSeenText.Text = FormatTime(cloud?.LastSeenAt);
        CloudErrorText.Text = string.IsNullOrWhiteSpace(cloud?.LastError) ? "-" : cloud.LastError;
        FingerprintText.Text = string.IsNullOrWhiteSpace(identity?.PublicKeyFingerprint)
            ? "-"
            : identity.PublicKeyFingerprint;
        RecommendationText.Text = Recommendation(snapshot);

        if (string.IsNullOrWhiteSpace(CloudBaseUrlBox.Text) && !string.IsNullOrWhiteSpace(cloud?.WebBaseUrl))
        {
            CloudBaseUrlBox.Text = cloud.WebBaseUrl;
        }

        Diagnostics.Clear();
        foreach (var check in snapshot.Checks)
        {
            Diagnostics.Add(new DiagnosticItemViewModel(check));
        }

        FooterStatusText.Text = snapshot.Overall == "healthy"
            ? "本地 MCP 与云端连接正常"
            : Recommendation(snapshot);
        FooterRefreshText.Text = $"更新于 {DateTime.Now:HH:mm:ss}";
    }

    private static string Recommendation(ManagerSnapshot snapshot)
    {
        if (snapshot.Overall == "healthy") return "本地 MCP 与云端连接正常";
        if (snapshot.Cloud?.Status == "pending_claim") return "请进入 [云端设备] 登录并认领本机";
        if (snapshot.Local?.McpHealthy != true) return "请点击 [检测并修复]";
        return "请查看 [诊断与修复] 中的异常项";
    }

    private void SetOverall(string status)
    {
        OverallBadgeText.Text = status switch
        {
            "healthy" => "正常",
            "warning" => "需要处理",
            _ => "异常"
        };
        OverallBadge.Background = DiagnosticItemViewModel.StatusColor(status);
    }

    private static string StatusText(string status) => status switch
    {
        "healthy" or "active" or "pass" or "online" or "Running" => "已连接",
        "pending_claim" => "等待认领",
        "offline" => "离线",
        "disabled" => "已停用",
        "Ready" => "就绪",
        "Missing" => "缺失",
        _ => "异常"
    };

    public static string FormatTime(string? value)
    {
        if (string.IsNullOrWhiteSpace(value)) return "-";
        return DateTimeOffset.TryParse(value, out var parsed)
            ? parsed.ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss")
            : value;
    }

    private void SetBusy(bool busy, string? message = null)
    {
        _busy = busy;
        HeaderProgress.IsActive = busy;
        HeaderProgress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        StartButton.IsEnabled = !busy;
        StopButton.IsEnabled = !busy;
        RestartButton.IsEnabled = !busy;
        RepairButton.IsEnabled = !busy;
        if (!string.IsNullOrWhiteSpace(message)) FooterStatusText.Text = message;
    }

    private void ShowInfo(string title, string message, InfoBarSeverity severity)
    {
        GlobalInfoBar.Title = title;
        GlobalInfoBar.Message = message;
        GlobalInfoBar.Severity = severity;
        GlobalInfoBar.IsOpen = true;
    }

    private async Task RunActionAsync(string action, string label)
    {
        if (_busy) return;
        SetBusy(true, $"{label}...");
        try
        {
            var environment = new Dictionary<string, string?>
            {
                ["HANA_MANAGER_ACTION"] = action
            };
            _snapshot = await _service.RunAsync<ManagerSnapshot>(
                "action",
                environment,
                TimeSpan.FromSeconds(100));
            ApplySnapshot(_snapshot);
            ShowInfo(label, "操作已完成。", InfoBarSeverity.Success);
        }
        catch (Exception ex)
        {
            ShowInfo($"{label}失败", ex.Message, InfoBarSeverity.Error);
        }
        finally
        {
            SetBusy(false);
        }
    }

    private async void RefreshButton_Click(object sender, RoutedEventArgs e) =>
        await RefreshSnapshotAsync();

    private async void StartButton_Click(object sender, RoutedEventArgs e) =>
        await RunActionAsync("start", "启动服务");

    private async void StopButton_Click(object sender, RoutedEventArgs e) =>
        await RunActionAsync("stop", "停止服务");

    private async void RestartButton_Click(object sender, RoutedEventArgs e) =>
        await RunActionAsync("restart", "重启服务");

    private async void RepairButton_Click(object sender, RoutedEventArgs e) =>
        await RunActionAsync("repair", "检测并修复");

    private async void SettingsButton_Click(object sender, RoutedEventArgs e)
    {
        if (_busy) return;
        SetBusy(true, "正在打开设置...");
        try
        {
            await _service.OpenSettingsAsync();
            SetBusy(false);
            await RunActionAsync("repair", "应用设置");
        }
        catch (Exception ex)
        {
            ShowInfo("设置失败", ex.Message, InfoBarSeverity.Error);
        }
        finally
        {
            SetBusy(false);
        }
    }

    private void StatusPageButton_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.Local is not null)
        {
            BridgeCommandService.OpenExternal($"http://127.0.0.1:{_snapshot.Local.StatusPort}/");
        }
    }

    private void WebButton_Click(object sender, RoutedEventArgs e)
    {
        if (!string.IsNullOrWhiteSpace(_snapshot?.Cloud?.WebBaseUrl))
        {
            BridgeCommandService.OpenExternal($"{_snapshot.Cloud.WebBaseUrl.TrimEnd('/')}/desktop/");
        }
    }

    private void CopyReportButton_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot is null) return;
        var package = new DataPackage();
        package.SetText(JsonSerializer.Serialize(_snapshot, new JsonSerializerOptions { WriteIndented = true }));
        Clipboard.SetContent(package);
        ShowInfo("诊断报告", "安全诊断报告已复制。", InfoBarSeverity.Success);
    }

    private async void QueryDevicesButton_Click(object sender, RoutedEventArgs e) =>
        await QueryCloudDevicesAsync(claim: false);

    private async void ClaimDeviceButton_Click(object sender, RoutedEventArgs e) =>
        await QueryCloudDevicesAsync(claim: true);

    private async Task QueryCloudDevicesAsync(bool claim)
    {
        if (_busy) return;
        if (string.IsNullOrWhiteSpace(CloudBaseUrlBox.Text) || string.IsNullOrWhiteSpace(AccessKeyBox.Password))
        {
            ShowInfo("缺少登录信息", "请输入 Hana 网页地址和访问密钥。", InfoBarSeverity.Warning);
            return;
        }

        SetBusy(true, claim ? "正在登录并认领本机..." : "正在查询云端设备...");
        try
        {
            var environment = new Dictionary<string, string?>
            {
                ["HANA_MANAGER_BASE_URL"] = CloudBaseUrlBox.Text.Trim(),
                ["HANA_MANAGER_ACCESS_KEY"] = AccessKeyBox.Password,
                ["HANA_MANAGER_CLAIM"] = claim ? "1" : "0"
            };
            var result = await _service.RunAsync<CloudQueryResult>(
                "cloud-query",
                environment,
                TimeSpan.FromSeconds(45));
            CloudDevices.Clear();
            foreach (var device in result.Devices)
            {
                CloudDevices.Add(new CloudDeviceViewModel(device));
            }

            ShowInfo(
                claim ? "设备认领" : "云端设备",
                claim
                    ? (result.Claimed ? "当前电脑已认领。" : result.ClaimMessage)
                    : $"已查询到 {result.Devices.Count} 台电脑。",
                result.Claimed || !claim ? InfoBarSeverity.Success : InfoBarSeverity.Informational);
            SetBusy(false);
            await RefreshSnapshotAsync(showErrors: false);
        }
        catch (Exception ex)
        {
            ShowInfo("云端设备操作失败", ex.Message, InfoBarSeverity.Error);
        }
        finally
        {
            AccessKeyBox.Password = "";
            SetBusy(false);
        }
    }

    private async Task RefreshLogsAsync()
    {
        try
        {
            var selected = LogFileCombo.SelectedItem as LogFileItem;
            var result = await _service.RunAsync<LogListResult>("logs");
            LogFiles.Clear();
            foreach (var log in result.Logs)
            {
                LogFiles.Add(log);
            }

            if (LogFiles.Count == 0)
            {
                LogTextBox.Text = "暂无日志文件。";
                return;
            }

            var index = selected is null
                ? 0
                : Math.Max(0, LogFiles.ToList().FindIndex(item => item.FullName == selected.FullName));
            LogFileCombo.SelectedIndex = index;
        }
        catch (Exception ex)
        {
            LogTextBox.Text = $"读取日志失败：{ex.Message}";
        }
    }

    private async void RefreshLogsButton_Click(object sender, RoutedEventArgs e) =>
        await RefreshLogsAsync();

    private async void LogFileCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (LogFileCombo.SelectedItem is not LogFileItem item) return;
        try
        {
            var environment = new Dictionary<string, string?>
            {
                ["HANA_MANAGER_LOG_PATH"] = item.FullName
            };
            var result = await _service.RunAsync<LogTailResult>("log-tail", environment);
            LogTextBox.Text = result.Content;
        }
        catch (Exception ex)
        {
            LogTextBox.Text = $"读取日志失败：{ex.Message}";
        }
    }

    private void OpenLogsButton_Click(object sender, RoutedEventArgs e)
    {
        BridgeCommandService.OpenExternal(Path.Combine(_service.InstallRoot, "logs"));
    }

    private void ThemeToggle_Click(object sender, RoutedEventArgs e)
    {
        RootGrid.RequestedTheme = ThemeToggle.IsChecked == true
            ? ElementTheme.Dark
            : ElementTheme.Light;
    }
}
