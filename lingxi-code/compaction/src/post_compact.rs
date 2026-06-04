//! Builds the post-compact message set: the boundary summary plus restored
//! file/skill attachments. Plans 09 (skills) and 10 (session) inject real data;
//! M1.7 ships the shape only.
//!
//! Also hosts [`run_post_compact_cleanup`] — the cache/state reset run after
//! every compaction (TS `src/services/compact/postCompactCleanup.ts`).

use crate::microcompact::reset_microcompact_state;
use crate::warning_state::clear_compact_warning_suppression;
use protocol::ConversationMessage;

/// Whether a compaction with this query source is a **main-thread** compact —
/// the gate that decides which module-level caches are safe to reset.
///
/// TS `isMainThreadCompact` (`postCompactCleanup.ts`): `querySource ===
/// undefined || querySource.startsWith('repl_main_thread') || querySource ===
/// 'sdk'`. Subagents (`agent:*`) share module-level state with the main thread,
/// so resetting it from a subagent compact would corrupt the main thread.
#[must_use]
pub fn is_main_thread_compact(query_source: Option<&str>) -> bool {
    match query_source {
        None => true,
        Some(s) => s.starts_with("repl_main_thread") || s == "sdk",
    }
}

/// Run cleanup of caches and tracking state after compaction.
///
/// TS ref: `src/services/compact/postCompactCleanup.ts` (full file). Call this
/// after both auto-compact and manual `/compact` to free memory held by
/// tracking structures that compaction invalidates.
///
/// `query_source` is the **string** label of the compacting query (TS
/// `QuerySource`, e.g. `"repl_main_thread"`, `"sdk"`, `"agent:foo"`). We pass
/// the string here — not the structured [`sidequery::QuerySource`] enum —
/// because the TS main-thread gate is a literal `startsWith`/equality check on
/// that string and the enum does not model the `repl_main_thread*` / `agent:*`
/// namespaces. Pass `None` only for callers that are genuinely
/// main-thread-only (`/compact`, `/clear`).
///
/// 1:1 fidelity ("close" per the batch spec): the main-thread gate and the
/// resets whose Rust counterparts exist are byte-faithful; the many TS cache
/// resets with **no Rust equivalent** are documented inline rather than ported.
///
/// We intentionally do NOT clear invoked-skill content here — skill content
/// must survive across compactions so post-compact attachments can re-include
/// the full skill text (TS note + Batch 5).
pub fn run_post_compact_cleanup(query_source: Option<&str>) {
    // Subagents (`agent:*`) run in the same process and share module-level
    // state with the main thread. Only reset main-thread module-level state for
    // main-thread compacts. Same `startsWith` pattern as TS `isMainThread`.
    let is_main_thread_compact = is_main_thread_compact(query_source);

    // resetMicrocompactState() — Rust microcompact layer is stateless (no-op).
    reset_microcompact_state();

    // TS: if feature('CONTEXT_COLLAPSE') && isMainThreadCompact ->
    // resetContextCollapse(). The Rust context-collapse layer
    // (`crate::context_collapse`) is stateless (pure functions over passed-in
    // history), so there is no module-level store to reset.
    if is_main_thread_compact {
        // TS postCompactCleanup.ts: resetContextCollapse — no Rust module state
        // to reset (context_collapse is stateless).

        // TS: getUserContext.cache.clear() + resetGetMemoryFilesCache('compact').
        // These memory-file caches live in the orchestrator/session layer, not
        // in this crate. Cross-crate wiring is flagged BLOCKED for this batch;
        // the orchestrator owns the actual cache clear at the call site.
        // TS postCompactCleanup.ts: getUserContext.cache.clear — no Rust
        //   equivalent in this crate (orchestrator-owned).
        // TS postCompactCleanup.ts: resetGetMemoryFilesCache('compact') — no
        //   Rust equivalent in this crate (orchestrator/session-owned).
    }

    // clearCompactWarningSuppression is NOT called here in TS post-compact
    // cleanup — TS clears suppression at the *start* of a new attempt and
    // *suppresses* after success. We mirror that elsewhere
    // (warning_state::{suppress_compact_warning, clear_compact_warning_suppression}).
    // Referenced here only to keep the symbol live for the orchestrator wiring;
    // see warning_state.rs for the suppress/clear contract.
    let _ = clear_compact_warning_suppression;

    // The remaining TS resets have no Rust equivalent in this crate:
    // TS postCompactCleanup.ts: clearSystemPromptSections — no Rust equivalent.
    // TS postCompactCleanup.ts: clearClassifierApprovals — no Rust equivalent.
    // TS postCompactCleanup.ts: clearSpeculativeChecks (Bash permissions) — no
    //   Rust equivalent.
    // TS postCompactCleanup.ts: resetSentSkillNames — intentionally NOT called
    //   (re-injecting skill_listing post-compact is pure cache_creation; see
    //   the TS rationale).
    // TS postCompactCleanup.ts: clearBetaTracingState — no Rust equivalent.
    // TS postCompactCleanup.ts: sweepFileContentCache (COMMIT_ATTRIBUTION) — no
    //   Rust equivalent.
    // TS postCompactCleanup.ts: clearSessionMessagesCache — session-storage
    //   cache lives in the session crate, not this one (orchestrator-owned).
}

/// Output of [`PostCompactBuilder::build`].
#[derive(Debug, Clone)]
pub struct PostCompactMessages {
    /// Messages to insert at the compact boundary (typically one system msg).
    pub summary_messages: Vec<ConversationMessage>,
    /// Untyped attachment payloads (filled in by later plans).
    pub attachments: Vec<serde_json::Value>,
}

/// Stateless builder for the post-compact boundary.
pub struct PostCompactBuilder;

impl PostCompactBuilder {
    /// Restore recent files + active skills attachments. Plans 09 (skills) /
    /// 10 (session) inject real data; this M1.7 ships the shape.
    #[must_use]
    pub fn build(summary_text: &str) -> PostCompactMessages {
        PostCompactMessages {
            summary_messages: vec![ConversationMessage::System {
                id: protocol::MessageId::new(),
                content: format!("Compact boundary:\n{summary_text}"),
            }],
            attachments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_thread_gate_true_for_none() {
        // `/compact`, `/clear` pass no source — genuinely main-thread-only.
        assert!(is_main_thread_compact(None));
    }

    #[test]
    fn main_thread_gate_true_for_repl_main_thread_prefix() {
        assert!(is_main_thread_compact(Some("repl_main_thread")));
        // startsWith, not equality — suffixed variants still gate true.
        assert!(is_main_thread_compact(Some("repl_main_thread_foreground")));
        assert!(is_main_thread_compact(Some("repl_main_thread:bash")));
    }

    #[test]
    fn main_thread_gate_true_for_sdk() {
        assert!(is_main_thread_compact(Some("sdk")));
    }

    #[test]
    fn main_thread_gate_false_for_agent_sources() {
        // Subagents share module-level state; resetting it would corrupt the
        // main thread, so the gate must be false.
        assert!(!is_main_thread_compact(Some("agent:foo")));
        assert!(!is_main_thread_compact(Some("agent:explore")));
        assert!(!is_main_thread_compact(Some("agent")));
    }

    #[test]
    fn main_thread_gate_false_for_other_sources() {
        assert!(!is_main_thread_compact(Some("sdk_subagent")));
        assert!(!is_main_thread_compact(Some("")));
        assert!(!is_main_thread_compact(Some("classifier")));
    }

    #[test]
    fn run_post_compact_cleanup_main_thread_does_not_panic() {
        // No Rust module-level caches to assert on (most resets are
        // cross-crate / stateless); exercise both branches for coverage.
        run_post_compact_cleanup(None);
        run_post_compact_cleanup(Some("repl_main_thread"));
        run_post_compact_cleanup(Some("sdk"));
    }

    #[test]
    fn run_post_compact_cleanup_subagent_does_not_panic() {
        run_post_compact_cleanup(Some("agent:foo"));
        run_post_compact_cleanup(Some("classifier"));
    }
}
