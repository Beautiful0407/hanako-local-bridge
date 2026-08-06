//! Local model path resolution and readiness checks (shared by PaddleOCR + YOLO).
//!
//! Keeps the same priority as the main crate's `src/desktop/mod.rs::resolve_models_dir`, ensuring
//! nuphus-mcp / main app / desktop-api all hit the same model directory:
//! 1. Env var `NUPHUS_MODELS_DIR`
//! 2. User data directory `data_dir/Nuphus/models`
//! 3. exe-relative path candidates
//! 4. cwd-relative path candidates (development/test environments)
//!
//! `models_dir_for_write` is the downloader's write target (creatable); `resolve_models_dir`
//! only returns existing directories.

use std::path::{Path, PathBuf};

/// PaddleOCR text detection model (~4.6 MB)
pub const PADDLE_DET_MODEL: &str = "ch_PP-OCRv4_det.onnx";
/// PaddleOCR text recognition model (~9.2 MB)
pub const PADDLE_REC_MODEL: &str = "ch_PP-OCRv4_rec.onnx";
/// PaddleOCR character dictionary (~93 KB, 6623 classes)
pub const PADDLE_DICT: &str = "ch_PP-OCR_keys_v1.txt";
/// YOLO UI element detection model (icon_detect.onnx, ~80 MB, optional enhancement)
pub const YOLO_MODEL: &str = "icon_detect.onnx";

/// The three files required by PaddleOCR
pub const PADDLE_OCR_FILES: [&str; 3] = [PADDLE_DET_MODEL, PADDLE_REC_MODEL, PADDLE_DICT];

/// Resolve an existing model directory (read-only, does not create).
pub fn resolve_models_dir() -> Option<PathBuf> {
    // 1. Env var NUPHUS_MODELS_DIR
    if let Ok(dir) = std::env::var("NUPHUS_MODELS_DIR") {
        let p = PathBuf::from(&dir);
        if p.exists() {
            tracing::debug!("[models] loaded from NUPHUS_MODELS_DIR: {}", p.display());
            return Some(p);
        }
    }

    // 2. User data directory
    if let Some(data_dir) = dirs::data_dir() {
        let p = data_dir.join("Nuphus").join("models");
        if p.exists() {
            tracing::debug!("[models] loaded from data_dir: {}", p.display());
            return Some(p);
        }
    }

    // 3. exe-relative path candidates (placed next to the release artifacts)
    if let Ok(exe) = std::env::current_exe() {
        let exe_candidates: Vec<PathBuf> = (0..=2)
            .filter_map(|n| {
                let mut p = exe.parent()?;
                for _ in 0..n {
                    p = p.parent()?;
                }
                Some(p.join("desktop").join("models"))
            })
            .collect();
        for c in &exe_candidates {
            if c.exists() {
                tracing::debug!("[models] loaded from exe path: {}", c.display());
                return Some(c.clone());
            }
        }
    }

    // 4. cwd candidates (development/test environments)
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_candidates = vec![
            cwd.join("src-tauri").join("desktop").join("models"),
            cwd.join("../src-tauri/desktop/models"),
        ];
        if let Some(parent) = cwd.parent() {
            let parent_candidates = vec![
                parent.join("src-tauri").join("desktop").join("models"),
                parent.join("../src-tauri/desktop/models"),
            ];
            for c in parent_candidates {
                if c.exists() {
                    tracing::debug!("[models] loaded from cwd parent path: {}", c.display());
                    return Some(c);
                }
            }
        }
        for c in &cwd_candidates {
            if c.exists() {
                tracing::debug!("[models] loaded from cwd path: {}", c.display());
                return Some(c.clone());
            }
        }
    }

    tracing::warn!("[models] model directory not found");
    None
}

/// The downloader's write target: prefers `NUPHUS_MODELS_DIR`, otherwise the user data directory, creating it if necessary.
pub fn models_dir_for_write() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("NUPHUS_MODELS_DIR") {
        let p = PathBuf::from(&dir);
        std::fs::create_dir_all(&p)
            .map_err(|e| format!("failed to create model directory {}: {e}", p.display()))?;
        return Ok(p);
    }
    let data_dir = dirs::data_dir().ok_or_else(|| {
        "unable to locate user data directory (dirs::data_dir returned None)".to_string()
    })?;
    let p = data_dir.join("Nuphus").join("models");
    std::fs::create_dir_all(&p)
        .map_err(|e| format!("failed to create model directory {}: {e}", p.display()))?;
    Ok(p)
}

/// Validate that the PaddleOCR trio is complete. On Err, the missing file paths are attached (for download guidance).
pub fn validate_ocr_models(dir: &Path) -> Result<(), String> {
    let missing: Vec<String> = PADDLE_OCR_FILES
        .iter()
        .filter(|f| !dir.join(f).exists())
        .map(|f| dir.join(f).display().to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "PaddleOCR model files missing ({}): {}",
            missing.len(),
            missing.join(", ")
        ))
    }
}

/// Whether the YOLO UI element detection model exists (optional enhancement; when missing, only OCR is available).
pub fn yolo_model_available(dir: &Path) -> bool {
    dir.join(YOLO_MODEL).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ocr_models_reports_missing_files() {
        // Empty temp dir → must report the missing files and not panic
        let tmp = std::env::temp_dir().join("nuphus_desktop_api_missing_models_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let err = validate_ocr_models(&tmp).expect_err("empty dir must fail");
        assert!(err.contains(PADDLE_DET_MODEL), "err should name det: {err}");
        assert!(err.contains(PADDLE_REC_MODEL), "err should name rec: {err}");
        assert!(err.contains(PADDLE_DICT), "err should name dict: {err}");
    }

    #[test]
    fn yolo_model_missing_is_not_fatal() {
        let tmp = std::env::temp_dir().join("nuphus_desktop_api_yolo_missing_test");
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!yolo_model_available(&tmp));
        // OCR validation is independent of YOLO: missing OCR files are an error, missing YOLO is just optional
        assert!(validate_ocr_models(&tmp).is_err());
    }

    #[test]
    fn file_constants_non_empty() {
        assert!(PADDLE_OCR_FILES.iter().all(|f| !f.is_empty()));
        assert_eq!(PADDLE_OCR_FILES.len(), 3);
    }
}
