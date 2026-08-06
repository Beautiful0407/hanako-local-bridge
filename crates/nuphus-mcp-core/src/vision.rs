//! `desktop_vision` — BYOK cloud vision understanding (OpenAI-compatible Chat Completions API).
//!
//! Uses the same protocol shape as `src/desktop/vision_ocr.rs` in the main crate (base64 image_url),
//! but is **fully independent**: it does not depend on the main crate's config/registry system, reading env vars directly:
//!
//! | Env var | Required | Default | Description |
//! |---------|----------|---------|-------------|
//! | `NUPHUS_MCP_VISION_API_KEY` | ✅ | - | API key (missing → explicit error) |
//! | `NUPHUS_MCP_VISION_BASE_URL` | - | `https://api.openai.com/v1` | OpenAI-compatible base URL |
//! | `NUPHUS_MCP_VISION_MODEL` | ✅ | - | Vision model ID (e.g. `gpt-4o-mini`) |
//!
//! When key/model are not configured, returns a clear error: no panic, no silent "fake green" degradation.

use serde_json::{json, Value};

/// BYOK vision config (parsed from environment variables).
#[derive(Debug, Clone)]
pub struct VisionConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl VisionConfig {
    /// Read config from environment variables. Returns a clear error for missing required fields.
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("NUPHUS_MCP_VISION_API_KEY")
            .map_err(|_| "NUPHUS_MCP_VISION_API_KEY required: set this environment variable to your vision model API key (BYOK)".to_string())?;
        if api_key.trim().is_empty() {
            return Err("NUPHUS_MCP_VISION_API_KEY required: value must not be empty".to_string());
        }

        let model = std::env::var("NUPHUS_MCP_VISION_MODEL")
            .map_err(|_| "NUPHUS_MCP_VISION_MODEL required: set the vision model id (e.g. gpt-4o-mini, qwen-vl-max)".to_string())?;
        if model.trim().is_empty() {
            return Err("NUPHUS_MCP_VISION_MODEL required: value must not be empty".to_string());
        }

        let base_url = std::env::var("NUPHUS_MCP_VISION_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let base_url = base_url.trim_end_matches('/').to_string();

        // Enforce HTTPS: screenshots sent to a remote endpoint must not leak over
        // plaintext. Loopback (localhost/127.0.0.1) is allowed over http for local
        // test endpoints (e.g. Ollama).
        let is_loopback = match reqwest::Url::parse(&base_url) {
            Ok(u) => matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "::1")),
            Err(_) => false,
        };
        if base_url.starts_with("http://") && !is_loopback {
            return Err(format!(
                "NUPHUS_MCP_VISION_BASE_URL must use https (plain http is only allowed for localhost/127.0.0.1 test endpoints): {base_url}"
            ));
        }

        Ok(Self {
            api_key,
            base_url,
            model,
        })
    }
}

/// Default prompt (used when no prompt parameter is given; English to be compatible with any vision model).
pub const DEFAULT_PROMPT: &str =
    "Describe the contents of this image in detail. If it contains readable text, output the text verbatim.";

/// Run vision understanding on an image, returning the model's text reply.
///
/// - `image_path`: local image path (BMP/PNG both accepted; BMP is auto-converted to PNG before sending).
/// - `prompt`: optional prompt, defaults to [`DEFAULT_PROMPT`].
pub async fn vision_image(image_path: &str, prompt: Option<&str>) -> Result<String, String> {
    let config = VisionConfig::from_env()?;

    // 1. Read image → convert to PNG (LLM APIs generally do not accept image/bmp)
    let image_bytes = std::fs::read(image_path).map_err(|e| format!("read image failed: {}", e))?;
    let (mime_type, final_bytes) = if image_path.to_lowercase().ends_with(".png") {
        ("image/png", image_bytes)
    } else {
        let img = image::load_from_memory(&image_bytes)
            .map_err(|e| format!("decode image failed: {}", e))?;
        let mut png_buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png_buf, image::ImageFormat::Png)
            .map_err(|e| format!("convert to PNG failed: {}", e))?;
        ("image/png", png_buf.into_inner())
    };

    use base64::Engine;
    let base64_image = base64::engine::general_purpose::STANDARD.encode(&final_bytes);
    let data_url = format!("data:{mime_type};base64,{base64_image}");

    // 2. Build Chat Completions request
    let prompt_text = prompt.filter(|p| !p.is_empty()).unwrap_or(DEFAULT_PROMPT);

    let body = json!({
        "model": config.model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt_text },
                    {
                        "type": "image_url",
                        "image_url": { "url": data_url, "detail": "high" }
                    }
                ]
            }
        ],
        "max_tokens": 4096
    });

    let url = format!("{}/chat/completions", config.base_url);

    // 3. POST
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("build http client failed: {}", e))?;

    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("vision API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        return Err(format!("vision API returned HTTP {status}: {err_body}"));
    }

    // 4. Parse response
    let resp_json: Value = response
        .json()
        .await
        .map_err(|e| format!("parse vision API response failed: {}", e))?;

    let text = resp_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("vision API returned empty content".to_string());
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env var mutations are global within the test process; serialize to avoid parallel tests polluting each other.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    const VARS: [&str; 3] = [
        "NUPHUS_MCP_VISION_API_KEY",
        "NUPHUS_MCP_VISION_BASE_URL",
        "NUPHUS_MCP_VISION_MODEL",
    ];

    /// Saves and clears the three vars; restores them when the test ends.
    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }
    impl EnvGuard {
        fn clear() -> Self {
            let saved = VARS
                .iter()
                .map(|k| (k.to_string(), std::env::var(k).ok()))
                .collect();
            for k in VARS {
                std::env::remove_var(k);
            }
            EnvGuard { saved }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn missing_api_key_returns_clear_error() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_MODEL", "gpt-4o-mini");
        let err = VisionConfig::from_env().expect_err("key missing must fail");
        assert!(
            err.contains("NUPHUS_MCP_VISION_API_KEY"),
            "error must name the missing env var: {err}"
        );
    }

    #[test]
    fn missing_model_returns_clear_error() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_API_KEY", "sk-test");
        let err = VisionConfig::from_env().expect_err("model missing must fail");
        assert!(
            err.contains("NUPHUS_MCP_VISION_MODEL"),
            "error must name the missing env var: {err}"
        );
    }

    #[test]
    fn full_config_parses_with_defaults() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_API_KEY", "sk-test");
        std::env::set_var("NUPHUS_MCP_VISION_MODEL", "qwen-vl-max");
        let cfg = VisionConfig::from_env().expect("full config ok");
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.model, "qwen-vl-max");
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn remote_http_base_url_rejected() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_API_KEY", "sk-test");
        std::env::set_var("NUPHUS_MCP_VISION_MODEL", "m");
        std::env::set_var("NUPHUS_MCP_VISION_BASE_URL", "http://api.example.com/v1");
        let err = VisionConfig::from_env().expect_err("remote http must fail");
        assert!(err.contains("https"), "error must mention https: {err}");
    }

    #[test]
    fn localhost_http_base_url_allowed() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_API_KEY", "sk-test");
        std::env::set_var("NUPHUS_MCP_VISION_MODEL", "m");
        std::env::set_var("NUPHUS_MCP_VISION_BASE_URL", "http://127.0.0.1:11434/v1");
        let cfg = VisionConfig::from_env().expect("loopback http ok");
        assert_eq!(cfg.base_url, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        let _lock = env_lock();
        let _g = EnvGuard::clear();
        std::env::set_var("NUPHUS_MCP_VISION_API_KEY", "sk-test");
        std::env::set_var("NUPHUS_MCP_VISION_MODEL", "m");
        std::env::set_var("NUPHUS_MCP_VISION_BASE_URL", "https://example.com/v1/");
        let cfg = VisionConfig::from_env().expect("ok");
        assert_eq!(cfg.base_url, "https://example.com/v1");
    }
}
