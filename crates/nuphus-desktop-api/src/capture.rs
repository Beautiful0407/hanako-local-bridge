//! Screenshot implementation - xcap + custom cropping + graphics backend dispatch

use crate::core::*;
use xcap::{Monitor, Window as XcapWindow};

/// Capture a screenshot - according to the target and scope
pub async fn capture(target: &Target, scope: Scope) -> Result<Frame> {
    match scope {
        Scope::Fullscreen => capture_fullscreen().await,
        Scope::Window => capture_window(target).await,
        Scope::ClientArea => capture_client_area(target).await,
        Scope::Element { x, y, w, h } => capture_region(x, y, w, h).await,
        Scope::Point { x, y, radius } => {
            let size = radius * 2;
            capture_region(x - radius as i32, y - radius as i32, size, size).await
        }
    }
}

/// Full-screen capture
async fn capture_fullscreen() -> Result<Frame> {
    let monitors = Monitor::all().map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    let primary = monitors
        .into_iter()
        .next()
        .ok_or_else(|| DesktopError::CaptureFailed("no monitor found".to_string()))?;

    let image = primary
        .capture_image()
        .map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    convert_to_frame(image, Scope::Fullscreen, FrameSource::Screenshot)
}

/// Window capture - dispatch strategy by graphics backend
async fn capture_window(target: &Target) -> Result<Frame> {
    #[cfg(not(windows))]
    let _ = target; // Target::Window only exists on Windows; fall through below.
    #[cfg(windows)]
    {
        if let Target::Window {
            hwnd, gfx_backend, ..
        } = target
        {
            return capture_window_by_backend(*hwnd, *gfx_backend).await;
        }
    }

    // Fallback: full-screen capture (all platforms). Target::Window only exists
    // on Windows, so a non-Windows window capture always lands here.
    capture_fullscreen().await
}

/// Dispatch the capture strategy by graphics backend
async fn capture_window_by_backend(hwnd: isize, gfx: GfxBackend) -> Result<Frame> {
    match gfx {
        GfxBackend::Gdi => capture_window_gdi(hwnd).await,
        GfxBackend::DirectX | GfxBackend::Unknown => {
            // Try GDI first, fall back to full-screen + crop on failure
            match capture_window_gdi(hwnd).await {
                Ok(frame) => Ok(frame),
                Err(_) => capture_fullscreen_and_crop(hwnd).await,
            }
        }
        GfxBackend::OpenGl | GfxBackend::Vulkan => {
            // OGL/Vulkan windows capture black screens via GDI, go straight to full-screen + crop
            capture_fullscreen_and_crop(hwnd).await
        }
    }
}

/// GDI window capture (xcap)
async fn capture_window_gdi(hwnd: isize) -> Result<Frame> {
    let windows = XcapWindow::all().map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    let win = windows
        .into_iter()
        .find(|w| w.id().ok().map(|id| id as isize) == Some(hwnd))
        .ok_or_else(|| DesktopError::CaptureFailed(format!("window {} not found", hwnd)))?;

    let image = win
        .capture_image()
        .map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    convert_to_frame(image, Scope::Window, FrameSource::WindowCapture)
}

/// Full-screen capture + crop to the window position (fallback strategy)
async fn capture_fullscreen_and_crop(hwnd: isize) -> Result<Frame> {
    let frame = capture_fullscreen().await?;

    #[cfg(windows)]
    {
        use ::windows::Win32::Foundation::{HWND, RECT};
        use ::windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(HWND(hwnd), &mut rect) };

        let x = rect.left.max(0) as u32;
        let y = rect.top.max(0) as u32;
        let w = (rect.right - rect.left) as u32;
        let h = (rect.bottom - rect.top) as u32;

        frame
            .crop(x, y, w, h)
            .ok_or_else(|| DesktopError::CaptureFailed("fullscreen crop failed".to_string()))
    }

    #[cfg(not(windows))]
    {
        // macOS/Linux: use xcap to get the window position for cropping
        let windows = XcapWindow::all().map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
        let win = windows
            .into_iter()
            .find(|w| w.id().ok().map(|id| id as isize) == Some(hwnd))
            .ok_or_else(|| DesktopError::CaptureFailed(format!("window {} not found", hwnd)))?;

        // xcap 0.9 returns Result from x()/y()/width()/height() — default to 0
        // on error so a stale window handle degrades to a full-screen crop.
        let x = win.x().unwrap_or(0).max(0) as u32;
        let y = win.y().unwrap_or(0).max(0) as u32;
        let w = win.width().unwrap_or(0);
        let h = win.height().unwrap_or(0);

        frame
            .crop(x, y, w, h)
            .ok_or_else(|| DesktopError::CaptureFailed("fullscreen crop failed".to_string()))
    }
}

/// Client area capture (without the title bar border)
async fn capture_client_area(target: &Target) -> Result<Frame> {
    #[cfg(windows)]
    {
        use ::windows::Win32::Foundation::{HWND, POINT, RECT};
        use ::windows::Win32::Graphics::Gdi::ClientToScreen;
        use ::windows::Win32::UI::WindowsAndMessaging::GetClientRect;

        if let Target::Window { hwnd, .. } = target {
            let hwnd = HWND(*hwnd);
            let mut client_rect = RECT::default();
            let mut point = POINT { x: 0, y: 0 };

            unsafe {
                let _ = GetClientRect(hwnd, &mut client_rect);
                let _ = ClientToScreen(hwnd, &mut point);
            }

            let x = point.x;
            let y = point.y;
            let w = (client_rect.right - client_rect.left) as u32;
            let h = (client_rect.bottom - client_rect.top) as u32;

            return capture_region(x, y, w, h).await;
        }
    }

    // Fallback
    capture_window(target).await
}

/// Region capture
async fn capture_region(x: i32, y: i32, w: u32, h: u32) -> Result<Frame> {
    let monitors = Monitor::all().map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    let primary = monitors
        .into_iter()
        .next()
        .ok_or_else(|| DesktopError::CaptureFailed("no monitor".to_string()))?;

    let image = primary
        .capture_image()
        .map_err(|e| DesktopError::CaptureFailed(e.to_string()))?;
    let frame = convert_to_frame(image, Scope::Fullscreen, FrameSource::Screenshot)?;

    let x = x.max(0) as u32;
    let y = y.max(0) as u32;

    // Out-of-bounds guard: a region starting beyond the frame edge would make
    // `frame.width - x` / `frame.height - y` underflow (u32) and panic in debug
    // builds. Fail with a clear error instead of crashing the server.
    if x >= frame.width || y >= frame.height {
        return Err(DesktopError::CaptureFailed(format!(
            "capture region out of bounds: x={x}, y={y}, screen={}x{}",
            frame.width, frame.height
        )));
    }

    let w = w.min(frame.width - x);
    let h = h.min(frame.height - y);

    frame
        .crop(x, y, w, h)
        .ok_or_else(|| DesktopError::CaptureFailed("crop failed".to_string()))
}

/// Convert an xcap image to a Frame
fn convert_to_frame(
    image: xcap::image::RgbaImage,
    scope: Scope,
    source: FrameSource,
) -> Result<Frame> {
    let width = image.width();
    let height = image.height();
    let pixels = image.into_raw();

    Ok(Frame {
        id: uuid::Uuid::new_v4(),
        pixels,
        width,
        height,
        scope,
        timestamp: chrono::Utc::now(),
        source,
    })
}
