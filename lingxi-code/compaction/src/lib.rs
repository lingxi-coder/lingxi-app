//! 5-layer compaction engine (Snip / Microcompact / `CachedMicrocompact` /
//! `ContextCollapse` / Autocompact) plus reactive PTL retry.
//!
//! See design spec §13 for the full architecture.
#![forbid(unsafe_code)]

pub mod autocompact;
pub mod boundary;
pub mod cached_microcompact;
pub mod context_collapse;
pub mod context_window;
pub mod grouping;
pub mod microcompact;
pub mod orchestrator;
pub mod post_compact;
pub mod prompt;
pub mod ptl_retry;
pub mod reactive;
pub mod session_memory;
pub mod snip;
pub mod thresholds;
pub mod warning_state;

pub use autocompact::{Autocompactor, CompactionError, CompactionResult};
pub use context_window::{
    context_window_for_model, max_output_tokens_for_model, CONTEXT_1M_BETA_HEADER,
    MODEL_CONTEXT_WINDOW_DEFAULT,
};
pub use boundary::{
    create_compact_boundary, find_last_compact_boundary_index, get_messages_after_compact_boundary,
    is_compact_boundary, CompactBoundaryMetadata, CompactTrigger, PreservedSegment,
    BOUNDARY_CONTENT,
};
pub use microcompact::{
    compactable_tools, reset_microcompact_state, MicrocompactResult, Microcompactor,
    TIME_BASED_MC_CLEARED_MESSAGE,
};
pub use orchestrator::{CompactionOrchestrator, IterationCompactionResult};
pub use post_compact::{is_main_thread_compact, run_post_compact_cleanup};
pub use warning_state::{
    clear_compact_warning_suppression, is_compact_warning_suppressed, suppress_compact_warning,
    CompactWarningState,
};
pub use prompt::{
    format_compact_summary, get_compact_prompt, get_compact_user_summary_message,
    BASE_COMPACT_PROMPT, NO_TOOLS_PREAMBLE, NO_TOOLS_TRAILER,
};
pub use snip::{SnipCompactor, SnipResult};
pub use thresholds::*;
