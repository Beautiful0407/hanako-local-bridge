//! Unified API layer - see / find / do / say + fallback strategy
//!
//! Only compiled when `http-server` feature is enabled.

#[cfg(feature = "http-server")]
pub mod http;

#[cfg(feature = "http-server")]
mod api_impl {
    use crate::core::{Point, Result, Scope, SessionHandle, Target};
    use crate::input::InputEngine;
    use crate::platform::WindowManager;
    use crate::vision::{FindResult, PerceiveWhat, Perception, Query, VisionEngine};

    /// Unified API entrypoint
    pub struct UnifiedApi {
        vision: VisionEngine,
        input: InputEngine,
        window_mgr: WindowManager,
    }

    impl UnifiedApi {
        pub fn new() -> Self {
            let cleanup = crate::utils::cleanup::CleanupQueue::new();
            Self {
                vision: VisionEngine::new(cleanup.clone()),
                input: InputEngine::new(),
                window_mgr: WindowManager::new(),
            }
        }

        // ─────────────────────────────── see ───────────────────────────────

        /// Perceive - screenshot + analyze
        pub async fn see(
            &self,
            session: &SessionHandle,
            scope: Scope,
            what: PerceiveWhat,
        ) -> Result<Perception> {
            let target = session.target.read().await;
            self.vision.see(&*target, scope, what).await
        }

        // ─────────────────────────────── find ───────────────────────────────

        /// Find - locate image/text/color
        pub async fn find(&self, session: &SessionHandle, query: &Query) -> Result<FindResult> {
            // Take a screenshot first
            let target = session.target.read().await;
            let frame = self.vision.capture(&*target, Scope::Window).await?;

            // Locate
            self.vision.find(&frame, query).await
        }

        // ─────────────────────────────── do ───────────────────────────────

        /// Operate - click/drag/keypress
        pub async fn do_(&self, session: &SessionHandle, action: Action) -> Result<DoResult> {
            let mut target = session.target.write().await;

            match action {
                Action::Click { x, y } => {
                    self.input.click(&mut *target, Point { x, y }).await?;
                    Ok(DoResult::Success)
                }
                Action::DoubleClick { x, y } => {
                    self.input.click(&mut *target, Point { x, y }).await?;
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    self.input.click(&mut *target, Point { x, y }).await?;
                    Ok(DoResult::Success)
                }
                Action::Drag { start, end } => {
                    self.input.drag(&mut *target, start, end).await?;
                    Ok(DoResult::Success)
                }
                Action::Press { key } => {
                    self.input.press(&mut *target, &key).await?;
                    Ok(DoResult::Success)
                }
                Action::Hotkey { keys } => {
                    let keys_ref: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                    self.input.hotkey(&mut *target, &keys_ref).await?;
                    Ok(DoResult::Success)
                }
            }
        }

        // ─────────────────────────────── say ───────────────────────────────

        /// Communicate - send message + verify delivery
        pub async fn say(&self, session: &SessionHandle, text: &str) -> Result<SayResult> {
            let mut target = session.target.write().await;

            // Strategy 1: direct send
            match self.input.send_text(text, &mut *target).await {
                Ok(()) => {
                    // Verify: screenshot the input area and confirm the text appears
                    // TODO: verification logic
                    return Ok(SayResult::Delivered);
                }
                Err(e) => {
                    tracing::warn!("direct send failed: {}, starting fallback", e);
                }
            }

            // Fallback 1: clipboard + paste
            // TODO: clipboard strategy

            // Fallback 2: send character by character
            // TODO

            // Fallback 3: screenshot and analyze the failure reason, report to human
            Ok(SayResult::Failed("all strategies exhausted".to_string()))
        }
    }

    // ─────────────────────────────── types ───────────────────────────────

    /// Operation type
    #[derive(Debug, Clone)]
    pub enum Action {
        Click { x: i32, y: i32 },
        DoubleClick { x: i32, y: i32 },
        Drag { start: Point, end: Point },
        Press { key: String },
        Hotkey { keys: Vec<String> },
    }

    /// Operation result
    #[derive(Debug, Clone)]
    pub enum DoResult {
        Success,
        Verified { before: String, after: String },
        Failed(String),
    }

    /// Communication result
    #[derive(Debug, Clone)]
    pub enum SayResult {
        Delivered,
        Partial { sent: String, failed: String },
        Failed(String),
    }
}

#[cfg(feature = "http-server")]
pub use api_impl::*;
