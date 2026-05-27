//! Orchestrator-lifecycle events (M5-02).
//!
//! Three events fired by `ConversationOrchestrator::run_turn`:
//! - [`CONVERSATION_STARTED`] at the top of the turn loop.
//! - [`CONVERSATION_COMPLETED`] after `emit_end_turn`.
//! - [`CONVERSATION_FAILED`] on any `Err` return.
//!
//! Wire strings are byte-locked at v0.6.0. M5-04..M5-13 may add more event
//! constants to this same module (streaming, permission, hooks, session,
//! resume, repl) or split into sibling submodules; for M5-02 only the three
//! lifecycle markers live here.

/// Conversation start marker — fired once per `run_turn` invocation.
pub const CONVERSATION_STARTED: &str = "tengu_orchestrator_conversation_started";

/// Conversation end marker (success) — fired after `emit_end_turn`.
pub const CONVERSATION_COMPLETED: &str = "tengu_orchestrator_conversation_completed";

/// Conversation end marker (failure) — fired on any non-success return.
pub const CONVERSATION_FAILED: &str = "tengu_orchestrator_conversation_failed";

/// Order-locked array of all orchestrator-lifecycle names; consumed by
/// `tengu::ALL_EVENT_NAMES`. Append-only: never reorder or remove entries.
pub(crate) const NAMES: &[&str] = &[
    CONVERSATION_STARTED,
    CONVERSATION_COMPLETED,
    CONVERSATION_FAILED,
];
