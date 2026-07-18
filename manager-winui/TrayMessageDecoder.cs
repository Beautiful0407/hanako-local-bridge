using System;

namespace HanakoBridgeManager
{
    public static class TrayMessageDecoder
    {
        public static uint GetNotificationCode(IntPtr lParam)
        {
            return unchecked((uint)lParam.ToInt64()) & 0xFFFF;
        }
    }
}
