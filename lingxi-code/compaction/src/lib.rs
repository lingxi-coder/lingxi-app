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
pub mod threshold_calc;
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
    collect_compactable_tool_ids, compactable_tools, evaluate_time_based_trigger,
    reset_microcompact_state, MicrocompactResult, Microcompactor, TimeBasedMCConfig,
    TimeBasedTrigger, TIME_BASED_MC_CLEARED_MESSAGE,
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
pub use ptl_retry::{truncate_head_for_ptl_retry, PTL_RETRY_MARKER};
pub use snip::{SnipCompactor, SnipResult};
pub use thresholds::*;
// Pure threshold kernels (Batch 3). `threshold_calc::auto_compact_threshold`
// takes a precomputed effective window and is reachable via the module path; it
// is intentionally NOT glob-re-exported to avoid shadowing the model-aware
// `thresholds::auto_compact_threshold`.
pub use threshold_calc::{effective_context_window, should_auto_compact};
