//! Orchestrator-level traits — the public surface that slash commands
//! (via `SlashContext`, M5-09) and the CLI binary (M5-12) consume.
//!
//! The concrete `ConversationOrchestrator` lives in `lingxi-orchestrator`;
//! these traits live here so consumers can depend on them without pulling
//! in the orchestrator (preserves the leaf position of `lingxi-traits`).
//!
//! See spec §2.3 (key traits) for the matched design.

use async_trait::async_trait;
use protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Snapshot of cumulative cost at a single point in time.
///
/// Lightweight echo of `cost::SessionCostSummary` — see that type for
/// the canonical session-scope rollup. We keep a leaf-friendly mirror here
/// so `lingxi-traits` does not need to depend on `lingxi-cost`.
///
/// M5-11 added the `total_usd`, `input_tokens`, `output_tokens`, `api_calls`,
/// and `session_duration` fields used by the `/cost` slash command's
/// locked render template. The legacy `total_nano_usd` and `total_tokens`
/// fields remain for back-compat with M5-02's `EndTurn` output event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostSnapshot {
    /// Session whose cost this snapshot describes.
    pub session_id: SessionId,
    /// Cumulative cost in nano-USD (legacy field — still consumed by `EndTurn`).
    pub total_nano_usd: u64,
    /// Cumulative tokens (input + output across all models — legacy field).
    pub total_tokens: u64,
    /// Cumulative cost in USD (4-decimal precision in displays).
    #[serde(default)]
    pub total_usd: f64,
    /// Cumulative input tokens across all turns.
    #[serde(default)]
    pub input_tokens: u64,
    /// Cumulative output tokens across all turns.
    #[serde(default)]
    pub output_tokens: u64,
    /// Cumulative successful `messages_create` calls.
    #[serde(default)]
    pub api_calls: u32,
    /// Elapsed time since the session started.
    #[serde(default)]
    pub session_duration: std::time::Duration,
}

/// Result of a `force_compact` operation. M5-10 wires `/compact` against
/// this surface; M5-02 only defines the type for forward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSummary {
    /// Number of messages in the session BEFORE compaction.
    pub messages_before: u32,
    /// Number of messages in the session AFTER compaction.
    pub messages_after: u32,
    /// Approximate bytes saved (summary token count delta × 4, as a UX
    /// estimate — exact accounting lives in `lingxi-compaction`).
    pub bytes_saved: u64,
}

/// Errors surfaced through the orchestrator's public handle.
///
/// Distinct from `orchestrator::OrchestratorError` because the
/// handle surface deliberately hides the API-error variants from slash
/// command authors (they cannot meaningfully act on a 429). Implementations
/// MAY wrap `OrchestratorError` and project a coarse `HandleError`.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum HandleError {
    /// The requested action could not be completed (e.g. `clear` during a
    /// turn-in-flight). The payload is a human-readable reason.
    #[error("handle action failed: {0}")]
    ActionFailed(String),
    /// The requested operation is not implemented on this handle.
    /// Used by the default `OrchestratorHandle::run_turn_streaming_with_cancel`
    /// impl (M6-03) so existing handle implementations (M5-13 stdio REPL
    /// path) don't need to override.
    #[error("operation not implemented: {0}")]
    Unimplemented(String),
}

/// Outcome of a TUI-driven turn invoked via
/// [`OrchestratorHandle::run_turn_streaming_with_cancel`]. (M6-03)
///
/// Mirrors `orchestrator::TurnOutcome` so the trait surface in
/// `lingxi-traits` does not depend on the orchestrator crate. Map between
/// the two in `orchestrator::handle_impl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Model returned a natural stop reason and the turn loop ended.
    EndTurn,
    /// The orchestrator's `max_turns` budget was reached before `end_turn`.
    MaxTurns,
    /// The cancel token fired mid-turn; the orchestrator unwound the
    /// current API call and returned early.
    Cancelled,
}

/// Result of [`OrchestratorHandle::open_memory_editor`] (M5-10).
///
/// Returned to `/memory`'s handler so it can render the locked
/// `"Edited {path} (exit {code})."` template. Carries the path the editor
/// was launched against (which may have been created if absent) and the
/// editor process's exit code (0 on success; non-zero on user cancel /
/// editor error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEditorOutcome {
    /// The CLAUDE.md path that was edited (may have been created if absent).
    pub edited_path: PathBuf,
    /// Exit code of the spawned `$EDITOR` process. 0 = success.
    pub exit_code: i32,
}

// ────────────────────────────────────────────────────────────────────────────
// M5-11 info structs (used by `/mcp`, `/hooks`, `/agents`, `/status`, `/doctor`)
// ────────────────────────────────────────────────────────────────────────────

/// One MCP server entry returned by [`OrchestratorHandle::list_mcp_servers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInfo {
    /// Server name as registered in settings.
    pub name: String,
    /// Connection status at snapshot time.
    pub status: McpStatus,
    /// Transport kind: `"stdio"`, `"sse"`, or `"http"`.
    pub transport: String,
}

/// Connection status for an MCP server in [`McpServerInfo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// Connected and healthy.
    Connected,
    /// Disconnected — either never connected or cleanly shut down.
    Disconnected,
    /// Connection failed with the wrapped reason.
    Error(String),
}

/// One hook entry returned by [`OrchestratorHandle::list_hooks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInfo {
    /// Hook identifier.
    pub name: String,
    /// Hook event (e.g. `"PreToolUse"`, `"PostToolUse"`, `"Stop"`, `"Notification"`).
    pub event: String,
    /// Optional matcher regex (tool-name pattern).
    pub matcher: Option<String>,
    /// Timeout in milliseconds (default `60_000` if unset).
    pub timeout_ms: u64,
}

/// One subagent entry returned by [`OrchestratorHandle::list_agents`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    /// Agent name (matches the markdown filename without extension).
    pub name: String,
    /// Human-readable description (may be truncated by callers).
    pub description: String,
    /// Tool allow-list (empty = all tools).
    pub tools_allowed: Vec<String>,
}

/// Aggregate diagnostic report returned by [`OrchestratorHandle::run_doctor_checks`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    /// Individual check results in execution order.
    pub checks: Vec<DoctorCheck>,
    /// Summary tallies (pass/warn/fail counts).
    pub summary: DoctorSummary,
}

/// One `/doctor` check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    /// Check identifier (`"config-dir"`, `"api-key"`, etc.).
    pub name: String,
    /// Pass/warn/fail outcome.
    pub status: CheckStatus,
    /// Optional detail string (rendered on a second indented line if `Some`).
    pub detail: Option<String>,
}

/// Outcome of a single [`DoctorCheck`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Check succeeded.
    Pass,
    /// Check produced a warning (non-fatal anomaly).
    Warn,
    /// Check failed.
    Fail,
}

/// Pass/warn/fail tallies in a [`DoctorReport`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorSummary {
    /// Number of checks that returned [`CheckStatus::Pass`].
    pub passed: u32,
    /// Number of checks that returned [`CheckStatus::Warn`].
    pub warnings: u32,
    /// Number of checks that returned [`CheckStatus::Fail`].
    pub failed: u32,
}

/// Snapshot returned by [`OrchestratorHandle::get_status_snapshot`] for the
/// `/status` panel. All fields are populated synchronously at snapshot time.
///
/// Note: `Eq` is intentionally not derived because `total_cost_usd: f64` does
/// not implement `Eq`. Use `PartialEq` for assertions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusSnapshot {
    /// Current session id (as a stable string for display).
    pub session_id: String,
    /// Active model name (e.g. `"claude-opus-4-7"`).
    pub model: String,
    /// Total messages in the session history.
    pub n_messages: u32,
    /// Cumulative cost in USD.
    pub total_cost_usd: f64,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// MCP servers currently in `Connected` state.
    pub n_mcp_connected: u32,
    /// MCP servers configured (any state).
    pub n_mcp_total: u32,
    /// Hooks registered.
    pub n_hooks: u32,
    /// Subagents available.
    pub n_agents: u32,
    /// Session start time, RFC 3339 (`"YYYY-MM-DDTHH:MM:SSZ"`, UTC).
    pub started_at: String,
    /// Working directory used to launch the session.
    pub cwd: PathBuf,
}

/// Public handle to the orchestrator that slash commands operate against.
///
/// Wired in M5-09 (slash-command surface). M5-02 only defines the trait —
/// `ConversationOrchestrator` does NOT yet implement it.
#[async_trait]
pub trait OrchestratorHandle: Send + Sync {
    /// The session id currently driving the conversation.
    async fn current_session_id(&self) -> SessionId;

    /// Clear the in-memory session and start fresh.
    async fn clear_session(&self) -> Result<(), HandleError>;

    /// Force a compaction pass and return the summary.
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError>;

    /// Snapshot the cumulative cost.
    async fn snapshot_cost(&self) -> CostSnapshot;

    /// Switch the active model. Subsequent turns use the new model.
    async fn switch_model(&self, model: &str) -> Result<(), HandleError>;

    // M5-10 additions:

    /// Set the orchestrator's internal `should_exit` flag.
    ///
    /// The REPL (M5-13) checks this after each turn and breaks out of the
    /// loop. The flag is one-way: once set, it cannot be cleared (so a
    /// double-`/exit` is idempotent).
    ///
    /// Wired by M5-10 (`/exit` handler).
    async fn request_exit(&self);

    /// Read the current value of the `should_exit` flag without mutating it.
    ///
    /// The REPL (M5-13) calls this after every slash-command dispatch to
    /// decide whether to break the loop. Returns `true` iff `request_exit`
    /// has been called at least once.
    async fn current_should_exit(&self) -> bool;

    /// Open `$EDITOR` on `<config-dir>/claude/CLAUDE.md` (creating the file
    /// if it does not exist), block until the editor exits, then return the
    /// outcome.
    ///
    /// The editor lookup order is `EDITOR` → `VISUAL` → `"vi"` (Unix) /
    /// `"notepad.exe"` (Windows). Empty env values are treated as missing.
    ///
    /// Wired by M5-10 (`/memory` handler).
    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>;

    // M5-11 additions:

    /// Enumerate currently registered MCP servers + their connection state.
    /// Used by `/mcp` and `/status`. Returns an empty vector when no MCP
    /// servers are configured.
    async fn list_mcp_servers(&self) -> Vec<McpServerInfo>;

    /// Enumerate registered hooks (built-in + user). Used by `/hooks` and
    /// `/status`.
    async fn list_hooks(&self) -> Vec<HookInfo>;

    /// Enumerate registered subagents (markdown-defined + built-in). Used
    /// by `/agents` and `/status`.
    async fn list_agents(&self) -> Vec<AgentInfo>;

    /// Run the 6 doctor checks and return the aggregated report. Used by
    /// `/doctor`.
    async fn run_doctor_checks(&self) -> DoctorReport;

    /// Snapshot the full status panel. Used by `/status`.
    async fn get_status_snapshot(&self) -> StatusSnapshot;

    /// Open `$EDITOR` on `<config-dir>/claude/config.json` (creating if
    /// absent). Used by `/config`.
    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Open `$EDITOR` on `<config-dir>/claude/permissions.json` (creating
    /// if absent). Used by `/permissions`.
    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Enumerate model names the orchestrator will accept via
    /// [`Self::switch_model`]. Used by `/model` (no-arg list mode).
    async fn list_available_models(&self) -> Vec<String>;

    // M6-03 addition:

    /// Streaming twin of the M5-13 cancel-aware turn entry point. The
    /// TUI calls this so Ctrl-C aborts the in-flight SSE stream cleanly.
    ///
    /// Default impl returns `Err(HandleError::Unimplemented(..))` so
    /// existing stdio REPL implementations (M5-13) need no override.
    /// `OrchestratorHandleImpl` (M6-03) overrides this to delegate to
    /// `ConversationOrchestrator::run_turn_streaming_with_cancel`.
    async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnOutcome, HandleError> {
        let _ = (prompt, cancel);
        Err(HandleError::Unimplemented(
            "run_turn_streaming_with_cancel".into(),
        ))
    }

    /// Streaming turn carrying pasted image file paths (TUI paste→image). Each
    /// path is read + base64-encoded into a `ContentBlock::Image` on the
    /// outgoing user message.
    ///
    /// Default delegates to [`Self::run_turn_streaming_with_cancel`] ignoring
    /// images, so non-TUI handle impls need no override.
    async fn run_turn_streaming_with_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnOutcome, HandleError> {
        let _ = image_paths;
        self.run_turn_streaming_with_cancel(prompt, cancel).await
    }

    // ────────────────────────────────────────────────────────────────────
    // engine-data-commands additions (`/export`, `/files`, `/context`,
    // `/resume`). Each carries a benign default so the production
    // `ConversationOrchestrator` (orchestrator/src/handle_impl.rs) and the
    // test `MockOrchestratorHandle` (orchestrator/src/test_support.rs) keep
    // compiling unchanged; the production impl overrides them with real data.
    // ────────────────────────────────────────────────────────────────────

    /// Clone of the live, ordered conversation history.
    ///
    /// Single read-only accessor backing `/export` (render the transcript to
    /// a file) and underpinning `/summary` and `/diff`. Returns the already
    /// public [`protocol::ConversationMessage`] so no new type is introduced.
    ///
    /// Default returns an empty `Vec`, so handle impls that do not track a
    /// session (e.g. the test mock) need no override.
    async fn conversation_transcript(&self) -> Vec<protocol::ConversationMessage> {
        Vec::new()
    }

    /// File paths currently tracked in the session's read-file-state cache.
    ///
    /// Backs `/files`, which renders each path relative to the cwd (cwd comes
    /// from [`Self::get_status_snapshot`]) and prints `"No files in context"`
    /// when empty — 1:1 with `files.ts`.
    ///
    /// Default returns an empty `Vec`, matching the TS "No files in context"
    /// branch when no read-file-state cache is wired.
    async fn files_in_context(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// `(used_tokens, max_tokens)` for the current context window.
    ///
    /// Minimal primitive-tuple accessor backing the `/context` flat panel
    /// (`**Tokens:** {used} / {max} ({pct}%)`). `used_tokens` comes from the
    /// session's cumulative usage; `max_tokens` from the active model's
    /// context budget. The model name itself already comes from
    /// [`Self::get_status_snapshot`], so no new struct is needed.
    ///
    /// Default returns `(0, 0)` when no usage is recorded.
    async fn context_window_usage(&self) -> (u64, u64) {
        (0, 0)
    }

    /// `Vec<(session_id, label)>` of prior on-disk sessions, newest-first.
    ///
    /// Single accessor for a non-interactive `/resume` listing (one
    /// `id` + `label` per line), built from `std` types only. The interactive
    /// picker and replaying a chosen session are handled elsewhere; this
    /// delivers only the enumeration half.
    ///
    /// Default returns an empty `Vec` when no on-disk store is present.
    async fn list_resumable_sessions(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Captured output emission. Useful for tests and (M5-13) the stdio sink.
///
/// The enum is `non_exhaustive` so M5-04 can add a `StreamingDelta` variant
/// without a breaking change.
///
/// M5-11: `Eq` was dropped (and replaced with `PartialEq` only) because
/// `CostSnapshot` now carries `f64` + `Duration` fields whose `Eq` impl is
/// not defined. Callers that need set semantics should bucket by the
/// `session_id` or other integer fields instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Plain text from the assistant.
    Text {
        /// The text payload emitted.
        text: String,
    },
    /// A tool invocation about to dispatch.
    ToolCall {
        /// Stable id (the `tool_use_id` echoed in the matching `ToolResult`).
        /// Added in M6-04 so the TUI can correlate calls with results and
        /// key the per-tool expanded-state map.
        id: protocol::ToolUseId,
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// A tool result returning to the conversation.
    ToolResult {
        /// Correlator with the matching `ToolCall`.
        id: protocol::ToolUseId,
        /// Name of the tool that returned.
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
    },
    /// End-of-turn marker with cost.
    EndTurn {
        /// Stop reason reported by the model (e.g. `"end_turn"`, `"max_tokens"`).
        stop_reason: String,
        /// Cumulative cost snapshot at end of turn.
        cost: CostSnapshot,
    },
    /// Emitted once a successful `force_compact` finishes. (M6-08)
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction (including the appended
        /// `[Compacted]` boundary marker).
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
    },
    /// A streaming reasoning ("thinking") delta as it arrives. (§0.7
    /// "light up thinking/usage"). Recorded by `MockOutputStream` so the
    /// orchestrator SSE-pump tests can assert `emit_thinking` fired. The
    /// `signature` is `None` for live deltas (it only arrives on the
    /// completed thinking block, not per-delta).
    Thinking {
        /// The reasoning fragment emitted.
        thinking: String,
        /// Cryptographic signature, `None` for live deltas.
        signature: Option<String>,
    },
    /// An incremental token-usage update for the latest API call. (§0.7
    /// "light up thinking/usage"). Recorded by `MockOutputStream` so the
    /// orchestrator SSE-pump tests can assert `emit_usage` fired with the
    /// right counts.
    Usage {
        /// Input tokens billed.
        input_tokens: u64,
        /// Output tokens billed.
        output_tokens: u64,
        /// Input tokens served from cache.
        cache_read_tokens: u64,
        /// Input tokens used to create a fresh cache entry.
        cache_creation_tokens: u64,
    },
}

/// Sink for orchestrator-emitted output events.
///
/// The stdio CLI (M5-12) and the future TUI (M6) both implement this.
/// M5-02 ships `MockOutputStream` (in `lingxi-orchestrator::test_support`)
/// for unit tests.
#[async_trait]
pub trait OutputStream: Send + Sync {
    /// Emit a piece of plain assistant text. In M5-02 this is called once
    /// per `Text` content block per turn (whole-body). M5-04 will switch
    /// to per-SSE-delta emission without changing this signature.
    async fn emit_text(&self, text: &str);

    /// Emit a tool-call notification immediately before dispatch.
    ///
    /// `id` is the `tool_use_id` echoed in the matching ToolResult. Added
    /// in M6-04 so consumers can correlate calls with results.
    async fn emit_tool_call(&self, id: &protocol::ToolUseId, tool: &str, input: &serde_json::Value);

    /// Emit a tool-result notification immediately after dispatch.
    async fn emit_tool_result(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        result: &serde_json::Value,
    );

    /// Emit the end-of-turn marker with the cost snapshot.
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);

    /// Emit a compaction-completed event. Default no-op for adapters
    /// that don't care (e.g. NDJSON sink may flush a one-line marker).
    /// (M6-08)
    async fn emit_compaction_completed(
        &self,
        _messages_before: u32,
        _messages_after: u32,
        _bytes_saved: u64,
    ) {
    }

    /// Emit a streaming reasoning ("thinking") delta as it arrives.
    ///
    /// Added by the §0.7 "light up thinking/usage" follow-up. Called once
    /// per `ThinkingDelta` SSE chunk from `event_router`. `signature` is
    /// `None` for live deltas — the cryptographic signature only arrives on
    /// the completed thinking block (`SignatureDelta`), not per-delta, so
    /// the live-delta path always passes `None`.
    ///
    /// **Default no-op**: pre-existing sinks (TUI, CLI, `MockOutputStream`)
    /// that don't render reasoning keep compiling unchanged. The
    /// client-adapter overrides this to surface a `ClientEvent::ThinkingDelta`.
    async fn emit_thinking(&self, _thinking: &str, _signature: Option<&str>) {}

    /// Emit an incremental token-usage update for the latest API call.
    ///
    /// Added by the §0.7 "light up thinking/usage" follow-up. Called from
    /// `event_router` when a `MessageDelta`/`MessageStart` SSE event carries
    /// a `usage` payload. Counts are passed as bare `u64`s (rather than a
    /// `cost::TokenUsage`) to keep `lingxi-traits` a leaf crate: `lingxi-cost`
    /// already depends on `lingxi-traits`, so a `cost` dependency here would
    /// form a cycle. The four arguments map field-for-field onto both
    /// `cost::TokenUsage` (caller side, in the orchestrator) and
    /// `ClientEvent::UsageUpdate` (adapter side).
    ///
    /// **Default no-op**: pre-existing sinks keep compiling unchanged. The
    /// client-adapter overrides this to surface a `ClientEvent::UsageUpdate`.
    async fn emit_usage(
        &self,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_read_tokens: u64,
        _cache_creation_tokens: u64,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn _trait_objects_compile() {
        fn _f<T: OutputStream + 'static>(t: T) -> Box<dyn OutputStream> {
            Box::new(t)
        }
        fn _g<T: OrchestratorHandle + 'static>(t: T) -> Box<dyn OrchestratorHandle> {
            Box::new(t)
        }
    }

    #[test]
    fn output_event_round_trips_through_json() {
        let ev = OutputEvent::Text {
            text: "hello".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    /// M6-04 Task 1: `OutputEvent::ToolCall` must carry a `ToolUseId` so the
    /// TUI can correlate calls with their results and key the per-tool
    /// expanded-state map.
    #[test]
    fn output_event_tool_call_carries_tool_use_id() {
        use protocol::ToolUseId;
        let id = ToolUseId::new();
        let ev = OutputEvent::ToolCall {
            id,
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    /// M6-04 Task 1: same for `OutputEvent::ToolResult`.
    #[test]
    fn output_event_tool_result_carries_tool_use_id() {
        use protocol::ToolUseId;
        let id = ToolUseId::new();
        let ev = OutputEvent::ToolResult {
            id,
            tool: "Read".into(),
            result: serde_json::json!({"content": "fn main() {}"}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn cost_snapshot_default_is_zero() {
        let s = CostSnapshot::default();
        assert_eq!(s.total_nano_usd, 0);
        assert_eq!(s.total_tokens, 0);
        assert!((s.total_usd - 0.0).abs() < f64::EPSILON);
        assert_eq!(s.api_calls, 0);
        assert_eq!(s.session_duration, std::time::Duration::ZERO);
    }

    #[test]
    fn compaction_summary_default_is_zero() {
        let s = CompactionSummary::default();
        assert_eq!(s.messages_before, 0);
        assert_eq!(s.messages_after, 0);
        assert_eq!(s.bytes_saved, 0);
    }

    // M5-11 trait extension tests

    #[allow(dead_code)]
    fn _handle_remains_object_safe_after_m5_11() {
        let _: Option<Box<dyn OrchestratorHandle>> = None;
    }

    #[test]
    fn mcp_server_info_fields() {
        let info = McpServerInfo {
            name: "memory".to_string(),
            status: McpStatus::Connected,
            transport: "stdio".to_string(),
        };
        assert_eq!(info.name, "memory");
        assert!(matches!(info.status, McpStatus::Connected));
        assert_eq!(info.transport, "stdio");
    }

    #[test]
    fn hook_info_fields() {
        let info = HookInfo {
            name: "fmt-on-write".to_string(),
            event: "PostToolUse".to_string(),
            matcher: Some("Write|Edit".to_string()),
            timeout_ms: 60_000,
        };
        assert_eq!(info.timeout_ms, 60_000);
        assert_eq!(info.matcher.as_deref(), Some("Write|Edit"));
    }

    #[test]
    fn agent_info_fields() {
        let info = AgentInfo {
            name: "reviewer".to_string(),
            description: "review code".to_string(),
            tools_allowed: vec!["Read".to_string(), "Grep".to_string()],
        };
        assert_eq!(info.tools_allowed.len(), 2);
    }

    #[test]
    fn doctor_report_default_is_empty() {
        let r = DoctorReport::default();
        assert_eq!(r.checks.len(), 0);
        assert_eq!(r.summary.passed, 0);
        assert_eq!(r.summary.warnings, 0);
        assert_eq!(r.summary.failed, 0);
    }

    #[test]
    fn status_snapshot_default_is_zero() {
        let s = StatusSnapshot::default();
        assert_eq!(s.n_messages, 0);
        assert_eq!(s.n_mcp_connected, 0);
        assert_eq!(s.n_mcp_total, 0);
        assert_eq!(s.n_hooks, 0);
        assert_eq!(s.n_agents, 0);
    }

    #[test]
    fn check_status_variants() {
        let pass = CheckStatus::Pass;
        let warn = CheckStatus::Warn;
        let fail = CheckStatus::Fail;
        assert_ne!(pass, warn);
        assert_ne!(warn, fail);
    }
}
