//! Content replacement state — tracks per-turn cleared tool results.
//!
//! Claude Code 2.1.245 threads an extra `storageV5` argument through
//! `insertContentReplacement` / content-replacement apply / subagent-exit
//! precompute. That store is Anthropic's remote session backend; this module
//! keeps the in-memory map only. The extra persist arg is a no-op without that
//! backend (same scope cut as `performCompactTranscriptV5`).
//!
//! ⚠️ SES-3, verified 2026-09-14: "in-memory map only" is generous. The state
//! is `None` at EVERY `ToolUseContext` construction site in the workspace, so
//! nothing in this port ever replaces a tool result — the map is not merely
//! unpersisted, it is never instantiated. The backlog asks for
//! `content-replacement` rows in ordinary sessions (today only `session::branch`
//! writes them, for fork/branch); persisting records of replacements that never
//! happen would write empty rows forever.
//!
//! The FEATURE underneath is the one worth building: clearing large tool
//! results from older turns to reclaim context, which is what populates this
//! map in the first place. The persistence is downstream of it, not a gap of
//! its own.

use protocol::ToolUseId;
use std::collections::HashMap;

/// Tracks replaced tool result content (large outputs cleared in older turns).
#[derive(Debug, Clone, Default)]
pub struct ContentReplacementState {
    /// Per-tool-use replacement records.
    pub replacements: HashMap<ToolUseId, ReplacementRecord>,
    /// Total character budget across the session.
    pub total_budget_chars: usize,
    /// Characters consumed by current results.
    pub used_chars: usize,
}

/// Record of one replaced tool result.
#[derive(Debug, Clone)]
pub struct ReplacementRecord {
    /// Original result size (chars).
    pub original_size: usize,
    /// Turn number when the result was replaced.
    pub replaced_at_turn: u32,
    /// Placeholder text shown in place of the original (e.g., `"[Old tool result content cleared]"`).
    pub placeholder: String,
}
