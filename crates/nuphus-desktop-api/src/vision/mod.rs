//! Vision perception module - screenshot, OCR, image/text/color location

use crate::core::*;
use crate::utils::cleanup::CleanupQueue;
use std::sync::Arc;
use uuid::Uuid;

pub mod locate;
pub mod models;
pub mod ocr;
pub mod paddle_ocr;
pub mod perceive;
pub mod yolo;

pub use locate::*;
pub use models::*;
pub use ocr::*;
pub use paddle_ocr::*;
pub use perceive::*;
pub use yolo::*;

/// Vision engine - perception layer core
pub struct VisionEngine {
    #[allow(dead_code)]
    cleanup: Arc<CleanupQueue>,
    ocr: OcrEngine,
    locate: Locator,
}

impl VisionEngine {
    pub fn new(cleanup: Arc<CleanupQueue>) -> Self {
        Self {
            cleanup,
            ocr: OcrEngine::new(),
            locate: Locator::new(),
        }
    }

    /// Perceive - screenshot and analyze according to the scope
    pub async fn see(
        &self,
        target: &Target,
        scope: Scope,
        what: PerceiveWhat,
    ) -> Result<Perception> {
        // 1. Screenshot
        let frame = self.capture(target, scope).await?;

        // 2. Analyze
        let mut perception = Perception {
            frame_id: frame.id,
            timestamp: frame.timestamp,
            scope,
            texts: vec![],
            elements: vec![],
            colors: vec![],
        };

        match what {
            PerceiveWhat::Text => {
                perception.texts = self.ocr.recognize(&frame).await?;
            }
            PerceiveWhat::Elements => {
                perception.elements = vec![];
            }
            PerceiveWhat::Colors => {
                perception.colors = self.locate.extract_colors(&frame)?;
            }
            PerceiveWhat::All => {
                perception.texts = self.ocr.recognize(&frame).await?;
                perception.elements = vec![];
                perception.colors = self.locate.extract_colors(&frame)?;
            }
        }

        Ok(perception)
    }

    /// Find - image/text/color
    pub async fn find(&self, frame: &Frame, query: &Query) -> Result<FindResult> {
        self.locate.find_with_fallback(frame, query).await
    }

    pub async fn capture(&self, target: &Target, scope: Scope) -> Result<Frame> {
        crate::capture::capture(target, scope).await
    }
}

/// Perception content type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerceiveWhat {
    Text,
    Elements,
    Colors,
    All,
}

/// Perception result
#[derive(Debug, Clone)]
pub struct Perception {
    pub frame_id: Uuid,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub scope: Scope,
    pub texts: Vec<TextBlock>,
    pub elements: Vec<Element>,
    pub colors: Vec<ColorSpot>,
}

/// Text block
#[derive(Debug, Clone)]
pub struct TextBlock {
    pub text: String,
    pub confidence: f32,
    pub rect: Rect,
}

/// UI element
#[derive(Debug, Clone)]
pub struct Element {
    pub kind: ElementKind,
    pub rect: Rect,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Button,
    Input,
    Text,
    Image,
    Icon,
    Unknown,
}

/// Color spot
#[derive(Debug, Clone)]
pub struct ColorSpot {
    pub color: Color,
    pub point: Point,
    pub area: u32,
}

// ───────────────────────────── color tolerance system ─────────────────────────────

/// Per-channel RGB color tolerance (DaMo-style "203040" = dr=32, dg=48, db=64)
#[derive(Debug, Clone, Copy)]
pub struct DeltaColor {
    pub dr: u8, // max deviation on the R channel
    pub dg: u8, // max deviation on the G channel
    pub db: u8, // max deviation on the B channel
}

impl DeltaColor {
    /// Construct from a single tolerance value (same value for all three channels)
    pub fn from_tolerance(tolerance: u8) -> Self {
        Self {
            dr: tolerance,
            dg: tolerance,
            db: tolerance,
        }
    }

    /// Construct from an RGB string, e.g. "203040" → dr=32, dg=48, db=64
    /// or "10a0b0" → dr=0x10, dg=0xa0, db=0xb0
    pub fn from_hex_str(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 6 {
            return None;
        }
        let dr = u8::from_str_radix(&s[0..2], 16).ok()?;
        let dg = u8::from_str_radix(&s[2..4], 16).ok()?;
        let db = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self { dr, dg, db })
    }

    /// Check whether a color is within the tolerance range
    pub fn matches(&self, actual: &Color, target: &Color) -> bool {
        actual.r.abs_diff(target.r) <= self.dr
            && actual.g.abs_diff(target.g) <= self.dg
            && actual.b.abs_diff(target.b) <= self.db
    }
}

/// Scan direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanDirection {
    /// Top-left → bottom-right (default)
    #[default]
    LeftTop,
    /// Top-right → bottom-left
    RightTop,
    /// Bottom-left → top-right
    LeftBottom,
    /// Bottom-right → top-left
    RightBottom,
}

/// Find query
#[derive(Debug, Clone)]
pub enum Query {
    /// Find image - template matching
    Image(Vec<u8>), // PNG bytes
    /// Find text - OCR keyword
    Text(String),
    /// Find color - color matching
    Color { target: Color, tolerance: u8 },
    /// Combined: find text first, then verify color
    TextThenColor {
        text: String,
        color: Color,
        tolerance: u8,
    },
    /// Combined: find image first, then verify text
    ImageThenText { image: Vec<u8>, text: String },
}

/// Find result
#[derive(Debug, Clone)]
pub struct FindResult {
    pub found: bool,
    pub rect: Option<Rect>,
    pub confidence: f32,
    pub method: FindMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindMethod {
    ImageMatch,
    TextOcr,
    ColorScan,
    Combined,
    Fallback,
}
