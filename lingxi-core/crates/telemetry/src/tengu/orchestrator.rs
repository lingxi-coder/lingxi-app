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

/// Streaming turn started — fired at the top of
/// `ConversationOrchestrator::run_turn_streaming` before any session
/// mutation. (M5-04)
pub const TURN_STREAMING_STARTED: &str = "tengu_orchestrator_turn_streaming_started";

/// Streaming turn completed — fired after the final `emit_end_turn` in
/// the streaming path. Payload carries `turn_count` and (optionally)
/// `stop_reason`. (M5-04)
pub const TURN_STREAMING_COMPLETED: &str = "tengu_orchestrator_turn_streaming_completed";

/// Permission prompt about to be shown to the user. Fired by
/// [`lingxi_permission::InteractivePromptingGate::prompt_user`]
/// immediately before each stderr write — once on the initial prompt and
/// once per retry. (M5-05)
pub const PERMISSION_PROMPTED: &str = "tengu_orchestrator_permission_prompted";

/// Permission prompt answered with a definitive y/n. Fired AFTER the
/// user's answer is parsed; NOT fired on retry / invalid input.
/// Payload carries `attempts` so consumers can distinguish a first-try
/// answer from a recovered one. (M5-05)
pub const PERMISSION_ANSWERED: &str = "tengu_orchestrator_permission_answered";

/// `PreToolUse` hook chain about to fire. Emitted by the orchestrator's
/// `dispatch_tool_with_hooks` BEFORE consulting the executor. (M5-06)
pub const HOOK_PRE_STARTED: &str = "tengu_orchestrator_hook_pre_started";

/// `PreToolUse` hook chain returned. Emitted by the orchestrator AFTER the
/// executor folds every matching hook's response into an
/// `AggregateHookResult`. Payload carries `decision` (one of
/// `"allow" / "block" / "approve" / "continue" / "none"`) and
/// `duration_ms`. (M5-06)
pub const HOOK_PRE_COMPLETED: &str = "tengu_orchestrator_hook_pre_completed";

/// `PreToolUse` hook chain errored (executor surfaced a failure outcome).
/// (M5-06)
pub const HOOK_PRE_FAILED: &str = "tengu_orchestrator_hook_pre_failed";

/// `PostToolUse` hook chain about to fire. (M5-06)
pub const HOOK_POST_STARTED: &str = "tengu_orchestrator_hook_post_started";

/// `PostToolUse` hook chain returned. Payload carries `duration_ms` and
/// `mutated_response: bool`. (M5-06)
pub const HOOK_POST_COMPLETED: &str = "tengu_orchestrator_hook_post_completed";

/// `PostToolUse` hook chain errored. (M5-06)
pub const HOOK_POST_FAILED: &str = "tengu_orchestrator_hook_post_failed";

/// HTTP hook URL rejected by the SSRF guard. Emitted by the HTTP arm
/// before any request is sent. (M5-06)
pub const HOOK_HTTP_SKIPPED_SSRF: &str = "tengu_orchestrator_hook_http_skipped_ssrf";

/// Hook execution exceeded its effective timeout. Emitted by the HTTP /
/// Agent arms on `tokio::time::timeout` elapse. (M5-06)
pub const HOOK_TIMEOUT: &str = "tengu_orchestrator_hook_timeout";

/// Order-locked array of all orchestrator-lifecycle names; consumed by
/// `tengu::ALL_EVENT_NAMES`. Append-only: never reorder or remove entries.
pub(crate) const NAMES: &[&str] = &[
    CONVERSATION_STARTED,
    CONVERSATION_COMPLETED,
    CONVERSATION_FAILED,
    TURN_STREAMING_STARTED,
    TURN_STREAMING_COMPLETED,
    PERMISSION_PROMPTED,
    PERMISSION_ANSWERED,
    HOOK_PRE_STARTED,
    HOOK_PRE_COMPLETED,
    HOOK_PRE_FAILED,
    HOOK_POST_STARTED,
    HOOK_POST_COMPLETED,
    HOOK_POST_FAILED,
    HOOK_HTTP_SKIPPED_SSRF,
    HOOK_TIMEOUT,
];
