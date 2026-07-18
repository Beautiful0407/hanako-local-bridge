using Microsoft.UI;
using Microsoft.UI.Xaml.Media;
using System.Collections.ObjectModel;
using System.Text.Json.Serialization;

namespace HanakoBridgeManager;

public sealed class ManagerSnapshot
{
    public string CapturedAt { get; set; } = "";
    public string Overall { get; set; } = "error";
    public string Recommendation { get; set; } = "";
    public string InstallRoot { get; set; } = "";
    public string ConfigPath { get; set; } = "";
    public string Version { get; set; } = "unknown";
    public DeviceSnapshot? Device { get; set; }
    public LocalSnapshot? Local { get; set; }
    public CloudSnapshot? Cloud { get; set; }
    public IdentitySnapshot? Identity { get; set; }
    public TaskSnapshot? Tasks { get; set; }
    public List<ProcessSnapshot> Processes { get; set; } = [];
    public List<CheckSnapshot> Checks { get; set; } = [];
}

public sealed class DeviceSnapshot
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Hostname { get; set; } = "";
}

public sealed class LocalSnapshot
{
    public int McpPort { get; set; }
    public int StatusPort { get; set; }
    public bool McpHealthy { get; set; }
    public bool StatusHealthy { get; set; }
    public string McpError { get; set; } = "";
    public string StatusError { get; set; } = "";
    public string TrustMode { get; set; } = "";
}

public sealed class CloudSnapshot
{
    public bool Enabled { get; set; }
    public string Url { get; set; } = "";
    public string WebBaseUrl { get; set; } = "";
    public string Status { get; set; } = "offline";
    public string LastConnectedAt { get; set; } = "";
    public string LastSeenAt { get; set; } = "";
    public string LastError { get; set; } = "";
}

public sealed class IdentitySnapshot
{
    public string Path { get; set; } = "";
    public bool CredentialPresent { get; set; }
    public bool ClaimTokenPresent { get; set; }
    public string PublicKeyFingerprint { get; set; } = "";
    public string UpdatedAt { get; set; } = "";
}

public sealed class TaskSnapshot
{
    public string McpName { get; set; } = "";
    public string McpState { get; set; } = "Missing";
    public string TunnelName { get; set; } = "";
    public string TunnelState { get; set; } = "Missing";
    public bool HiddenLauncher { get; set; }
    public string McpAction { get; set; } = "";
}

public sealed class ProcessSnapshot
{
    public int ProcessId { get; set; }
    public string Name { get; set; } = "";
    public int ParentProcessId { get; set; }
    public string CommandLine { get; set; } = "";
}

public sealed class CheckSnapshot
{
    public string Code { get; set; } = "";
    public string Status { get; set; } = "error";
    public string Detail { get; set; } = "";
}

public sealed class CloudQueryResult
{
    public bool Claimed { get; set; }
    public string ClaimMessage { get; set; } = "";
    public List<CloudDevice> Devices { get; set; } = [];
    public string BaseUrl { get; set; } = "";
}

public sealed class CloudDevice
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Version { get; set; } = "";
    public string Status { get; set; } = "";
    public string LastSeenAt { get; set; } = "";
}

public sealed class LogListResult
{
    public List<LogFileItem> Logs { get; set; } = [];
}

public sealed class LogTailResult
{
    public string Content { get; set; } = "";
}

public sealed class UpdateStatus
{
    public string CurrentVersion { get; set; } = "unknown";
    public string LatestVersion { get; set; } = "unknown";
    public bool UpdateAvailable { get; set; }
    public string Manifest { get; set; } = "";
    public string PackageUrl { get; set; } = "";
    public string PublishedAt { get; set; } = "";
    public string Notes { get; set; } = "";
    public bool SignatureVerified { get; set; }
}

public sealed class LogFileItem
{
    public string Name { get; set; } = "";
    public string FullName { get; set; } = "";
    public long Length { get; set; }
    public string LastWriteTime { get; set; } = "";
}

public sealed class DiagnosticItemViewModel
{
    private static readonly IReadOnlyDictionary<string, string> Names =
        new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["config"] = "配置文件",
            ["package"] = "程序文件",
            ["mcp_task"] = "后台计划任务",
            ["hidden_launcher"] = "无感启动",
            ["mcp_process"] = "MCP 进程",
            ["mcp_health"] = "本地 MCP",
            ["status_health"] = "状态服务",
            ["cloud"] = "云端 WebSocket",
            ["identity"] = "设备凭证",
            ["tunnel"] = "旧 SSH 隧道"
        };

    public DiagnosticItemViewModel(CheckSnapshot check)
    {
        Name = Names.TryGetValue(check.Code, out var value) ? value : check.Code;
        Status = StatusText(check.Status);
        Detail = check.Detail;
        StatusBrush = StatusColor(check.Status);
    }

    public string Name { get; }
    public string Status { get; }
    public string Detail { get; }
    public Brush StatusBrush { get; }

    public static string StatusText(string status) => status switch
    {
        "pass" or "healthy" or "active" or "online" => "正常",
        "warning" or "pending_claim" or "pending" => "需要处理",
        "offline" => "离线",
        "disabled" => "已停用",
        _ => "异常"
    };

    public static SolidColorBrush StatusColor(string status) => status switch
    {
        "pass" or "healthy" or "active" or "online" => new SolidColorBrush(ColorHelper.FromArgb(255, 22, 138, 78)),
        "warning" or "pending_claim" or "pending" => new SolidColorBrush(ColorHelper.FromArgb(255, 190, 112, 0)),
        _ => new SolidColorBrush(ColorHelper.FromArgb(255, 196, 43, 43))
    };
}

public sealed class CloudDeviceViewModel
{
    public CloudDeviceViewModel(CloudDevice device)
    {
        Id = device.Id;
        Name = device.Name;
        Version = device.Version;
        Status = DiagnosticItemViewModel.StatusText(device.Status);
        StatusBrush = DiagnosticItemViewModel.StatusColor(device.Status);
        LastSeen = MainWindow.FormatTime(device.LastSeenAt);
    }

    public string Id { get; }
    public string Name { get; }
    public string Version { get; }
    public string Status { get; }
    public Brush StatusBrush { get; }
    public string LastSeen { get; }
}
