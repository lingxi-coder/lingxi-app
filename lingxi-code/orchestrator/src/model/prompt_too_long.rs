//! Prompt-too-long (413 / "prompt is too long") detection + token-gap parsing.
//!
//! The implementation has been moved to `compaction::prompt_too_long` so that
//! the compaction crate's PTL retry loop can use these helpers without creating
//! a dependency cycle (orchestrator → compaction is an existing edge; compaction
//! → orchestrator would be a cycle).
//!
//! This module re-exports all the public items from `compaction::prompt_too_long`
//! so that orchestrator-internal consumers (`turn_loop`, `conversation`, etc.) can
//! continue importing from the familiar path
//! `crate::model::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE` without any
//! changes.
//!
//! ## Dropped functions (not re-exported)
//!
//! `classify_prompt_too_long` and `reclassify_prompt_too_long` from
//! `api-client/src/prompt_too_long.rs` are NOT re-exported here. Both functions
//! return `ApiError`-typed values (`ApiError::PromptTooLong`), which is an
//! `api-client`-local type. In the `orchestrator`/`llm-client` world this
//! classification is superseded by `llm_client::LlmError::ContextOverflow`,
//! which the llm-client codec layer emits directly on prompt-too-long responses.
//! The pure parsing helpers above (`parse_prompt_too_long_token_counts`,
//! `is_prompt_too_long_body`, `prompt_too_long_token_gap`) are all that the
//! orchestrator's recovery loop needs.

// Re-export all public symbols from the canonical home.
pub use compaction::prompt_too_long::{
    is_prompt_too_long_body, parse_prompt_too_long_token_counts, prompt_too_long_token_gap,
    PROMPT_TOO_LONG_ERROR_MESSAGE,
};
