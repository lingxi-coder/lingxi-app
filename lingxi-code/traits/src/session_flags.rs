//! Process-global session-level flags resolved once at session init and read by
//! leaf consumers that have no per-call context handle.
//!
//! claude-code exposes session-mode predicates as module-level globals (e.g.
//! `getIsNonInteractiveSession()`); prompt builders call them inline. The Rust
//! port models per-tool-call interactivity via `ToolUseContext.is_non_interactive_session`,
//! but the wire `tools` array (and thus each tool's `prompt()`) is assembled
//! WITHOUT a `ToolUseContext`. This global is the faithful analog for those
//! prompt-build-time reads: set once by the composition point that knows the
//! session mode, read by builders such as the `AgentTool` fork gate.

use std::sync::atomic::{AtomicBool, Ordering};

/// `getIsNonInteractiveSession()` analog. Defaults `false` (interactive); set by
/// [`ConversationOrchestrator::new`](../../orchestrator) from the session's
/// `interactive_permissions` (`-p`/print/headless ⇒ non-interactive).
static NON_INTERACTIVE_SESSION: AtomicBool = AtomicBool::new(false);

/// Record whether the current process is a non-interactive (`-p`/print/headless)
/// session. Idempotent; safe to call repeatedly (the value is fixed per process).
pub fn set_non_interactive_session(non_interactive: bool) {
    NON_INTERACTIVE_SESSION.store(non_interactive, Ordering::Relaxed);
}

/// Whether the current process is a non-interactive session (default `false`).
#[must_use]
pub fn is_non_interactive_session() -> bool {
    NON_INTERACTIVE_SESSION.load(Ordering::Relaxed)
}
