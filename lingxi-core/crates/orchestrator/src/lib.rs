//! Top-level conversational orchestrator — drives the v0.6.0 turn loop.
//!
//! `ConversationOrchestrator` is the single owner of an in-process AI
//! coding conversation:
//!
//! 1. Append user prompt to `SessionState`.
//! 2. Call `messages_create_non_stream` (batched; streaming lands in M5-04).
//! 3. Dispatch `tool_use` blocks through `ToolRegistry` (after `PreToolUse`
//!    hook + permission gate; both stubbed in M5-02, real in M5-05 / M5-06).
//! 4. Append assistant message to session.
//! 5. Loop until `stop_reason == "end_turn"` or `max_turns` exceeded.
//!
//! See spec §2.2 (data flow diagram) and §4.2 (turn loop limits).
#![forbid(unsafe_code)]

pub mod config;
pub mod conversation;
pub mod error;
pub mod turn_loop;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};
// pub use conversation::{ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient};  // Task 10
pub use error::OrchestratorError;
