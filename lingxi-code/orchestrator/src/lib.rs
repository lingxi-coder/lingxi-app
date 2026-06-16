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
pub mod model;
pub mod cost_wiring;
pub mod cwd_changed_firer;
pub mod diagnostics;
pub mod error;
pub mod file_changed_firer;
pub mod handle_impl;
pub mod hook_prompt_runner;
pub mod image_input;
pub mod mcp_hook_dispatcher;
pub mod prompt;
pub mod provider_adapter;
pub mod resume;
pub mod sse;
pub mod streaming_loop;
pub mod task_completed_firer;
pub mod task_created_firer;
pub mod task_lifecycle_hook_firer;
pub mod teammate_idle_firer;
pub mod token_budget;
pub mod turn_loop;

// test_support carries the HookExecutor / PermissionGate trait definitions
// that ConversationOrchestrator's signature uses; it MUST be available in
// production builds (the plan's `#[cfg(any(test, feature = "test-support"))]`
// guard would hide those traits from production). M5-05 / M5-06 will move
// the traits to lingxi-permission / lingxi-hooks; until then they live in
// test_support unconditionally.
pub mod test_support;

/// Streaming-path test fixtures (`MockStreamingApiClient`, `scripted!`).
/// Compiled unconditionally — mirrors `test_support` (M5-02 made that
/// module unconditional; this one follows the same convention so
/// integration tests can use the fixtures without a feature flag).
pub mod test_support_stream;

pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};
pub use conversation::{
    ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient, StreamingApiClient,
    TurnOutcome,
};
pub use error::OrchestratorError;
pub use hook_prompt_runner::ApiClientHookPromptRunner;
pub use mcp_hook_dispatcher::OrchestratorHookDispatcher;
pub use cwd_changed_firer::OrchestratorCwdChangedFirer;
pub use file_changed_firer::OrchestratorFileChangedFirer;
pub use task_completed_firer::OrchestratorTaskCompletedFirer;
pub use task_created_firer::OrchestratorTaskCreatedFirer;
pub use task_lifecycle_hook_firer::OrchestratorTaskLifecycleHookFirer;
pub use teammate_idle_firer::OrchestratorTeammateIdleFirer;
pub use prompt::{
    assemble_system_prompt, FileTree, FileTreeEntry, GitStatus, MemoryFile, SystemPromptContext,
};
pub use provider_adapter::ProviderApiAdapter;
pub use resume::{replay_session_state, state_from_messages, ReplayedSession, ResumeError};
