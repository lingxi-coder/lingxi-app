//! Prefetch the selector result in parallel with the main API call.
//!
//! The runtime kicks off this prefetch as soon as it knows the turn
//! query; the result is awaited just before assembling the next prompt.

use crate::selector::MemorySelector;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use traits::RuntimeSpawner;

/// Side-channel that fires the memory selector concurrently with the
/// main turn.
pub struct MemoryPrefetch {
    /// Selector that scores and ranks the available memory set.
    #[allow(dead_code)] // wired into the prefetch body in Plan 08
    selector: Arc<MemorySelector>,
    /// Runtime adapter used to spawn the background task.
    #[allow(dead_code)] // wired into the prefetch body in Plan 08
    runtime: Arc<dyn RuntimeSpawner>,
}

/// Handle awaiting an in-flight prefetch.
pub struct PendingMemoryPrefetch {
    /// Receiver that produces the selected file paths.
    pub rx: tokio::sync::Mutex<Option<oneshot::Receiver<Vec<PathBuf>>>>,
}

impl MemoryPrefetch {
    /// Construct a prefetcher bound to a selector and the platform runtime.
    #[must_use]
    pub fn new(selector: Arc<MemorySelector>, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        Self { selector, runtime }
    }

    /// Kick off a background selector call and return a pending handle.
    pub async fn start(&self, _query: String, _memory_dir: PathBuf) -> PendingMemoryPrefetch {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .runtime
            .spawn(
                "memory-prefetch",
                Box::pin(async move {
                    // Plan 08 wires this to the actual selector. Here we
                    // ship the channel plumbing only.
                    let _ = tx.send(Vec::<PathBuf>::new());
                }),
            )
            .await;
        PendingMemoryPrefetch {
            rx: tokio::sync::Mutex::new(Some(rx)),
        }
    }
}
