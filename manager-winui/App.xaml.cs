using Microsoft.UI.Xaml;

namespace HanakoBridgeManager;

public partial class App : Application
{
    public App()
    {
        InitializeComponent();
        UnhandledException += App_UnhandledException;
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var window = new MainWindow();
        window.Activate();
    }

    private static void App_UnhandledException(object sender, Microsoft.UI.Xaml.UnhandledExceptionEventArgs e)
    {
        try
        {
            var root = Environment.GetCommandLineArgs()
                .SkipWhile(value => !value.Equals("--install-root", StringComparison.OrdinalIgnoreCase))
                .Skip(1)
                .FirstOrDefault();
            if (!string.IsNullOrWhiteSpace(root))
            {
                var logs = Path.Combine(root, "logs");
                Directory.CreateDirectory(logs);
                File.AppendAllText(
                    Path.Combine(logs, "manager-winui-crash.log"),
                    $"[{DateTimeOffset.Now:o}] {e.Exception}\r\n");
            }
        }
        catch { }
    }
}
