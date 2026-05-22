//! Tool progress channel — stub for Task 1.
//!
//! Task 3 fills in the real `mpsc::Sender<ToolProgressEvent>` implementation.
//! Until then, this module exposes a no-op sender so the `Tool` trait
//! signature compiles.

/// Sender for tool progress events. Stubbed in Task 1.
///
/// Tools accept a `ToolProgressSender` to stream incremental progress
/// (partial reads, token counts, etc.) back to the dispatcher. The full
/// implementation lands in Task 3.
#[derive(Clone, Debug, Default)]
pub struct ToolProgressSender {
    _placeholder: (),
}

impl ToolProgressSender {
    /// Construct a no-op sender (for tests / Task 1).
    #[must_use]
    pub fn null() -> Self {
        Self::default()
    }
}
