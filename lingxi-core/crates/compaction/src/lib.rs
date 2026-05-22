//! 5-layer compaction engine (Snip / Microcompact / `CachedMicrocompact` /
//! `ContextCollapse` / Autocompact) plus reactive PTL retry.
//!
//! See design spec §13 for the full architecture.
#![forbid(unsafe_code)]

pub mod thresholds;

pub use thresholds::*;
