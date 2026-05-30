//! Content replacement state — tracks per-turn cleared tool results.

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
