//! Windows platform-specific implementation
//!
//! Only `detect_gfx_backend` lives here; window enumeration/search callbacks are
//! private to `platform/mod.rs` (duplicate dead copies were removed in batch B).

use crate::core::*;

/// Detect the window graphics backend
pub fn detect_gfx_backend(hwnd: isize) -> GfxBackend {
    use ::windows::Win32::Foundation::HWND;
    use ::windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(HWND(hwnd), &mut buf) };
    if len == 0 {
        return GfxBackend::Unknown;
    }

    let class = String::from_utf16_lossy(&buf[..len as usize]);
    match class.as_str() {
        "Chrome_WidgetWin_1" | "Chrome_WidgetWin_2" | "CefBrowserWindow" => GfxBackend::DirectX,
        "Qt5QWindowIcon" | "Qt6QWindowIcon" => GfxBackend::OpenGl,
        "ConsoleWindowClass" | "#32770" => GfxBackend::Gdi,
        // Unknown classes must NOT default to GDI: for hardware-accelerated
        // windows GDI returns a black image *successfully*, which suppresses the
        // fullscreen+crop fallback. `Unknown` routes through the try-GDI-then-
        // fallback path in vision::capture instead.
        _ => GfxBackend::Unknown,
    }
}
