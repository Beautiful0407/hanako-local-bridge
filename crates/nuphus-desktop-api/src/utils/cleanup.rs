//! Utility module - cleanup queue, error handling

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Global cleanup queue - automatically deletes temporary files
pub struct CleanupQueue {
    sender: mpsc::UnboundedSender<PathBuf>,
}

impl CleanupQueue {
    pub fn new() -> Arc<Self> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<PathBuf>();

        // Start the background cleanup task
        tokio::spawn(async move {
            let mut pending: VecDeque<(PathBuf, tokio::time::Instant)> = VecDeque::new();
            let cleanup_delay = tokio::time::Duration::from_secs(300); // 5-minute delay
            let max_size_mb: usize = 100;

            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                interval.tick().await;

                // Receive new files
                while let Ok(path) = receiver.try_recv() {
                    pending.push_back((path, tokio::time::Instant::now() + cleanup_delay));
                }

                // Execute expired cleanups
                let now = tokio::time::Instant::now();
                while let Some((_path, deadline)) = pending.front() {
                    if *deadline > now {
                        break;
                    }
                    let (path, _) = pending.pop_front().unwrap();
                    let _ = std::fs::remove_file(&path);
                    tracing::debug!("cleaned up: {}", path.display());
                }

                // LRU: if total size exceeds the limit, clean the oldest (recompute after each deletion)
                while Self::calc_size(&pending) > max_size_mb * 1024 * 1024 && pending.len() > 1 {
                    if let Some((path, _)) = pending.pop_front() {
                        let _ = std::fs::remove_file(&path);
                        tracing::debug!("LRU cleanup: removed {}", path.display());
                    }
                }
            }
        });

        Arc::new(Self { sender })
    }

    /// Register a file for cleanup
    pub fn send(&self, path: PathBuf) {
        let _ = self.sender.send(path);
    }

    fn calc_size(pending: &VecDeque<(PathBuf, tokio::time::Instant)>) -> usize {
        // Rough estimate
        pending.len() * 500 * 1024 // assume average 500KB
    }
}
