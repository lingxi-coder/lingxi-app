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
pub mod prompt;
pub mod sse;
pub mod streaming_loop;
pub mod turn_loop;

// test_support carries the HookExecutor / PermissionGate trait definitions
// that ConversationOrchestrator's signature uses; it MUST be available in
// production builds (the plan's `#[cfg(any(test, feature = "test-support"))]`
// guard would hide those traits from production). M5-05 / M5-06 will move
// the traits to lingxi-permission / lingxi-hooks; until then they live in
// test_support unconditionally.
pub mod test_support;

/// Streaming-path test fixtures (`MockStreamingApiClient`, `scripted!`).
/// Gated behind `test-support` so production builds don't pull them in.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support_stream;

pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};
pub use conversation::{
    AnthropicProviderAdapter, ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient,
};
pub use error::OrchestratorError;
pub use prompt::{
    assemble_system_prompt, FileTree, FileTreeEntry, GitStatus, MemoryFile, SystemPromptContext,
};
