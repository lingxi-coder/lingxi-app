//! 5-layer compaction engine (Snip / Microcompact / `CachedMicrocompact` /
//! `ContextCollapse` / Autocompact) plus reactive PTL retry.
//!
//! See design spec §13 for the full architecture.
#![forbid(unsafe_code)]

pub mod autocompact;
pub mod cached_microcompact;
pub mod context_collapse;
pub mod grouping;
pub mod microcompact;
pub mod orchestrator;
pub mod post_compact;
pub mod ptl_retry;
pub mod reactive;
pub mod session_memory;
pub mod snip;
pub mod thresholds;

pub use autocompact::{Autocompactor, CompactionError, CompactionResult};
pub use microcompact::{
    compactable_tools, MicrocompactResult, Microcompactor, TIME_BASED_MC_CLEARED_MESSAGE,
};
pub use orchestrator::{CompactionOrchestrator, IterationCompactionResult};
pub use snip::{SnipCompactor, SnipResult};
pub use thresholds::*;
