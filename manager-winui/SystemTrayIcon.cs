using System.Runtime.InteropServices;

namespace HanakoBridgeManager;

internal sealed class SystemTrayIcon : IDisposable
{
    private const int GwlWndProc = -4;
    private const uint NifMessage = 0x00000001;
    private const uint NifIcon = 0x00000002;
    private const uint NifTip = 0x00000004;
    private const uint NimAdd = 0x00000000;
    private const uint NimDelete = 0x00000002;
    private const uint NimSetVersion = 0x00000004;
    private const uint NotifyIconVersion = 4;
    private const uint WmApp = 0x8000;
    private const uint WmLButtonUp = 0x0202;
    private const uint WmLButtonDblClk = 0x0203;
    private const uint WmRButtonUp = 0x0205;
    private const uint WmSize = 0x0005;
    private const uint SizeMinimized = 1;
    private const uint WmNull = 0x0000;
    private const uint TpmRightButton = 0x0002;
    private const uint TpmReturnCmd = 0x0100;
    private const uint MfString = 0x00000000;
    private const uint MfSeparator = 0x00000800;
    private const uint IdiApplication = 32512;
    private const uint OpenCommand = 1001;
    private const uint ExitCommand = 1002;

    private readonly IntPtr _windowHandle;
    private readonly Action _restoreWindow;
    private readonly Action _exitApplication;
    private readonly Action _minimizeToTray;
    private readonly uint _callbackMessage = WmApp + 0x45;
    private readonly WindowProc _windowProc;
    private IntPtr _originalWindowProc;
    private bool _disposed;

    public SystemTrayIcon(
        IntPtr windowHandle,
        Action restoreWindow,
        Action exitApplication,
        Action minimizeToTray)
    {
        _windowHandle = windowHandle;
        _restoreWindow = restoreWindow;
        _exitApplication = exitApplication;
        _minimizeToTray = minimizeToTray;
        _windowProc = WindowProcHandler;

        _originalWindowProc = SetWindowLongPtr(_windowHandle, GwlWndProc, _windowProc);
        if (_originalWindowProc == IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"Unable to install the manager window procedure. Win32 error {Marshal.GetLastWin32Error()}.");
        }

        var data = CreateNotifyIconData();
        if (!Shell_NotifyIcon(NimAdd, ref data))
        {
            RestoreWindowProcedure();
            throw new InvalidOperationException(
                $"Unable to create the manager system tray icon. Win32 error {Marshal.GetLastWin32Error()}.");
        }

        data.uVersion = NotifyIconVersion;
        Shell_NotifyIcon(NimSetVersion, ref data);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        var data = CreateNotifyIconData();
        Shell_NotifyIcon(NimDelete, ref data);
        RestoreWindowProcedure();
        GC.KeepAlive(_windowProc);
    }

    private void RestoreWindowProcedure()
    {
        if (_originalWindowProc == IntPtr.Zero) return;
        SetWindowLongPtr(_windowHandle, GwlWndProc, _originalWindowProc);
        _originalWindowProc = IntPtr.Zero;
    }

    private NotifyIconData CreateNotifyIconData() =>
        new()
        {
            cbSize = (uint)Marshal.SizeOf<NotifyIconData>(),
            hWnd = _windowHandle,
            uID = 1,
            uFlags = NifMessage | NifIcon | NifTip,
            uCallbackMessage = _callbackMessage,
            hIcon = LoadIcon(IntPtr.Zero, (IntPtr)IdiApplication),
            szTip = "Hanako Local Bridge"
        };

    private IntPtr WindowProcHandler(
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam)
    {
        if (message == WmSize && unchecked((uint)wParam.ToInt64()) == SizeMinimized)
        {
            _minimizeToTray();
            return IntPtr.Zero;
        }

        if (message == _callbackMessage)
        {
            var trayMessage = unchecked((uint)lParam.ToInt64());
            if (trayMessage is WmLButtonUp or WmLButtonDblClk)
            {
                _restoreWindow();
                return IntPtr.Zero;
            }

            if (trayMessage == WmRButtonUp)
            {
                ShowContextMenu();
                return IntPtr.Zero;
            }
        }

        return CallWindowProc(_originalWindowProc, windowHandle, message, wParam, lParam);
    }

    private void ShowContextMenu()
    {
        if (!GetCursorPos(out var cursor)) return;

        var menu = CreatePopupMenu();
        if (menu == IntPtr.Zero) return;

        try
        {
            AppendMenu(menu, MfString, OpenCommand, "打开管理器");
            AppendMenu(menu, MfSeparator, 0, null);
            AppendMenu(menu, MfString, ExitCommand, "退出管理器");

            SetForegroundWindow(_windowHandle);
            var command = TrackPopupMenuEx(
                menu,
                TpmReturnCmd | TpmRightButton,
                cursor.X,
                cursor.Y,
                _windowHandle,
                IntPtr.Zero);

            PostMessage(_windowHandle, WmNull, IntPtr.Zero, IntPtr.Zero);
            if (command == OpenCommand)
            {
                _restoreWindow();
            }
            else if (command == ExitCommand)
            {
                _exitApplication();
            }
        }
        finally
        {
            DestroyMenu(menu);
        }
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData
    {
        public uint cbSize;
        public IntPtr hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
        public string szTip;

        public uint dwState;
        public uint dwStateMask;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string szInfo;

        public uint uVersion;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
        public string szInfoTitle;

        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Point
    {
        public int X;
        public int Y;
    }

    private delegate IntPtr WindowProc(
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool Shell_NotifyIcon(uint message, ref NotifyIconData data);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadIcon(IntPtr instance, IntPtr iconName);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr SetWindowLongPtr(
        IntPtr windowHandle,
        int index,
        WindowProc newWindowProc);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr SetWindowLongPtr(
        IntPtr windowHandle,
        int index,
        IntPtr newWindowProc);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr CallWindowProc(
        IntPtr previousWindowProc,
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out Point point);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr CreatePopupMenu();

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AppendMenu(
        IntPtr menu,
        uint flags,
        uint identifier,
        string? newItem);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint TrackPopupMenuEx(
        IntPtr menu,
        uint flags,
        int x,
        int y,
        IntPtr ownerWindow,
        IntPtr reserved);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyMenu(IntPtr menu);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetForegroundWindow(IntPtr windowHandle);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessage(
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam);
}
