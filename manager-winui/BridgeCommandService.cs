using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace HanakoBridgeManager;

public sealed class BridgeCommandService
{
    private readonly string _installRoot;
    private readonly string _commandScript;
    private readonly JsonSerializerOptions _jsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    public BridgeCommandService(string installRoot)
    {
        _installRoot = Path.GetFullPath(installRoot);
        _commandScript = Path.Combine(_installRoot, "manager-command.ps1");
    }

    public string InstallRoot => _installRoot;

    public async Task<T> RunAsync<T>(
        string operation,
        IReadOnlyDictionary<string, string?>? environment = null,
        TimeSpan? timeout = null,
        CancellationToken cancellationToken = default)
    {
        if (!File.Exists(_commandScript))
        {
            throw new FileNotFoundException("管理命令脚本不存在。", _commandScript);
        }

        var windows = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        var powershell = Path.Combine(windows, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
        var startInfo = new ProcessStartInfo
        {
            FileName = powershell,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = new UTF8Encoding(false),
            StandardErrorEncoding = new UTF8Encoding(false)
        };
        startInfo.ArgumentList.Add("-NoLogo");
        startInfo.ArgumentList.Add("-NoProfile");
        startInfo.ArgumentList.Add("-NonInteractive");
        startInfo.ArgumentList.Add("-ExecutionPolicy");
        startInfo.ArgumentList.Add("Bypass");
        startInfo.ArgumentList.Add("-File");
        startInfo.ArgumentList.Add(_commandScript);
        startInfo.ArgumentList.Add("-Operation");
        startInfo.ArgumentList.Add(operation);
        startInfo.ArgumentList.Add("-InstallRoot");
        startInfo.ArgumentList.Add(_installRoot);

        if (environment is not null)
        {
            foreach (var entry in environment)
            {
                startInfo.Environment[entry.Key] = entry.Value ?? string.Empty;
            }
        }

        using var process = new Process { StartInfo = startInfo };
        if (!process.Start())
        {
            throw new InvalidOperationException("无法启动本地管理命令。");
        }

        var stdoutTask = process.StandardOutput.ReadToEndAsync(cancellationToken);
        var stderrTask = process.StandardError.ReadToEndAsync(cancellationToken);
        using var timeoutSource = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutSource.CancelAfter(timeout ?? TimeSpan.FromSeconds(75));

        try
        {
            await process.WaitForExitAsync(timeoutSource.Token);
        }
        catch (OperationCanceledException)
        {
            try { process.Kill(true); } catch { }
            throw new TimeoutException("本地管理操作超时。");
        }

        var stdout = (await stdoutTask).Trim();
        var stderr = (await stderrTask).Trim();
        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(stderr)
                ? $"管理命令失败，退出代码 {process.ExitCode}。"
                : stderr);
        }
        if (string.IsNullOrWhiteSpace(stdout))
        {
            throw new InvalidOperationException("管理命令没有返回数据。");
        }

        return DeserializeCommandOutput<T>(stdout);
    }

    private T DeserializeCommandOutput<T>(string stdout)
    {
        try
        {
            return JsonSerializer.Deserialize<T>(stdout, _jsonOptions)
                ?? throw new JsonException("管理命令返回了空 JSON。");
        }
        catch (JsonException originalError)
        {
            var lines = stdout.Split(
                ["\r\n", "\n", "\r"],
                StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
            for (var index = lines.Length - 1; index >= 0; index--)
            {
                var candidate = lines[index];
                var looksLikeJson =
                    candidate.StartsWith('{') && candidate.EndsWith('}') ||
                    candidate.StartsWith('[') && candidate.EndsWith(']');
                if (!looksLikeJson) continue;

                try
                {
                    var value = JsonSerializer.Deserialize<T>(candidate, _jsonOptions);
                    if (value is not null) return value;
                }
                catch (JsonException)
                {
                    // Keep looking for the final complete JSON value.
                }
            }

            throw new InvalidOperationException(
                "管理命令返回了无法识别的数据，请重新检测或查看日志。",
                originalError);
        }
    }

    public async Task OpenSettingsAsync(CancellationToken cancellationToken = default)
    {
        var script = Path.Combine(_installRoot, "configuration-ui.ps1");
        if (!File.Exists(script))
        {
            throw new FileNotFoundException("设置界面不存在。", script);
        }

        var windows = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        var powershell = Path.Combine(windows, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
        var startInfo = new ProcessStartInfo
        {
            FileName = powershell,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        startInfo.ArgumentList.Add("-NoLogo");
        startInfo.ArgumentList.Add("-NoProfile");
        startInfo.ArgumentList.Add("-ExecutionPolicy");
        startInfo.ArgumentList.Add("Bypass");
        startInfo.ArgumentList.Add("-WindowStyle");
        startInfo.ArgumentList.Add("Hidden");
        startInfo.ArgumentList.Add("-File");
        startInfo.ArgumentList.Add(script);
        startInfo.ArgumentList.Add("-InstallRoot");
        startInfo.ArgumentList.Add(_installRoot);
        startInfo.ArgumentList.Add("-ConfigPath");
        startInfo.ArgumentList.Add(Path.Combine(_installRoot, "config.json"));

        using var process = Process.Start(startInfo)
            ?? throw new InvalidOperationException("无法打开设置界面。");
        await process.WaitForExitAsync(cancellationToken);
        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException("设置界面异常退出。");
        }
    }

    public static void OpenExternal(string target)
    {
        Process.Start(new ProcessStartInfo
        {
            FileName = target,
            UseShellExecute = true
        });
    }
}
