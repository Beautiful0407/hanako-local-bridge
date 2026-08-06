//! OCR implementation - pending replacement by the PaddleOCR integration
//!
//! Currently an empty-shell (stub) implementation, waiting for ONNX Runtime + PaddleOCR integration.

use crate::core::*;
use crate::vision::TextBlock;

/// Streaming OCR result
#[derive(Debug, Clone)]
pub enum OcrChunk {
    /// Quick result (low precision, returned first)
    Quick(Vec<TextBlock>),
    /// Detailed result (high precision, returned later)
    Detailed(Vec<TextBlock>),
}

/// OCR engine (stub/empty shell, waiting for PaddleOCR integration)
pub struct OcrEngine;

impl Default for OcrEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrEngine {
    pub fn new() -> Self {
        Self
    }

    /// Recognize - full-frame OCR (currently returns an empty result)
    pub async fn recognize(&self, _frame: &Frame) -> Result<Vec<TextBlock>> {
        Ok(vec![])
    }

    /// Quick recognition - only locates keyword regions
    pub async fn recognize_quick(
        &self,
        frame: &Frame,
        _keywords: &[String],
    ) -> Result<Vec<TextBlock>> {
        self.recognize(frame).await
    }

    /// Region recognition - only recognizes the specified region
    pub async fn recognize_region(&self, _frame: &Frame, _region: Rect) -> Result<Vec<TextBlock>> {
        Ok(vec![])
    }
}
