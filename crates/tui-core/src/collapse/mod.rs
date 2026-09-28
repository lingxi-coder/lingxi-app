//! Client-side streaming fold of a contiguous run of read/search/list tool
//! uses into one collapsed cell — port of claude-code
//! `utils/collapseReadSearch.ts`. See the design doc
//! `docs/superpowers/specs/2026-07-07-tui-collapsed-read-search-fold-design.md`.
//!
//! The transcript commits each scrollback cell exactly once and cannot
//! un-commit, so the whole-array batch transform of claude-code is ported as an
//! incremental accumulator ([`group::CollapseGroup`]): one group lives in the
//! transcript's single mutable active slot while streaming and is committed once
//! on a breaker. The output is identical because claude-code's groups are
//! already contiguous runs sealed by the same breakers.

pub use client_presentation::classify;
pub mod group;

pub use classify::{classify, SearchOrReadResult};
pub use group::{search_read_summary_text, search_read_summary_text_full, CollapseGroup};
