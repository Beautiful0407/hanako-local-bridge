//! Core type system - Session, Target, Frame, Scope, AppKind

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "http-server")]
use std::sync::Arc;
#[cfg(feature = "http-server")]
use tokio::sync::RwLock;
use uuid::Uuid;

// ─────────────────────────────── application types ───────────────────────────────

#[cfg(feature = "http-server")]
/// Application type - determines the interaction strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppKind {
    /// Win32/UWP desktop application - needs window activation, SendInput
    Desktop,
    /// Browser page - Playwright/CDP operations
    Browser,
    /// Terminal/TUI application - special character encoding
    Tui,
}

// ─────────────────────────────── target specs ───────────────────────────────

#[cfg(feature = "http-server")]
/// Target specified when creating a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSpec {
    /// Find by window title
    Title(String),
    /// Find by window handle (Windows)
    Hwnd(isize),
    /// Find by process name
    Process(String),
    /// Browser URL
    Url(String),
    /// Playwright-connected page
    PlaywrightPage {
        ws_url: String,
        token: Option<String>,
    },
}

// ─────────────────────────────── sessions ───────────────────────────────

#[cfg(feature = "http-server")]
/// Session handle - entrypoint for all operations
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: Uuid,
    pub kind: AppKind,
    pub target: Arc<RwLock<Target>>,
    pub viewport: Arc<RwLock<Viewport>>,
    pub created_at: DateTime<Utc>,
    pub last_activity: Arc<RwLock<DateTime<Utc>>>,
}

#[cfg(feature = "http-server")]
impl SessionHandle {
    /// Update the activity time
    pub async fn touch(&self) {
        let mut last = self.last_activity.write().await;
        *last = Utc::now();
    }
}

// ─────────────────────────────── graphics backends ───────────────────────────────

/// Window graphics rendering backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfxBackend {
    Unknown,
    Gdi,
    DirectX,
    OpenGl,
    Vulkan,
}

// ─────────────────────────────── targets ───────────────────────────────

/// Runtime target - contains platform handles
#[derive(Debug)]
pub enum Target {
    /// Windows window
    #[cfg(windows)]
    Window {
        hwnd: isize,
        title: String,
        verified: bool,
        gfx_backend: GfxBackend,
    },
    /// Browser page
    Browser { page_id: String, url: String },
    /// Terminal
    Tui { hwnd: isize, title: String },
}

impl Target {
    /// Get the window title (for identification)
    pub fn title(&self) -> &str {
        match self {
            #[cfg(windows)]
            Target::Window { title, .. } => title,
            Target::Browser { url, .. } => url,
            Target::Tui { title, .. } => title,
        }
    }

    /// Whether activation has been verified
    pub fn is_verified(&self) -> bool {
        match self {
            #[cfg(windows)]
            Target::Window { verified, .. } => *verified,
            Target::Browser { .. } => true, // browser does not need the foreground
            Target::Tui { .. } => false,
        }
    }

    /// Mark as verified
    pub fn verify(&mut self) {
        match self {
            #[cfg(windows)]
            Target::Window { verified, .. } => *verified = true,
            Target::Browser { .. } => {}
            Target::Tui { .. } => {}
        }
    }
}

// ─────────────────────────────── viewports ───────────────────────────────

#[cfg(feature = "http-server")]
/// Current visible area
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

#[cfg(feature = "http-server")]
impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
        }
    }
}

// ─────────────────────────────── frames ───────────────────────────────

/// Screen frame - in memory, never written to disk
#[derive(Debug)]
pub struct Frame {
    pub id: Uuid,
    pub pixels: Vec<u8>, // RGBA
    pub width: u32,
    pub height: u32,
    pub scope: Scope,
    pub timestamp: DateTime<Utc>,
    pub source: FrameSource,
}

/// Frame source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameSource {
    Screenshot,
    WindowCapture,
    BrowserCapture,
    RegionCrop,
}

impl Frame {
    /// Compute approximate memory usage (KB)
    pub fn memory_kb(&self) -> f64 {
        (self.pixels.len() as f64) / 1024.0
    }

    /// Estimate PNG size (KB)
    pub fn estimated_png_kb(&self) -> f64 {
        // Rough estimate: compression ratio about 30-50%
        self.memory_kb() * 0.4
    }

    /// Crop region
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Option<Self> {
        if x + w > self.width || y + h > self.height {
            return None;
        }

        let mut cropped = Vec::with_capacity((w * h * 4) as usize);
        for row in y..(y + h) {
            let start = ((row * self.width + x) * 4) as usize;
            let end = start + (w * 4) as usize;
            cropped.extend_from_slice(&self.pixels[start..end]);
        }

        Some(Self {
            id: Uuid::new_v4(),
            pixels: cropped,
            width: w,
            height: h,
            scope: Scope::Element {
                x: x as i32,
                y: y as i32,
                w,
                h,
            },
            timestamp: Utc::now(),
            source: FrameSource::RegionCrop,
        })
    }

    /// Get the pixel color
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        Some(Color {
            r: self.pixels[idx],
            g: self.pixels[idx + 1],
            b: self.pixels[idx + 2],
            a: self.pixels[idx + 3],
        })
    }
}

// ─────────────────────────────── screenshot scopes ───────────────────────────────

/// Screenshot scope - determines perception granularity
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Scope {
    /// Full screen - rarely used, must be specified explicitly
    Fullscreen,
    /// Current window - default
    #[default]
    Window,
    /// Window client area (without title bar border)
    ClientArea,
    /// Element-level - recommended
    Element { x: i32, y: i32, w: u32, h: u32 },
    /// Area around a point - most economical
    Point { x: i32, y: i32, radius: u32 },
}

// ─────────────────────────────── colors ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Color distance (0-441.7)
    pub fn distance(&self, other: &Color) -> f32 {
        let dr = (self.r as f32 - other.r as f32).abs();
        let dg = (self.g as f32 - other.g as f32).abs();
        let db = (self.b as f32 - other.b as f32).abs();
        (dr * dr + dg * dg + db * db).sqrt()
    }

    /// Whether it is within tolerance
    pub fn matches(&self, target: &Color, tolerance: u8) -> bool {
        self.distance(target) <= (tolerance as f32 * 3.0_f32.sqrt())
    }
}

// ─────────────────────────────── geometry ───────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn center(&self) -> Point {
        Point {
            x: self.x + (self.w as i32) / 2,
            y: self.y + (self.h as i32) / 2,
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x
            && p.x < self.x + self.w as i32
            && p.y >= self.y
            && p.y < self.y + self.h as i32
    }
}

// ─────────────────────────────── session management ───────────────────────────────

#[cfg(feature = "http-server")]
use std::collections::HashMap;

#[cfg(feature = "http-server")]
pub struct SessionManager {
    sessions: HashMap<Uuid, SessionHandle>,
}

#[cfg(feature = "http-server")]
impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub async fn create(&mut self, spec: TargetSpec) -> Result<SessionHandle> {
        let id = Uuid::new_v4();
        let now = Utc::now();

        // TODO: parse the target type from the spec
        let kind = AppKind::Desktop;
        let target = Arc::new(RwLock::new(Target::Tui {
            hwnd: 0,
            title: "placeholder".to_string(),
        }));

        let handle = SessionHandle {
            id,
            kind,
            target,
            viewport: Arc::new(RwLock::new(Viewport::default())),
            created_at: now,
            last_activity: Arc::new(RwLock::new(now)),
        };

        self.sessions.insert(id, handle.clone());
        Ok(handle)
    }

    pub fn get(&self, id: Uuid) -> Option<SessionHandle> {
        self.sessions.get(&id).cloned()
    }

    pub async fn close(&mut self, id: Uuid) -> Result<()> {
        self.sessions.remove(&id);
        Ok(())
    }
}

// ─────────────────────────────── errors ───────────────────────────────

pub type Result<T> = std::result::Result<T, DesktopError>;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("Target not found: {0}")]
    TargetNotFound(String),
    #[error("Window activation failed: {0}")]
    ActivationFailed(String),
    #[error("Screenshot failed: {0}")]
    CaptureFailed(String),
    #[error("OCR failed: {0}")]
    OcrFailed(String),
    #[error("Input sending failed: {0}")]
    InputFailed(String),
    #[error("Locate failed: {0}")]
    LocateFailed(String),
    #[error("All strategies exhausted")]
    AllStrategiesFailed,
    #[error("Session not found: {0}")]
    SessionNotFound(Uuid),
    #[error("Platform not supported")]
    PlatformNotSupported,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
