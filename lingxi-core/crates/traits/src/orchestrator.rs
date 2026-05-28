//! Orchestrator-level traits — the public surface that slash commands
//! (via `SlashContext`, M5-09) and the CLI binary (M5-12) consume.
//!
//! The concrete `ConversationOrchestrator` lives in `lingxi-orchestrator`;
//! these traits live here so consumers can depend on them without pulling
//! in the orchestrator (preserves the leaf position of `lingxi-traits`).
//!
//! See spec §2.3 (key traits) for the matched design.

use async_trait::async_trait;
use lingxi_protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Snapshot of cumulative cost at a single point in time.
///
/// Lightweight echo of `lingxi_cost::SessionCostSummary` — see that type for
/// the canonical session-scope rollup. We keep a leaf-friendly mirror here
/// so `lingxi-traits` does not need to depend on `lingxi-cost`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostSnapshot {
    /// Session whose cost this snapshot describes.
    pub session_id: SessionId,
    /// Cumulative cost in nano-USD.
    pub total_nano_usd: u64,
    /// Cumulative tokens (input + output across all models).
    pub total_tokens: u64,
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
/// Distinct from `lingxi_orchestrator::OrchestratorError` because the
/// handle surface deliberately hides the API-error variants from slash
/// command authors (they cannot meaningfully act on a 429). Implementations
/// MAY wrap `OrchestratorError` and project a coarse `HandleError`.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum HandleError {
    /// The requested action could not be completed (e.g. `clear` during a
    /// turn-in-flight). The payload is a human-readable reason.
    #[error("handle action failed: {0}")]
    ActionFailed(String),
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

    /// Open `$EDITOR` on `<config-dir>/claude/CLAUDE.md` (creating the file
    /// if it does not exist), block until the editor exits, then return the
    /// outcome.
    ///
    /// The editor lookup order is `EDITOR` → `VISUAL` → `"vi"` (Unix) /
    /// `"notepad.exe"` (Windows). Empty env values are treated as missing.
    ///
    /// Wired by M5-10 (`/memory` handler).
    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>;
}

/// Captured output emission. Useful for tests and (M5-13) the stdio sink.
///
/// The enum is `non_exhaustive` so M5-04 can add a `StreamingDelta` variant
/// without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Plain text from the assistant.
    Text {
        /// The text payload emitted.
        text: String,
    },
    /// A tool invocation about to dispatch.
    ToolCall {
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// A tool result returning to the conversation.
    ToolResult {
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
    async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value);

    /// Emit a tool-result notification immediately after dispatch.
    async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value);

    /// Emit the end-of-turn marker with the cost snapshot.
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);
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

    #[test]
    fn cost_snapshot_default_is_zero() {
        let s = CostSnapshot::default();
        assert_eq!(s.total_nano_usd, 0);
        assert_eq!(s.total_tokens, 0);
    }

    #[test]
    fn compaction_summary_default_is_zero() {
        let s = CompactionSummary::default();
        assert_eq!(s.messages_before, 0);
        assert_eq!(s.messages_after, 0);
        assert_eq!(s.bytes_saved, 0);
    }
}
