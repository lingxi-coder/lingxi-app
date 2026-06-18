//! Prefetch the selector result in parallel with the main API call.
//!
//! The runtime kicks off this prefetch as soon as it knows the turn
//! query; the result is awaited just before assembling the next prompt.
//!
//! P0.1: the pending handle resolves to a `Vec<`[`SurfacedMemory`]`>` — the
//! exact shape [`crate::surfacing::render_surfacing_block`] renders — so the
//! orchestrator's `relevant_memory_reminder_message` can await this handle and
//! render the surfaced block with no further disk work. The default stub body
//! resolves to an EMPTY vec (the selector is not yet wired to a real
//! side-query client at the composition root), so the surfacing reminder is a
//! strict no-op and the locked fixtures stay byte-identical — exactly matching
//! claude-code's `tengu_moth_copse`-default-false gate (the LingXi equivalent
//! gate is "is a prefetch wired at all", `memory_prefetch.is_some()`).

use crate::selector::MemorySelector;
use crate::surfacing::SurfacedMemory;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use traits::RuntimeSpawner;

/// Side-channel that fires the memory selector concurrently with the
/// main turn.
pub struct MemoryPrefetch {
    /// Selector that scores and ranks the available memory set.
    #[allow(dead_code)] // wired into the prefetch body when a real selector lands
    selector: Option<Arc<MemorySelector>>,
    /// Runtime adapter used to spawn the background task.
    runtime: Arc<dyn RuntimeSpawner>,
    /// Pre-resolved surfaced set: when `Some`, [`Self::start`] short-circuits
    /// to this set instead of running the (not-yet-wired) selector body. This is
    /// the injection seam a composition root uses once it has already selected
    /// the relevant memories out of band, and the deterministic seam the
    /// orchestrator's surfacing tests drive. `None` ⇒ the default stub body
    /// (empty result ⇒ inert surfacing reminder).
    fixed_result: Option<Vec<SurfacedMemory>>,
}

/// Handle awaiting an in-flight prefetch.
pub struct PendingMemoryPrefetch {
    /// Receiver that produces the surfaced-memory set (ready to render).
    pub rx: tokio::sync::Mutex<Option<oneshot::Receiver<Vec<SurfacedMemory>>>>,
}

impl PendingMemoryPrefetch {
    /// Await the in-flight prefetch, consuming the one-shot receiver.
    ///
    /// Returns the surfaced-memory set, or an empty vec if the receiver was
    /// already taken or the background task dropped its sender (e.g. a cancelled
    /// turn). Never errors — a failed prefetch must not break the turn.
    pub async fn take(&self) -> Vec<SurfacedMemory> {
        let rx = self.rx.lock().await.take();
        match rx {
            Some(rx) => rx.await.unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl MemoryPrefetch {
    /// Construct a prefetcher bound to a selector and the platform runtime.
    #[must_use]
    pub fn new(selector: Arc<MemorySelector>, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        Self {
            selector: Some(selector),
            runtime,
            fixed_result: None,
        }
    }

    /// Construct a prefetcher that resolves to a PRE-SELECTED surfaced set,
    /// bypassing the selector body. Used by a composition root that has already
    /// picked the relevant memories, and by the orchestrator's surfacing tests
    /// to drive `relevant_memory_reminder_message` deterministically. The
    /// runtime is still required (the background task carries the result through
    /// the same one-shot channel) but no selector is needed.
    #[must_use]
    pub fn with_fixed_result(
        runtime: Arc<dyn RuntimeSpawner>,
        result: Vec<SurfacedMemory>,
    ) -> Self {
        Self {
            selector: None,
            runtime,
            fixed_result: Some(result),
        }
    }

    /// Kick off a background selector call and return a pending handle.
    ///
    /// When a [`Self::with_fixed_result`] set is present, the background task
    /// ships that set; otherwise it ships the channel plumbing only and resolves
    /// to an EMPTY surfaced-memory set (no real selector wired yet), so the
    /// surfacing reminder is inert by default. The signature is the final one: a
    /// real selector body fills `tx` with the loaded, ranked [`SurfacedMemory`]
    /// set.
    pub async fn start(&self, _query: String, _memory_dir: PathBuf) -> PendingMemoryPrefetch {
        let (tx, rx) = oneshot::channel();
        let result = self.fixed_result.clone().unwrap_or_default();
        let _ = self
            .runtime
            .spawn(
                "memory-prefetch",
                Box::pin(async move {
                    // A real selector wires here: select_relevant → load files →
                    // map to SurfacedMemory. Until then, ship the fixed result
                    // (or empty ⇒ inert surfacing reminder).
                    let _ = tx.send(result);
                }),
            )
            .await;
        PendingMemoryPrefetch {
            rx: tokio::sync::Mutex::new(Some(rx)),
        }
    }
}
