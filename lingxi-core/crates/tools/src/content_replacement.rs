//! Content replacement — stub for Task 1.
//!
//! Task 3 fills in the real `ContentReplacementState`, which tracks
//! tool-output deltas that must be replayed when a tool re-emits content
//! (e.g. streaming reads). For now this is an empty placeholder so
//! [`crate::context::ToolUseContext`] can carry an `Arc<Mutex<_>>` slot.

/// State for the content-replacement protocol (placeholder).
///
/// Held inside `ToolUseContext` behind an `Arc<Mutex<_>>` so concurrent
/// tool invocations can coordinate replay metadata. The fields are filled
/// in by Task 3.
#[derive(Debug, Default)]
pub struct ContentReplacementState {
    _placeholder: (),
}
