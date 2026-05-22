//! Autocompact — LLM-driven summarization layer. M1.7 ships a stub that
//! preserves the PTL retry plumbing; real summarization wires
//! `ForkedAgentRunner` in Plan 08.

#![allow(
    unused_imports,
    unreachable_code,
    clippy::needless_range_loop,
    clippy::unused_async
)]

use crate::grouping::group_messages_by_api_round;
use crate::ptl_retry::truncate_head_for_ptl_retry;
use crate::thresholds::{MAX_OUTPUT_TOKENS_FOR_SUMMARY, MAX_PTL_RETRIES};
use lingxi_api_client::ApiError;
use lingxi_cost::Usage;
use lingxi_protocol::ConversationMessage;
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

/// Stateful autocompactor. M1.7 has no injected dependencies; Plan 08 will add
/// a `ForkedAgentRunner` field.
#[derive(Default)]
pub struct Autocompactor {
    /// Tunables; defaults to [`AutocompactConfig::default`].
    pub config: AutocompactConfig,
}

impl Autocompactor {
    /// Construct an autocompactor with default config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: AutocompactConfig::default(),
        }
    }

    /// M1.7 sketch: PTL retry shape + summary plumbing. Real summarization
    /// happens once Plan 08 wires `ForkedAgentRunner`.
    #[allow(clippy::never_loop)]
    pub async fn compact(
        &self,
        messages: Vec<ConversationMessage>,
    ) -> Result<CompactionResult, CompactionError> {
        let pre = crate::grouping::estimate_tokens_for_range(&messages);
        let groups = group_messages_by_api_round(&messages);

        for attempt in 0..MAX_PTL_RETRIES {
            // In Plan 08 this is replaced with ForkedAgentRunner::run.
            let summary_text = format!(
                "[stub-summary attempt={attempt}; messages={}]",
                messages.len()
            );

            let summary_msg = ConversationMessage::System {
                id: lingxi_protocol::MessageId::new(),
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
        // PTL handling sketch (not reached in stub but compiles):
        let _ = truncate_head_for_ptl_retry(messages, 0, &groups);
        Err(CompactionError::MaxRetriesExceeded)
    }
}
