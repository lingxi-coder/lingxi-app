//! Autocompact — LLM-driven summarization layer.
//!
//! Plan 08 wires the optional `ForkedAgentRunner` path: when a runner +
//! `CacheSafeParamsSlot` are configured via
//! [`Autocompactor::with_forked_runner`], `compact` issues a forked agent
//! that shares the parent's prompt cache. When neither is configured (as
//! in the orchestrator's default construction today) the layer falls back
//! to the M1.7 stub summary so the orchestrator e2e test still runs.

#![allow(unused_imports, unreachable_code, clippy::needless_range_loop)]

use crate::grouping::group_messages_by_api_round;
use crate::ptl_retry::truncate_head_for_ptl_retry;
use crate::thresholds::{MAX_OUTPUT_TOKENS_FOR_SUMMARY, MAX_PTL_RETRIES};
use api_client::ApiError;
use cost::Usage;
use protocol::ConversationMessage;
use sidequery::{CacheSafeParamsSlot, ForkedAgentRequest, ForkedAgentRunner, QuerySource};
use std::sync::Arc;
use thiserror::Error;

/// Result of one autocompact pass.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Estimated tokens before compaction.
    pub pre_compact_token_count: u64,
    /// Estimated tokens after compaction (advertised).
    pub post_compact_token_count: u64,
    /// Actual measured tokens after compaction (post-call).
    pub true_post_compact_token_count: u64,
    /// Token/cost usage incurred by the summarization call, if any.
    pub compaction_usage: Option<Usage>,
    /// Resulting messages (typically a single summary system message).
    pub summary_messages: Vec<ConversationMessage>,
}

/// Errors surfaced by the autocompact layer.
#[derive(Debug, Clone, Error)]
pub enum CompactionError {
    /// Underlying API call failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Exhausted PTL retries without success.
    #[error("max retries exceeded")]
    MaxRetriesExceeded,
    /// Autocompact does not apply to this state.
    #[error("not applicable")]
    NotApplicable,
    /// Internal logic error.
    #[error("internal: {0}")]
    Internal(String),
}

/// Tunables for the autocompact layer.
pub struct AutocompactConfig {
    /// Model used for summarization.
    pub summary_model: String,
    /// Maximum output tokens for the summary.
    pub max_output_tokens: u64,
    /// User prompt instructing the summarizer.
    pub compact_user_prompt: String,
}

impl Default for AutocompactConfig {
    fn default() -> Self {
        Self {
            summary_model: "claude-opus-4-6".into(),
            max_output_tokens: MAX_OUTPUT_TOKENS_FOR_SUMMARY,
            compact_user_prompt: "Summarize the conversation so far in a concise paragraph that retains key decisions, file paths read, and pending tasks. Output ONLY the summary.".into(),
        }
    }
}

/// Stateful autocompactor.
///
/// Two construction paths:
/// - [`Autocompactor::new`] / [`Autocompactor::default`] — no forked
///   runner, returns the M1.7 stub summary. Used by the orchestrator's
///   default wiring and the e2e test that does not need a real
///   summarization call.
/// - [`Autocompactor::with_forked_runner`] — Plan 08 path, routes through
///   the shared `ForkedAgentRunner` using the latest `CacheSafeParams`
///   from the supplied slot. Closes spec gap **C2**.
#[derive(Default)]
pub struct Autocompactor {
    /// Tunables; defaults to [`AutocompactConfig::default`].
    pub config: AutocompactConfig,
    forked_runner: Option<Arc<ForkedAgentRunner>>,
    cache_slot: Option<Arc<CacheSafeParamsSlot>>,
}

impl Autocompactor {
    /// Construct an autocompactor with default config and no forked runner
    /// (falls back to the M1.7 stub summary).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct an autocompactor that routes through `forked_runner`,
    /// reading the latest cache-safe prompt prefix from `cache_slot`. This
    /// is the Plan 08 production path.
    #[must_use]
    pub fn with_forked_runner(
        forked_runner: Arc<ForkedAgentRunner>,
        cache_slot: Arc<CacheSafeParamsSlot>,
    ) -> Self {
        Self {
            config: AutocompactConfig::default(),
            forked_runner: Some(forked_runner),
            cache_slot: Some(cache_slot),
        }
    }

    /// Compact `messages` into a summary.
    ///
    /// When a forked runner is configured, issues a forked-agent call that
    /// shares the parent's prompt cache via the latest `CacheSafeParams`
    /// snapshot. Otherwise falls back to the M1.7 stub.
    ///
    /// # Errors
    ///
    /// Returns [`CompactionError::Internal`] when the configured runner is
    /// present but the slot is empty, or the forked call fails.
    #[allow(clippy::never_loop, clippy::cast_possible_truncation)]
    pub async fn compact(
        &self,
        messages: Vec<ConversationMessage>,
    ) -> Result<CompactionResult, CompactionError> {
        let pre = crate::grouping::estimate_tokens_for_range(&messages);

        // Plan 08 path — closes C2.
        if let (Some(runner), Some(slot)) = (&self.forked_runner, &self.cache_slot) {
            let cache_params = slot
                .get_last()
                .await
                .ok_or_else(|| CompactionError::Internal("no cache-safe params".into()))?;

            let req = ForkedAgentRequest {
                prompt_messages: vec![ConversationMessage::user(
                    protocol::MessageId::new(),
                    self.config.compact_user_prompt.clone(),
                )],
                cache_safe_params: cache_params,
                fork_label: "compaction".into(),
                query_source: QuerySource::Compaction,
                max_output_tokens: Some(
                    u32::try_from(self.config.max_output_tokens).unwrap_or(u32::MAX),
                ),
            };
            let result = runner
                .run(req)
                .await
                .map_err(|e| CompactionError::Internal(e.to_string()))?;

            return Ok(CompactionResult {
                pre_compact_token_count: pre,
                post_compact_token_count: (result.final_text.len() as u64) / 4,
                true_post_compact_token_count: result.usage.tokens.input,
                compaction_usage: Some(result.usage),
                summary_messages: vec![ConversationMessage::System {
                    id: protocol::MessageId::new(),
                    content: result.final_text,
                }],
            });
        }

        // Fallback: M1.7 stub summary (used by the orchestrator's default
        // construction). Real summarization only happens when the engine
        // wires a ForkedAgentRunner via `with_forked_runner`.
        let groups = group_messages_by_api_round(&messages);
        for attempt in 0..MAX_PTL_RETRIES {
            let summary_text = format!(
                "[stub-summary attempt={attempt}; messages={}]",
                messages.len()
            );

            let summary_msg = ConversationMessage::System {
                id: protocol::MessageId::new(),
                content: summary_text,
            };
            return Ok(CompactionResult {
                pre_compact_token_count: pre,
                post_compact_token_count: 200,
                true_post_compact_token_count: 200,
                compaction_usage: Some(Usage::default()),
                summary_messages: vec![summary_msg],
            });
        }
        // PTL handling sketch (not reached in stub but compiles).
        let _ = truncate_head_for_ptl_retry(messages, 0, &groups);
        Err(CompactionError::MaxRetriesExceeded)
    }
}
