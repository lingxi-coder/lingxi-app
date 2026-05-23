//! Relevance ranking with `#![deny(clippy::float_arithmetic)]`.
//! Filled in Tasks 8-10.

#![deny(clippy::float_arithmetic)]

use lingxi_protocol::MemoryEntry;

/// Inputs to `find_relevant`. Bundled so the public signature stays
/// stable as we add fields (e.g. recent tools, agent type) in M5.
#[derive(Debug)]
pub struct RelevanceInputs<'a> {
    /// Prompt text — keyword overlap basis.
    pub prompt: &'a str,
    /// Top-k cutoff. `None` → `DEFAULT_RELEVANT_MEMORIES` (5).
    pub k: Option<usize>,
    /// Whether team boost applies (mirrors `settings.team_memory.enabled`).
    pub team_boost_enabled: bool,
}

/// Placeholder; real impl in Tasks 8-10.
#[must_use]
pub fn find_relevant(
    _entries: &[MemoryEntry],
    _inputs: &RelevanceInputs<'_>,
) -> Vec<MemoryEntry> {
    Vec::new()
}
