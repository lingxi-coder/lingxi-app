//! 5-layer compaction engine (Snip / Microcompact / `CachedMicrocompact` /
//! `ContextCollapse` / Autocompact) plus reactive PTL retry.
//!
//! See design spec §13 for the full architecture.
#![forbid(unsafe_code)]

pub mod autocompact;
pub mod boundary;
pub mod prompt_too_long;
pub mod cached_microcompact;
pub mod context_collapse;
pub mod context_window;
pub mod grouping;
pub mod microcompact;
pub mod orchestrator;
pub mod partial;
pub mod post_compact;
pub mod prompt;
pub mod ptl_retry;
pub mod reactive;
pub mod session_memory;
pub mod snip;
pub mod strip_media;
pub mod threshold_calc;
pub mod thresholds;
pub mod token_warning_banner;
pub mod warning_state;

pub use autocompact::{Autocompactor, CompactionError, CompactionResult};
pub use context_window::{
    context_window_for_model, max_output_tokens_for_model, max_thinking_tokens_for_model,
    CONTEXT_1M_BETA_HEADER, MODEL_CONTEXT_WINDOW_DEFAULT,
};
pub use boundary::{
    create_compact_boundary, create_compact_boundary_with_preserved_tail,
    find_last_compact_boundary_index, get_messages_after_compact_boundary, is_compact_boundary,
    preserved_segment_for_tail, CompactBoundaryMetadata, CompactTrigger, PreservedSegment,
    BOUNDARY_CONTENT,
};
pub use partial::{
    select_preserved_tail, zero_preserved_tail_usage, zero_preserved_usage, PreservedTailSplit,
};
pub use microcompact::{
    collect_compactable_tool_ids, compactable_tools, evaluate_time_based_trigger,
    reset_microcompact_state, MicrocompactResult, Microcompactor, TimeBasedMCConfig,
    TimeBasedTrigger, TIME_BASED_MC_CLEARED_MESSAGE,
};
pub use orchestrator::{CompactionOrchestrator, IterationCompactionResult};
pub use post_compact::{
    estimate_content_tokens, is_main_thread_compact, restore_post_compact_files,
    restore_post_compact_skills, run_post_compact_cleanup, truncate_skill_content,
    FileRestoreCandidate, PostCompactBuilder, PostCompactMessages, RestoredFile, RestoredSkill,
    SkillRestoreCandidate, SKILL_TRUNCATION_MARKER,
};
pub use warning_state::{
    clear_compact_warning_suppression, is_compact_warning_suppressed, suppress_compact_warning,
    CompactWarningState,
};
pub use token_warning_banner::{token_warning_banner, TokenWarningBanner, TokenWarningColor};
pub use prompt::{
    format_compact_summary, get_compact_prompt, get_compact_user_summary_message,
    BASE_COMPACT_PROMPT, NO_TOOLS_PREAMBLE, NO_TOOLS_TRAILER,
};
pub use ptl_retry::{truncate_head_for_ptl_retry, PTL_RETRY_MARKER};
pub use prompt_too_long::{
    is_prompt_too_long_body, parse_prompt_too_long_token_counts, prompt_too_long_token_gap,
    PROMPT_TOO_LONG_ERROR_MESSAGE,
};
pub use snip::{SnipCompactor, SnipResult};
pub use thresholds::*;
// Pure threshold kernels (Batch 3). `threshold_calc::auto_compact_threshold`
// takes a precomputed effective window and is reachable via the module path; it
// is intentionally NOT glob-re-exported to avoid shadowing the model-aware
// `thresholds::auto_compact_threshold`.
pub use threshold_calc::{
    compaction_prefix_overflow, effective_context_window, should_auto_compact, PrefixOverflow,
};
