//! `tengu_agent_*` event schemas — 30 events emitted by the agent loop and
//! orchestration layer (M5 owner; M3-06 schema lock).
//!
//! Spec §7 line 757-762. Field sets sourced from the M3-02 plan reference
//! and the `claude-code` @ 6a25909 agent-loop emitters. User-derived string
//! fields are typed [`Verified`](crate::Verified); truncated user content
//! is [`PiiTagged`](crate::PiiTagged).

use crate::pii::{PiiTagged, Verified};
use serde::{Deserialize, Serialize};

// -- Event name constants (byte-locked) ---------------------------------------

/// `tengu_agent_started` — agent loop bootstrapped, before first turn.
pub const STARTED: &str = "tengu_agent_started";
/// `tengu_agent_completed` — agent loop exited normally (stop condition met).
pub const COMPLETED: &str = "tengu_agent_completed";
/// `tengu_agent_failed` — agent loop exited with an unrecoverable error.
pub const FAILED: &str = "tengu_agent_failed";
/// `tengu_agent_cancelled` — caller (or killswitch) cancelled the loop.
pub const CANCELLED: &str = "tengu_agent_cancelled";
/// `tengu_agent_turn_started` — fired before sending the model request for a turn.
pub const TURN_STARTED: &str = "tengu_agent_turn_started";
/// `tengu_agent_turn_completed` — fired after the turn's response/tool cycle finished.
pub const TURN_COMPLETED: &str = "tengu_agent_turn_completed";
/// `tengu_agent_turn_failed` — turn ended in an error (tool or API failure).
pub const TURN_FAILED: &str = "tengu_agent_turn_failed";
/// `tengu_agent_subagent_dispatched` — Task tool spawned a subagent.
pub const SUBAGENT_DISPATCHED: &str = "tengu_agent_subagent_dispatched";
/// `tengu_agent_subagent_completed` — dispatched subagent returned successfully.
pub const SUBAGENT_COMPLETED: &str = "tengu_agent_subagent_completed";
/// `tengu_agent_subagent_failed` — dispatched subagent errored.
pub const SUBAGENT_FAILED: &str = "tengu_agent_subagent_failed";
/// `tengu_agent_memory_loaded` — CLAUDE.md / memory files merged into prompt.
pub const MEMORY_LOADED: &str = "tengu_agent_memory_loaded";
/// `tengu_agent_system_prompt_built` — final system prompt was assembled.
pub const SYSTEM_PROMPT_BUILT: &str = "tengu_agent_system_prompt_built";
/// `tengu_agent_persona_resolved` — output style / persona was selected.
pub const PERSONA_RESOLVED: &str = "tengu_agent_persona_resolved";
/// `tengu_agent_compaction_triggered` — compaction decided to fire this turn.
pub const COMPACTION_TRIGGERED: &str = "tengu_agent_compaction_triggered";
/// `tengu_agent_compaction_completed` — compaction summary was applied to history.
pub const COMPACTION_COMPLETED: &str = "tengu_agent_compaction_completed";
/// `tengu_agent_compaction_failed` — compaction errored; the loop fell back to raw history.
pub const COMPACTION_FAILED: &str = "tengu_agent_compaction_failed";
/// `tengu_agent_resume_started` — `/resume` began restoring a prior session.
pub const RESUME_STARTED: &str = "tengu_agent_resume_started";
/// `tengu_agent_resume_completed` — resume succeeded; the loop is ready to continue.
pub const RESUME_COMPLETED: &str = "tengu_agent_resume_completed";
/// `tengu_agent_resume_failed` — resume errored (corrupt state, missing transcript, etc.).
pub const RESUME_FAILED: &str = "tengu_agent_resume_failed";
/// `tengu_agent_fork_started` — fork began (compaction summarizer or side-quest).
pub const FORK_STARTED: &str = "tengu_agent_fork_started";
/// `tengu_agent_fork_completed` — forked agent returned its summary/result.
pub const FORK_COMPLETED: &str = "tengu_agent_fork_completed";
/// `tengu_agent_fork_failed` — forked agent errored.
pub const FORK_FAILED: &str = "tengu_agent_fork_failed";
/// `tengu_agent_state_persisted` — agent state checkpoint was written to disk.
pub const STATE_PERSISTED: &str = "tengu_agent_state_persisted";
/// `tengu_agent_state_loaded` — agent state was rehydrated from disk.
pub const STATE_LOADED: &str = "tengu_agent_state_loaded";
/// `tengu_agent_state_load_failed` — state rehydration errored.
pub const STATE_LOAD_FAILED: &str = "tengu_agent_state_load_failed";
/// `tengu_agent_idle_timeout` — no activity within the idle window; loop self-cancelled.
pub const IDLE_TIMEOUT: &str = "tengu_agent_idle_timeout";
/// `tengu_agent_killswitch_activated` — killswitch tripped (user / engine / remote).
pub const KILLSWITCH_ACTIVATED: &str = "tengu_agent_killswitch_activated";
/// `tengu_agent_loop_iteration` — periodic loop-state snapshot (sampled).
pub const LOOP_ITERATION: &str = "tengu_agent_loop_iteration";
/// `tengu_agent_message_added` — a message was appended to the conversation history.
pub const MESSAGE_ADDED: &str = "tengu_agent_message_added";
/// `tengu_agent_message_truncated` — a message exceeded the per-message cap and was truncated.
pub const MESSAGE_TRUNCATED: &str = "tengu_agent_message_truncated";

/// Order-locked array of all 30 names; consumed by `tengu::ALL_EVENT_NAMES`.
pub(crate) const NAMES: &[&str] = &[
    STARTED,
    COMPLETED,
    FAILED,
    CANCELLED,
    TURN_STARTED,
    TURN_COMPLETED,
    TURN_FAILED,
    SUBAGENT_DISPATCHED,
    SUBAGENT_COMPLETED,
    SUBAGENT_FAILED,
    MEMORY_LOADED,
    SYSTEM_PROMPT_BUILT,
    PERSONA_RESOLVED,
    COMPACTION_TRIGGERED,
    COMPACTION_COMPLETED,
    COMPACTION_FAILED,
    RESUME_STARTED,
    RESUME_COMPLETED,
    RESUME_FAILED,
    FORK_STARTED,
    FORK_COMPLETED,
    FORK_FAILED,
    STATE_PERSISTED,
    STATE_LOADED,
    STATE_LOAD_FAILED,
    IDLE_TIMEOUT,
    KILLSWITCH_ACTIVATED,
    LOOP_ITERATION,
    MESSAGE_ADDED,
    MESSAGE_TRUNCATED,
];

// -- Payload structs (deny_unknown_fields locked) -----------------------------

/// Agent dispatch kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentKind {
    /// User-facing top-level agent.
    Main,
    /// Subagent dispatched via the Task tool.
    Subagent,
    /// Forked agent (compaction or side-quest).
    Forked,
}

/// Payload for [`STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Whether this is a main agent, a Task-spawned subagent, or a fork.
    pub agent_kind: AgentKind,
    /// Parent agent id for [`AgentKind::Subagent`] / [`AgentKind::Forked`].
    pub parent_agent_id: Option<Verified>,
    /// Session id this agent runs inside.
    pub session_id: Verified,
}

/// Payload for [`COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Total turns the agent ran.
    pub turns: u32,
    /// Wall-clock duration of the agent in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Final error message (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`CANCELLED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelledPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Why the loop was cancelled (e.g. `user`, `budget`, `hook_stop`).
    pub reason: Verified,
}

/// Payload for [`TURN_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnStartedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// 1-based turn number within the agent.
    pub turn: u32,
}

/// Payload for [`TURN_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnCompletedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// 1-based turn number within the agent.
    pub turn: u32,
    /// Wall-clock turn duration in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`TURN_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnFailedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// 1-based turn number within the agent.
    pub turn: u32,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`SUBAGENT_DISPATCHED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentDispatchedPayload {
    /// Parent agent that dispatched the subagent.
    pub parent_agent_id: Verified,
    /// Stable identifier for the spawned subagent.
    pub subagent_id: Verified,
    /// Tool that did the dispatching (typically `Task`).
    pub tool: Verified,
}

/// Payload for [`SUBAGENT_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentCompletedPayload {
    /// Parent agent that dispatched the subagent.
    pub parent_agent_id: Verified,
    /// Stable identifier for the spawned subagent.
    pub subagent_id: Verified,
    /// Wall-clock subagent duration in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`SUBAGENT_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentFailedPayload {
    /// Parent agent that dispatched the subagent.
    pub parent_agent_id: Verified,
    /// Stable identifier for the spawned subagent.
    pub subagent_id: Verified,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`MEMORY_LOADED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryLoadedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Number of memory files merged into the prompt.
    pub files_loaded: u32,
    /// Total bytes loaded across all memory files.
    pub total_bytes: u64,
}

/// Payload for [`SYSTEM_PROMPT_BUILT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemPromptBuiltPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Token count of the final system prompt.
    pub prompt_tokens: u64,
}

/// Payload for [`PERSONA_RESOLVED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaResolvedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Persona / output-style id selected for this agent.
    pub persona: Verified,
}

/// Payload for [`COMPACTION_TRIGGERED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionTriggeredPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// What caused compaction to fire.
    pub trigger: CompactionTrigger,
    /// Number of messages in history at trigger time.
    pub message_count: u32,
}

/// What caused compaction to fire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionTrigger {
    /// Context window pressure crossed the threshold.
    AutoThreshold,
    /// User ran `/compact`.
    Manual,
    /// Hooks-mandated compaction (`PreCompact` returned an action).
    Hook,
}

/// Payload for [`COMPACTION_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionCompletedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Number of pre-existing messages folded into the summary.
    pub messages_summarized: u32,
    /// Wall-clock duration of compaction in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`COMPACTION_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionFailedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`RESUME_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeStartedPayload {
    /// Stable identifier for this agent instance (the resumed one).
    pub agent_id: Verified,
    /// Session id being resumed from.
    pub from_session_id: Verified,
}

/// Payload for [`RESUME_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeCompletedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Number of historical turns restored from disk.
    pub turns_restored: u32,
}

/// Payload for [`RESUME_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeFailedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`FORK_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkStartedPayload {
    /// Agent that initiated the fork.
    pub parent_agent_id: Verified,
    /// Stable identifier for the forked agent.
    pub fork_agent_id: Verified,
}

/// Payload for [`FORK_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkCompletedPayload {
    /// Agent that initiated the fork.
    pub parent_agent_id: Verified,
    /// Stable identifier for the forked agent.
    pub fork_agent_id: Verified,
    /// Wall-clock duration of the fork in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`FORK_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkFailedPayload {
    /// Agent that initiated the fork.
    pub parent_agent_id: Verified,
    /// Stable identifier for the forked agent.
    pub fork_agent_id: Verified,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`STATE_PERSISTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatePersistedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Bytes written to the on-disk checkpoint.
    pub bytes_written: u64,
}

/// Payload for [`STATE_LOADED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateLoadedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Bytes read from the on-disk checkpoint.
    pub bytes_read: u64,
}

/// Payload for [`STATE_LOAD_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateLoadFailedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Failure description (whitelisted, no PII).
    pub error: Verified,
}

/// Payload for [`IDLE_TIMEOUT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdleTimeoutPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Seconds the agent was idle before timing out.
    pub idle_secs: u64,
}

/// Payload for [`KILLSWITCH_ACTIVATED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillswitchActivatedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Who tripped the killswitch.
    pub source: KillswitchSource,
}

/// Who tripped the killswitch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum KillswitchSource {
    /// User hit Ctrl-C / UI cancel.
    User,
    /// Engine policy (budget exceeded, hook returned `Stop`).
    Engine,
    /// Org-level remote killswitch (feature flag).
    Remote,
}

/// Payload for [`LOOP_ITERATION`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopIterationPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// 1-based loop iteration counter.
    pub iteration: u32,
    /// Free-form snapshot data (todo counts, tool stats, etc.). The audit
    /// macro whitelists `serde_json::Value` only for this exact field name.
    pub extra: serde_json::Value,
}

/// Payload for [`MESSAGE_ADDED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageAddedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// Speaker role for the appended message.
    pub role: MessageRole,
    /// Size of the appended message body in bytes.
    pub content_bytes: u64,
}

/// Message role for [`MessageAddedPayload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageRole {
    /// End-user input.
    User,
    /// Model output.
    Assistant,
    /// System / persona instructions.
    System,
    /// Tool result message.
    Tool,
}

/// Payload for [`MESSAGE_TRUNCATED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageTruncatedPayload {
    /// Stable identifier for this agent instance.
    pub agent_id: Verified,
    /// PII-tagged: truncated text fragment for diagnostic logging only.
    pub fragment: PiiTagged,
    /// Original message size in bytes (pre-truncation).
    pub original_bytes: u64,
    /// Retained size in bytes (post-truncation).
    pub kept_bytes: u64,
}
