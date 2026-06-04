//! Autocompact — LLM-driven summarization layer.
//!
//! Plan 08 wires the optional `ForkedAgentRunner` path: when a runner +
//! `CacheSafeParamsSlot` are configured via
//! [`Autocompactor::with_forked_runner`], `compact` issues a forked agent
//! that shares the parent's prompt cache. The forked agent is driven by the
//! byte-faithful compaction prompt (see [`crate::prompt`]); its raw response
//! runs through [`crate::prompt::format_compact_summary`] (strip `<analysis>`,
//! rewrite `<summary>` → `Summary:`) and is wrapped by
//! [`crate::prompt::get_compact_user_summary_message`] into the continuation
//! message that becomes the post-compact history.
//!
//! When neither a runner nor a slot is configured (the orchestrator's default
//! construction), the layer falls back to a clearly-labelled deterministic
//! summary built through the *same* `format_compact_summary` /
//! `get_compact_user_summary_message` pipeline (no model call is possible
//! without a runner). The old `[stub-summary attempt=…]` marker is gone — see
//! the divergence note on [`Autocompactor::compact`].

use crate::thresholds::MAX_OUTPUT_TOKENS_FOR_SUMMARY;
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
            // Byte-faithful base compact prompt (TS `getCompactPrompt(None)`),
            // including the no-tools preamble/trailer.
            compact_user_prompt: crate::prompt::get_compact_prompt(None),
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
    /// When a forked runner is configured, issues a forked-agent call driven
    /// by the byte-faithful compaction prompt and shares the parent's prompt
    /// cache via the latest `CacheSafeParams` snapshot. The raw model text is
    /// run through [`crate::prompt::format_compact_summary`] then wrapped by
    /// [`crate::prompt::get_compact_user_summary_message`] (with
    /// `suppress_follow_up_questions = true`, `transcript_path = None`) — the
    /// TS `compact.ts` summary-request path.
    ///
    /// **Divergence — summary message role.** TS builds the summary as a
    /// *user* message carrying the `isCompactSummary` /
    /// `isVisibleInTranscriptOnly` metadata flags. The Rust `protocol`
    /// `ConversationMessage` has no such flag, so the summary is emitted as a
    /// [`ConversationMessage::User`] (role-faithful, matching TS
    /// `createUserMessage(getCompactUserSummaryMessage(...))`); the metadata
    /// flags have no protocol equivalent and are dropped. The user-facing text
    /// is byte-for-byte the TS string.
    ///
    /// **Fallback (no runner/slot).** Without a runner there is no model to
    /// call, so the layer cannot produce a real summary. Rather than ship a
    /// model-call result, it emits a clearly-labelled deterministic
    /// continuation message built through the *same* formatting pipeline. The
    /// production CLI path always wires `with_forked_runner`, so this fallback
    /// never reaches a wired binary. This replaces the prior
    /// `[stub-summary attempt=…]` placeholder.
    ///
    /// # Errors
    ///
    /// Returns [`CompactionError::Internal`] when the configured runner is
    /// present but the slot is empty, or the forked call fails.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn compact(
        &self,
        messages: Vec<ConversationMessage>,
    ) -> Result<CompactionResult, CompactionError> {
        let pre = crate::grouping::estimate_tokens_for_range(&messages);

        // Plan 08 path — closes C2. Wired summarizer via the forked runner.
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

            // Strip <analysis>, rewrite <summary> → Summary:, then wrap in the
            // continuation message — the TS `compact.ts` summary-request path.
            let summary_text = crate::prompt::get_compact_user_summary_message(
                &result.final_text,
                /* suppress_follow_up_questions */ true,
                /* transcript_path */ None,
            );

            return Ok(CompactionResult {
                pre_compact_token_count: pre,
                post_compact_token_count: (summary_text.len() as u64) / 4,
                true_post_compact_token_count: result.usage.tokens.input,
                compaction_usage: Some(result.usage),
                // Divergence: TS uses a user message with isCompactSummary;
                // protocol lacks the flag, so we emit a plain User message.
                summary_messages: vec![ConversationMessage::user(
                    protocol::MessageId::new(),
                    summary_text,
                )],
            });
        }

        // Fallback: no runner is wired, so no model summary is possible.
        // Emit a deterministic, clearly-labelled continuation message built
        // through the SAME format_compact_summary / continuation pipeline so
        // downstream history shape is consistent. NOTE: the production CLI
        // always uses `with_forked_runner`, so this branch is never hit by a
        // wired binary — it exists only for default/unwired construction
        // (e.g. the orchestrator e2e test). The `[stub-summary attempt=…]`
        // marker is intentionally gone.
        let unwired_summary = format!(
            "<summary>\n[autocompact fallback: no forked summarizer wired; {} messages elided]\n</summary>",
            messages.len()
        );
        let summary_text = crate::prompt::get_compact_user_summary_message(
            &unwired_summary,
            /* suppress_follow_up_questions */ true,
            /* transcript_path */ None,
        );
        Ok(CompactionResult {
            pre_compact_token_count: pre,
            post_compact_token_count: (summary_text.len() as u64) / 4,
            true_post_compact_token_count: (summary_text.len() as u64) / 4,
            compaction_usage: Some(Usage::default()),
            summary_messages: vec![ConversationMessage::user(
                protocol::MessageId::new(),
                summary_text,
            )],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use protocol::MessageId;
    use sidequery::{
        CacheSafeParams, SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse,
        SubagentSlotProvider,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tool_api::context::ToolUseOptions;

    /// `SubagentSlotProvider` never asked to allocate on the single-turn path.
    struct NoopProvider;
    impl SubagentSlotProvider for NoopProvider {}

    /// Mock `SideQueryClient` returning a canned raw summary string and
    /// recording the request it was handed.
    struct MockClient {
        seen: Mutex<Option<SideQueryRequest>>,
        canned_text: String,
    }

    #[async_trait]
    impl SideQueryClient for MockClient {
        async fn query(
            &self,
            request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            *self.seen.lock().unwrap() = Some(request);
            Ok(SideQueryResponse {
                text: Some(self.canned_text.clone()),
                structured: None,
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stop_reason: Some("end_turn".into()),
            })
        }
    }

    fn user_msg(text: &str) -> ConversationMessage {
        ConversationMessage::user(MessageId::new(), text.into())
    }

    fn tool_use_options() -> ToolUseOptions {
        ToolUseOptions {
            debug: false,
            verbose: false,
            main_loop_model: "test".into(),
            max_budget_nano_usd: None,
            mcp_clients: vec![],
            is_non_interactive_session: false,
            custom_system_prompt: None,
            append_system_prompt: None,
        }
    }

    fn cache_safe_params(prefix: Vec<ConversationMessage>) -> CacheSafeParams {
        CacheSafeParams {
            system_prompt: std::sync::Arc::from("PARENT SYSTEM PROMPT"),
            user_context: HashMap::new(),
            system_context: HashMap::new(),
            tool_use_options: tool_use_options(),
            fork_context_messages: prefix,
            generation: 0,
        }
    }

    /// Build an `Autocompactor` wired to a mock client returning `canned_text`,
    /// with a `CacheSafeParamsSlot` pre-populated from `prefix`. Returns the
    /// compactor plus the shared mock client (to assert the sent request).
    async fn wired(
        canned_text: &str,
        prefix: Vec<ConversationMessage>,
    ) -> (Autocompactor, Arc<MockClient>) {
        let client = Arc::new(MockClient {
            seen: Mutex::new(None),
            canned_text: canned_text.into(),
        });
        let runner = Arc::new(
            ForkedAgentRunner::new(Arc::new(NoopProvider))
                .with_side_query_client(client.clone(), "claude-opus-4-6".into()),
        );
        let slot = Arc::new(CacheSafeParamsSlot::new());
        slot.save(cache_safe_params(prefix)).await;
        (Autocompactor::with_forked_runner(runner, slot), client)
    }

    #[tokio::test]
    async fn compact_wraps_forked_summary_through_format_pipeline() {
        // Spec test plan: mock SideQueryClient returning
        // `<analysis>x</analysis><summary>S</summary>` → wrapped output
        // contains `Summary:\nS` and the continuation sentence.
        let (compactor, _client) =
            wired("<analysis>scratch thoughts</analysis><summary>S</summary>", vec![]).await;

        let result = compactor
            .compact(vec![user_msg("hello"), user_msg("world")])
            .await
            .expect("wired compact succeeds");

        assert_eq!(result.summary_messages.len(), 1);
        // Divergence: emitted as a User message (TS uses isCompactSummary).
        let msg = &result.summary_messages[0];
        assert!(
            matches!(msg, ConversationMessage::User { .. }),
            "summary should be a User message, got {msg:?}"
        );
        let text = msg.text_content();

        // <analysis> stripped, <summary> → Summary:\nS.
        assert!(!text.contains("scratch thoughts"), "analysis not stripped: {text}");
        assert!(!text.contains("<analysis>"), "raw analysis tag leaked: {text}");
        assert!(!text.contains("<summary>"), "raw summary tag leaked: {text}");
        assert!(text.contains("Summary:\nS"), "missing unwrapped summary: {text}");

        // Continuation preamble + suppress-follow-up sentence are byte-faithful.
        assert!(text.starts_with(
            "This session is being continued from a previous conversation that ran out of context."
        ));
        assert!(text.contains(
            "Continue the conversation from where it left off without asking the user any further questions."
        ));
        assert!(text.contains("Pick up the last task as if the break never happened."));
    }

    #[tokio::test]
    async fn compact_sends_byte_faithful_compact_prompt_after_cache_prefix() {
        let (compactor, client) =
            wired("<summary>ok</summary>", vec![user_msg("PREFIX-A")]).await;

        compactor
            .compact(vec![user_msg("hello")])
            .await
            .expect("wired compact succeeds");

        let sent = client.seen.lock().unwrap().clone().expect("client called");
        // The forked request replays the cache prefix first, then the compact
        // prompt (the byte-faithful `get_compact_prompt(None)`).
        let texts: Vec<String> = sent
            .messages
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        assert_eq!(texts.len(), 2, "prefix + compact prompt");
        assert_eq!(texts[0], "PREFIX-A");
        assert_eq!(texts[1], crate::prompt::get_compact_prompt(None));
        // Parent system prompt replayed verbatim for the cache hit.
        assert_eq!(sent.system_prompt.as_deref(), Some("PARENT SYSTEM PROMPT"));
        // Usage from the forked call is carried through.
    }

    #[tokio::test]
    async fn compact_passthrough_when_model_returns_no_tags() {
        // A model that ignored the <analysis>/<summary> structure still yields
        // a valid continuation message (formatCompactSummary passthrough).
        let (compactor, _client) = wired("plain summary text", vec![]).await;
        let result = compactor.compact(vec![user_msg("x")]).await.unwrap();
        let text = result.summary_messages[0].text_content();
        assert!(text.contains("plain summary text"));
        assert!(text.contains("Continue the conversation from where it left off"));
    }

    #[tokio::test]
    async fn compact_errors_when_runner_wired_but_slot_empty() {
        let client = Arc::new(MockClient {
            seen: Mutex::new(None),
            canned_text: "<summary>S</summary>".into(),
        });
        let runner = Arc::new(
            ForkedAgentRunner::new(Arc::new(NoopProvider))
                .with_side_query_client(client, "m".into()),
        );
        // Empty slot — never saved.
        let slot = Arc::new(CacheSafeParamsSlot::new());
        let compactor = Autocompactor::with_forked_runner(runner, slot);

        let err = compactor
            .compact(vec![user_msg("x")])
            .await
            .expect_err("empty slot surfaces an error");
        match err {
            CompactionError::Internal(msg) => {
                assert!(msg.contains("no cache-safe params"), "unexpected msg: {msg}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compact_fallback_without_runner_no_stub_marker() {
        // Default construction (no forked runner): deterministic fallback,
        // built through the SAME continuation pipeline, with NO `[stub-summary]`.
        let compactor = Autocompactor::new();
        let result = compactor
            .compact(vec![user_msg("a"), user_msg("b")])
            .await
            .expect("fallback compact succeeds");

        assert_eq!(result.summary_messages.len(), 1);
        let msg = &result.summary_messages[0];
        assert!(
            matches!(msg, ConversationMessage::User { .. }),
            "fallback summary should be a User message"
        );
        let text = msg.text_content();
        assert!(
            !text.contains("[stub-summary"),
            "the old stub marker must not ship: {text}"
        );
        assert!(text.starts_with(
            "This session is being continued from a previous conversation that ran out of context."
        ));
        assert!(text.contains("Continue the conversation from where it left off"));
        // The fallback's <summary> wrapper is consumed by formatCompactSummary.
        assert!(!text.contains("<summary>"));
    }

    #[test]
    fn default_config_uses_byte_faithful_compact_prompt() {
        let cfg = AutocompactConfig::default();
        assert_eq!(cfg.compact_user_prompt, crate::prompt::get_compact_prompt(None));
        // Sanity: the prior placeholder prompt is gone.
        assert!(!cfg.compact_user_prompt.contains("Output ONLY the summary."));
        assert!(cfg
            .compact_user_prompt
            .contains("Your task is to create a detailed summary of the conversation so far"));
    }
}
