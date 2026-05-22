//! 5-layer compaction engine (Snip / Microcompact / `CachedMicrocompact` /
//! `ContextCollapse` / Autocompact) plus reactive PTL retry.
//!
//! See design spec §13 for the full architecture.
#![forbid(unsafe_code)]

pub mod microcompact;
pub mod snip;
pub mod thresholds;

pub use microcompact::{
    compactable_tools, MicrocompactResult, Microcompactor, TIME_BASED_MC_CLEARED_MESSAGE,
};
pub use snip::{SnipCompactor, SnipResult};
pub use thresholds::*;
