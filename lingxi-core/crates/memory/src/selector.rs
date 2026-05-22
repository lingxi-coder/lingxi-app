//! LLM-driven memory selector.
//!
//! The selector is invoked once per turn (in parallel with the main API
//! call via Plan 08's `SideQueryClient`) and returns a small set of
//! memory file paths to surface in the next prompt. This module ships
//! the skeleton; the side-query wiring lands in Plan 08.

use crate::file::{MemoryError, MemoryFile};
use std::collections::HashSet;
use std::path::PathBuf;

/// Selects which available memory files are relevant to the current turn.
pub struct MemorySelector {
    /// Model identifier used for the side query.
    pub selector_model: String,
    /// Hard cap on how many files are returned per call.
    pub max_selected: usize,
}

impl Default for MemorySelector {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySelector {
    /// Build a selector with engine defaults (Haiku-class model, 5 files).
    #[must_use]
    pub fn new() -> Self {
        Self {
            selector_model: "claude-haiku-4-5".into(),
            max_selected: 5,
        }
    }

    /// Pick a subset of `available` files relevant to `_query`.
    ///
    /// In Plan 08 this delegates to `SideQueryClient`; here it is a
    /// deterministic stub that picks files not already surfaced.
    #[allow(clippy::unused_async)] // stub — Plan 08 makes it truly async
    pub async fn select_relevant(
        &self,
        _query: &str,
        available: &[MemoryFile],
        _recent_tools: &[String],
        already: &HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, MemoryError> {
        Ok(available
            .iter()
            .filter(|m| !already.contains(&m.path))
            .take(self.max_selected)
            .map(|m| m.path.clone())
            .collect())
    }
}
