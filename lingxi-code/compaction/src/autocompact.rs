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
    /// Actual model inherited by the summary side-query.
    pub summary_model: String,
    /// Resulting messages (typically a single summary system message).
    pub summary_messages: Vec<ConversationMessage>,
    /// #58: the verbatim tail of recent messages preserved across the
    /// compaction boundary (`messagesToPreserve` from `DRn`, usage-zeroed via
    /// [`crate::partial::zero_preserved_tail_usage`] = TS `k4e`). Empty on the
    /// full-replacement path (short conversation / no preservable tail), in
    /// which case post-compact history is byte-identical to before this finding.
    /// Otherwise this rides AFTER the summary in the `Iqn`
    /// `[boundaryMarker, ...summaryMessages, ...messagesToKeep, ...]` order.
    pub messages_to_preserve: Vec<ConversationMessage>,
}

/// Errors surfaced by the autocompact layer.
#[derive(Debug, Clone, Error)]
pub enum CompactionError {
    /// Underlying API call failed.
    #[error(transparent)]
    Api(#[from] llm_client::LlmError),
    /// Exhausted PTL retries without success.
    #[error("max retries exceeded")]
    MaxRetriesExceeded,
    /// Autocompact does not apply to this state.
    #[error("not applicable")]
    NotApplicable,
    /// A manual `/compact` needs at least one completed exchange to summarize
    /// while preserving a valid recent tail.
    #[error("Not enough messages to compact.")]
    NotEnoughMessages,
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
        self.compact_with_instructions(messages, None).await
    }

    /// Compact `messages`, appending optional caller/hook instructions to the
    /// byte-faithful compaction prompt.
    ///
    /// The ordinary automatic path calls [`Self::compact`] and therefore keeps
    /// the configured base prompt. Successful `PreCompact` hook stdout uses
    /// this seam so its instructions reach the summarizer exactly once
    /// (manual `/compact <focus>` routes through
    /// [`Self::compact_manual_with_instructions`] instead).
    pub async fn compact_with_instructions(
        &self,
        messages: Vec<ConversationMessage>,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult, CompactionError> {
        self.compact_impl(messages, custom_instructions, false)
            .await
    }

    /// Run the explicit `/compact` manual path.
    ///
    /// VERIFIED against the 2.1.211 binary (`edy → Juy → mXi → Nto`): the
    /// manual compactor is the SAME group compactor as the reactive path —
    /// it starts with `s = 1` (the LAST API-round group preserved verbatim,
    /// `messagesToPreserve: m.flat()`), and errors `too_few_groups` when the
    /// summarize prefix contains no assistant message. (The full-history
    /// `messagesToKeep: []` path — `Pto` — is only ever called with
    /// `isAutoCompact: !0`.) Where the manual path DOES differ from auto:
    /// - no wired summarizer is a hard error (an explicit command must never
    ///   fake success), and
    /// - no valid preserved-tail split is `Not enough messages to compact.`
    ///   (auto falls back to full replacement instead).
    pub async fn compact_manual_with_instructions(
        &self,
        messages: Vec<ConversationMessage>,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult, CompactionError> {
        self.compact_impl(messages, custom_instructions, true).await
    }

    async fn compact_impl(
        &self,
        messages: Vec<ConversationMessage>,
        custom_instructions: Option<&str>,
        manual: bool,
    ) -> Result<CompactionResult, CompactionError> {
        let pre = crate::grouping::estimate_tokens_for_range(&messages);

        if manual
            && !self
                .forked_runner
                .as_ref()
                .is_some_and(|runner| runner.has_side_query_client())
        {
            return Err(CompactionError::Internal(
                "no forked summarizer wired".into(),
            ));
        }

        // #58 suffix-preserving split (`DRn`/`Nto` with `s = 1`): choose the
        // smallest preserved tail (largest summarize set) whose summarize
        // prefix still contains an assistant message. Applies to BOTH the auto
        // and manual paths (binary-verified: manual `/compact` routes through
        // the same group compactor). `None` ⇒ no valid prefix: the manual path
        // errors with the byte-exact `dQt` string (`Nto`'s `too_few_groups`
        // bail), while auto falls back to full-replacement (pre-#58
        // behaviour, empty preserved tail). The tail is usage-zeroed (`k4e`)
        // and carried out so `apply_post_compact` can splice it after the
        // summary.
        let split = crate::partial::select_preserved_tail(&messages);
        if manual && split.is_none() {
            return Err(CompactionError::NotEnoughMessages);
        }
        let preserved_tail: Vec<ConversationMessage> = split
            .as_ref()
            .map(|s| crate::partial::zero_preserved_tail_usage(s.to_preserve.clone()))
            .unwrap_or_default();
        let recent_messages_preserved = !preserved_tail.is_empty();

        // Plan 08 path — closes C2. Wired summarizer via the forked runner.
        if let (Some(runner), Some(slot)) = (&self.forked_runner, &self.cache_slot) {
            let mut cache_params = slot
                .get_last()
                .await
                .ok_or_else(|| CompactionError::Internal("no cache-safe params".into()))?;
            let summary_model = if cache_params
                .tool_use_options
                .main_loop_model
                .trim()
                .is_empty()
            {
                self.config.summary_model.clone()
            } else {
                cache_params.tool_use_options.main_loop_model.clone()
            };

            // #58: summarize the ACTUAL current prefix, not the last cache-slot
            // history clone. The slot is captured immediately after an API call
            // and therefore predates the assistant reply appended afterwards;
            // truncating that stale clone by the preserved-tail length silently
            // dropped the newest assistant turn. We still reuse the slot's
            // system prompt / model metadata, but replay exactly the messages
            // selected from this invocation's live history.
            cache_params.fork_context_messages = split
                .as_ref()
                .map_or_else(|| messages.clone(), |split| split.to_summarize.clone());

            // Strip image blocks from the replayed context before the summary
            // request — the text summarizer must not receive raw image data
            // (TS `stripImagesFromMessages`, `compact.ts:145-200`, applied at the
            // summary call site). `get_last` returned an owned clone, so this
            // only affects this summary request, not the stored slot.
            cache_params.fork_context_messages =
                crate::strip_media::strip_images_from_messages(cache_params.fork_context_messages);

            // COMPACT.2: prompt-too-long (PTL) retry loop — TS `compact.ts:445-491`.
            // CC-1180: when the compact request ITSELF hits prompt-too-long, drop
            // the oldest API-round groups from the replayed fork context and retry,
            // up to `MAX_PTL_RETRIES` times, rather than leaving the user stuck. TS
            // detects PTL by the summary text starting with
            // `PROMPT_TOO_LONG_ERROR_MESSAGE`; we mirror that check on
            // `final_text`, and thread the truncated set through
            // `cache_safe_params.fork_context_messages` each attempt (TS's
            // `retryCacheSafeParams.forkContextMessages = truncated`).
            let mut ptl_attempts: u32 = 0;
            let result = loop {
                let req = ForkedAgentRequest {
                    prompt_messages: vec![ConversationMessage::user(
                        protocol::MessageId::new(),
                        if custom_instructions.is_some_and(|s| !s.trim().is_empty()) {
                            crate::prompt::get_compact_prompt(custom_instructions)
                        } else {
                            self.config.compact_user_prompt.clone()
                        },
                    )],
                    cache_safe_params: cache_params.clone(),
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

                // Not a prompt-too-long summary → accept it (TS `break`).
                if !result
                    .final_text
                    .starts_with(crate::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE)
                {
                    break result;
                }

                // The compact request itself hit prompt-too-long: truncate the
                // oldest API-round groups and retry. `None` (nothing safe left to
                // drop) or exhausting `MAX_PTL_RETRIES` surfaces the failure — TS
                // throws `ERROR_MESSAGE_PROMPT_TOO_LONG`; the Rust port models
                // "exhausted PTL retries" as `MaxRetriesExceeded`.
                ptl_attempts += 1;
                let truncated = if ptl_attempts <= crate::thresholds::MAX_PTL_RETRIES {
                    crate::ptl_retry::truncate_head_for_ptl_retry(
                        cache_params.fork_context_messages.clone(),
                        crate::prompt_too_long::prompt_too_long_token_gap(&result.final_text),
                    )
                } else {
                    None
                };
                let Some(truncated) = truncated else {
                    return Err(CompactionError::MaxRetriesExceeded);
                };
                cache_params.fork_context_messages = truncated;
            };

            // Claude rejects a summary response with no text instead of
            // wrapping an empty string in the continuation template. Treat it
            // as a hard failure so `/compact` cannot report a second form of
            // fake success when a provider returns an empty assistant message.
            // The string differs per path (binary-verified): manual `/compact`
            // surfaces `Error during compaction: <detail>` (Juy's catch over
            // pIg's `summarization produced empty response` detail), while the
            // auto path (`Pto`) throws the unprefixed `Failed to generate…`.
            if result.final_text.is_empty() {
                return Err(CompactionError::Internal(if manual {
                    "Error during compaction: summarization produced empty response".into()
                } else {
                    "Failed to generate conversation summary - response did not contain valid text content"
                        .into()
                }));
            }

            // Strip <analysis>, rewrite <summary> → Summary:, then wrap in the
            // continuation message — the TS `compact.ts` summary-request path.
            // #58: `recent_messages_preserved` adds the "Recent messages are
            // preserved verbatim." sentence when a tail rides after the summary.
            let summary_text = crate::prompt::get_compact_user_summary_message(
                &result.final_text,
                /* suppress_follow_up_questions */ true,
                cache_params
                    .transcript_path
                    .as_deref()
                    .and_then(std::path::Path::to_str),
                recent_messages_preserved,
            );

            return Ok(CompactionResult {
                pre_compact_token_count: pre,
                post_compact_token_count: (summary_text.len() as u64) / 4,
                true_post_compact_token_count: result.usage.tokens.input,
                compaction_usage: Some(result.usage),
                summary_model,
                // Divergence: TS uses a user message with isCompactSummary;
                // protocol lacks the flag, so we emit a plain User message.
                summary_messages: vec![ConversationMessage::user(
                    protocol::MessageId::new(),
                    summary_text,
                )],
                // #58: the usage-zeroed verbatim tail, spliced after the summary
                // by `apply_post_compact`. Empty ⇒ full-replacement path.
                messages_to_preserve: preserved_tail,
            });
        }

        // An explicit user command must never claim success without a model
        // summary. Keep the deterministic fallback only for the legacy
        // automatic-test path; every full/manual compact requires real wiring.
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
            recent_messages_preserved,
        );
        Ok(CompactionResult {
            pre_compact_token_count: pre,
            post_compact_token_count: (summary_text.len() as u64) / 4,
            true_post_compact_token_count: (summary_text.len() as u64) / 4,
            compaction_usage: Some(Usage::default()),
            summary_model: self.config.summary_model.clone(),
            summary_messages: vec![ConversationMessage::user(
                protocol::MessageId::new(),
                summary_text,
            )],
            // #58: carry the preserved tail through the fallback too, so the
            // history shape is consistent across both construction paths.
            messages_to_preserve: preserved_tail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use protocol::{ContentBlock, MessageId, ToolUseId};
    use serde_json::json;
    use sidequery::{
        CacheSafeParams, SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse,
    };
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use tool_api::context::ToolUseOptions;

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
            model_profile: None,
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
            transcript_path: None,
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
            ForkedAgentRunner::new()
                .with_side_query_client(client.clone(), "claude-opus-4-6".into()),
        );
        let slot = Arc::new(CacheSafeParamsSlot::new());
        slot.save(cache_safe_params(prefix)).await;
        (Autocompactor::with_forked_runner(runner, slot), client)
    }

    /// Mock returning a queue of canned texts (one per call) and recording the
    /// message count of every request, so the PTL retry tests can assert both the
    /// retry count and that each retry's prompt shrank.
    struct SeqMockClient {
        texts: Mutex<VecDeque<String>>,
        seen_lens: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl SideQueryClient for SeqMockClient {
        async fn query(
            &self,
            request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            self.seen_lens.lock().unwrap().push(request.messages.len());
            let text = self.texts.lock().unwrap().pop_front().unwrap_or_default();
            Ok(SideQueryResponse {
                text: Some(text),
                structured: None,
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stop_reason: Some("end_turn".into()),
            })
        }
    }

    /// Wire an autocompactor to a `SeqMockClient` that yields `texts` in order,
    /// with the fork-context prefix pre-populated from `prefix`.
    async fn wired_seq(
        texts: Vec<String>,
        prefix: Vec<ConversationMessage>,
    ) -> (Autocompactor, Arc<SeqMockClient>) {
        let client = Arc::new(SeqMockClient {
            texts: Mutex::new(texts.into()),
            seen_lens: Mutex::new(Vec::new()),
        });
        let runner = Arc::new(
            ForkedAgentRunner::new()
                .with_side_query_client(client.clone(), "claude-opus-4-6".into()),
        );
        let slot = Arc::new(CacheSafeParamsSlot::new());
        slot.save(cache_safe_params(prefix)).await;
        (Autocompactor::with_forked_runner(runner, slot), client)
    }

    /// Assistant message carrying a tool-use block under `id` (opens a group).
    fn assistant_tool(id: MessageId, tool: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id,
            content: vec![ContentBlock::ToolUse {
                id: ToolUseId::new(),
                name: tool.into(),
                input: json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        }
    }

    fn tool_result_msg() -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: "ok".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
        }
    }

    #[tokio::test]
    async fn compact_wraps_forked_summary_through_format_pipeline() {
        // Spec test plan: mock SideQueryClient returning
        // `<analysis>x</analysis><summary>S</summary>` → wrapped output
        // contains `Summary:\nS` and the continuation sentence.
        let (compactor, _client) = wired(
            "<analysis>scratch thoughts</analysis><summary>S</summary>",
            vec![],
        )
        .await;

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
        assert!(
            !text.contains("scratch thoughts"),
            "analysis not stripped: {text}"
        );
        assert!(
            !text.contains("<analysis>"),
            "raw analysis tag leaked: {text}"
        );
        assert!(
            !text.contains("<summary>"),
            "raw summary tag leaked: {text}"
        );
        assert!(
            text.contains("Summary:\nS"),
            "missing unwrapped summary: {text}"
        );

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
        let (compactor, client) = wired("<summary>ok</summary>", vec![user_msg("PREFIX-A")]).await;

        compactor
            .compact(vec![user_msg("CURRENT-HISTORY")])
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
        assert_eq!(
            texts[0], "CURRENT-HISTORY",
            "the summary must replay the live history, not the stale slot clone"
        );
        assert_eq!(texts[1], crate::prompt::get_compact_prompt(None));
        // Parent system prompt replayed verbatim for the cache hit.
        assert_eq!(sent.system_prompt.as_deref(), Some("PARENT SYSTEM PROMPT"));
        // Usage from the forked call is carried through.
    }

    #[tokio::test]
    async fn compact_appends_manual_focus_to_prompt() {
        let (compactor, client) = wired("<summary>ok</summary>", vec![]).await;

        compactor
            .compact_with_instructions(vec![user_msg("current")], Some("Focus on the Rust changes"))
            .await
            .expect("wired compact succeeds");

        let sent = client.seen.lock().unwrap().clone().expect("client called");
        let prompt = sent.messages.last().expect("prompt message").text_content();
        assert_eq!(
            prompt,
            crate::prompt::get_compact_prompt(Some("Focus on the Rust changes"))
        );
    }

    /// Binary-verified (`Juy → mXi → Nto`, `s = 1`): the MANUAL path summarizes
    /// the group prefix and preserves the last API-round group verbatim —
    /// `[q1],[a1,q2],[a2]` → summarize `[q1,a1,q2]`, preserve `[a2]` — with the
    /// "Recent messages are preserved verbatim." sentence in the continuation.
    #[tokio::test]
    async fn manual_compact_summarizes_prefix_and_preserves_last_group() {
        let history = vec![
            user_msg("q1"),
            assistant_text("first reply"),
            user_msg("q2"),
            assistant_text("second reply"),
        ];
        let (compactor, client) = wired("<summary>ok</summary>", history.clone()).await;

        let result = compactor
            .compact_manual_with_instructions(history.clone(), None)
            .await
            .expect("manual compact succeeds");

        assert_eq!(
            result.messages_to_preserve.len(),
            1,
            "the last assistant-led group is preserved verbatim"
        );
        assert_eq!(
            result.messages_to_preserve[0].text_content(),
            "second reply"
        );
        let sent = client.seen.lock().unwrap().clone().expect("client called");
        let replayed: Vec<String> = sent.messages[..sent.messages.len() - 1]
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        let expected: Vec<String> = history[..3]
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        assert_eq!(
            replayed, expected,
            "the summarizer sees only the summarize prefix, never the preserved tail"
        );
        assert!(result.summary_messages[0]
            .text_content()
            .contains("Recent messages are preserved verbatim."));
    }

    #[tokio::test]
    async fn manual_compact_without_a_real_client_never_returns_fake_success() {
        let err = Autocompactor::new()
            .compact_manual_with_instructions(vec![user_msg("q"), assistant_text("answer")], None)
            .await
            .expect_err("manual compact requires a real summarizer");
        assert!(err.to_string().contains("no forked summarizer wired"));
    }

    /// Manual empty-response failure surfaces `Error during compaction:
    /// summarization produced empty response` (CC `Juy` catch over `pIg`'s
    /// detail) — NOT the auto path's `Failed to generate…` string.
    #[tokio::test]
    async fn manual_compact_rejects_an_empty_model_response() {
        let history = vec![
            user_msg("q1"),
            assistant_text("first reply"),
            user_msg("q2"),
            assistant_text("second reply"),
        ];
        let (compactor, _client) = wired("", history.clone()).await;
        let err = compactor
            .compact_manual_with_instructions(history, None)
            .await
            .expect_err("an empty model response is not a compact summary");
        assert!(err
            .to_string()
            .contains("Error during compaction: summarization produced empty response"));
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
        let runner = Arc::new(ForkedAgentRunner::new().with_side_query_client(client, "m".into()));
        // Empty slot — never saved.
        let slot = Arc::new(CacheSafeParamsSlot::new());
        let compactor = Autocompactor::with_forked_runner(runner, slot);

        let err = compactor
            .compact(vec![user_msg("x")])
            .await
            .expect_err("empty slot surfaces an error");
        match err {
            CompactionError::Internal(msg) => {
                assert!(
                    msg.contains("no cache-safe params"),
                    "unexpected msg: {msg}"
                );
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
        assert_eq!(
            cfg.compact_user_prompt,
            crate::prompt::get_compact_prompt(None)
        );
        // Sanity: the prior placeholder prompt is gone.
        assert!(!cfg.compact_user_prompt.contains("Output ONLY the summary."));
        assert!(cfg
            .compact_user_prompt
            .contains("Your task is to create a detailed summary of the conversation so far"));
    }

    // --- COMPACT.2: prompt-too-long retry loop --------------------------- //

    #[tokio::test]
    async fn compact_ptl_retry_truncates_and_succeeds() {
        // First summary call returns prompt-too-long; the loop drops the oldest
        // API-round group from the fork context and retries, and the second call
        // returns a real summary (TS `compact.ts:445-491`). Prefix groups:
        // [u,u,u] [aA,result] [aB,result] → 3 groups; the unknown gap drops the
        // 3-message preamble (group 0), so the retried prompt is strictly shorter.
        let id_a = MessageId::new();
        let id_b = MessageId::new();
        let prefix = vec![
            user_msg("preamble line one"),
            user_msg("preamble line two"),
            user_msg("preamble line three"),
            assistant_tool(id_a, "Read"),
            tool_result_msg(),
            assistant_tool(id_b, "Bash"),
            tool_result_msg(),
        ];
        let (compactor, client) = wired_seq(
            vec![
                "Prompt is too long".into(),
                "<summary>RETRIED-OK</summary>".into(),
            ],
            prefix.clone(),
        )
        .await;

        let result = compactor
            .compact(prefix)
            .await
            .expect("PTL retry then success");

        let text = result.summary_messages[0].text_content();
        assert!(
            text.contains("Summary:\nRETRIED-OK"),
            "retried summary not surfaced: {text}"
        );

        let lens = client.seen_lens.lock().unwrap().clone();
        assert_eq!(lens.len(), 2, "exactly one PTL retry (two summary calls)");
        assert!(
            lens[1] < lens[0],
            "the retry must send a SHORTER prompt after head-truncation: {lens:?}"
        );
    }

    #[tokio::test]
    async fn compact_ptl_retry_exhausts_to_max_retries_error() {
        // Every summary call returns prompt-too-long: the loop truncates until
        // there is nothing safe left to drop, then surfaces MaxRetriesExceeded
        // (TS throws ERROR_MESSAGE_PROMPT_TOO_LONG).
        let id_a = MessageId::new();
        let id_b = MessageId::new();
        let prefix = vec![
            user_msg("preamble"),
            assistant_tool(id_a, "Read"),
            tool_result_msg(),
            assistant_tool(id_b, "Bash"),
            tool_result_msg(),
        ];
        let (compactor, client) =
            wired_seq(vec!["Prompt is too long".into(); 6], prefix.clone()).await;

        let err = compactor
            .compact(prefix)
            .await
            .expect_err("PTL exhaustion must error");
        assert!(
            matches!(err, CompactionError::MaxRetriesExceeded),
            "expected MaxRetriesExceeded, got {err:?}"
        );
        // It actually retried (more than the single initial attempt) before
        // giving up — bounded by what truncation can shed.
        assert!(
            client.seen_lens.lock().unwrap().len() >= 2,
            "should attempt at least one truncated retry before exhausting"
        );
    }

    // --- #58: preserved recent-message tail ------------------------------ //

    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            stop_reason: Some("end_turn".into()),
        }
    }

    #[tokio::test]
    async fn compact_preserves_recent_tail_for_long_conversation() {
        // A multi-group conversation: [u(q1), aA, u(q2), aB] → three API-round
        // groups (boundary before each new assistant id). `DRn`/select_preserved_tail
        // preserves the last group (the final assistant reply) and summarizes the
        // prefix. The forked summarizer only sees the prefix; the tail is carried
        // out verbatim in `messages_to_preserve`.
        let prefix = vec![
            user_msg("q1"),
            assistant_text("first reply"),
            user_msg("q2"),
            assistant_text("second reply"),
        ];
        let (compactor, _client) = wired("<summary>S</summary>", prefix.clone()).await;

        let result = compactor.compact(prefix).await.expect("wired compact");

        // The preserved tail is non-empty (the last API round).
        assert!(
            !result.messages_to_preserve.is_empty(),
            "a long conversation must preserve a recent tail"
        );
        // The summary (leading) is a single User message; the tail follows.
        assert_eq!(result.summary_messages.len(), 1);
        // The continuation message carries the preserved-tail sentence.
        let summary_text = result.summary_messages[0].text_content();
        assert!(
            summary_text.contains("Recent messages are preserved verbatim."),
            "preserved-tail summary must announce the verbatim tail: {summary_text}"
        );
        // The preserved tail is the final assistant reply, carried verbatim.
        let tail_texts: Vec<String> = result
            .messages_to_preserve
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        assert!(
            tail_texts.iter().any(|t| t == "second reply"),
            "the most recent assistant reply must be preserved verbatim: {tail_texts:?}"
        );
    }

    #[tokio::test]
    async fn compact_summarizer_sees_only_the_prefix_when_tail_preserved() {
        // The forked summarizer must replay only the summarize PREFIX (TS
        // `_kd(A,...)`), with the preserved tail dropped from `fork_context_messages`.
        let prefix = vec![
            user_msg("q1"),
            assistant_text("first reply"),
            user_msg("q2"),
            assistant_text("second reply"),
        ];
        let (compactor, client) = wired("<summary>S</summary>", prefix.clone()).await;

        let result = compactor.compact(prefix.clone()).await.expect("compact");
        let keep = result.messages_to_preserve.len();
        assert!(keep > 0, "precondition: a tail was preserved");

        let sent = client.seen.lock().unwrap().clone().expect("client called");
        // The forked request replays (prefix − preserved_tail) then the compact
        // prompt. So the replayed-context count is `4 − keep`, plus the prompt.
        let replayed = sent.messages.len() - 1; // last is the compact prompt
        assert_eq!(
            replayed,
            prefix.len() - keep,
            "summarizer must see only the prefix (tail dropped): replayed={replayed}, keep={keep}"
        );
        // The compact prompt is still last.
        assert_eq!(
            sent.messages.last().unwrap().text_content(),
            crate::prompt::get_compact_prompt(None)
        );
    }

    #[tokio::test]
    async fn compact_short_conversation_has_no_preserved_tail() {
        // A short conversation (fewer than two API-round groups) cannot be
        // split: `messages_to_preserve` is empty and the summary carries NO
        // preserved-tail sentence — byte-identical to before #58.
        let prefix = vec![user_msg("only one user message")];
        let (compactor, _client) = wired("<summary>S</summary>", prefix.clone()).await;

        let result = compactor.compact(prefix).await.expect("compact");
        assert!(
            result.messages_to_preserve.is_empty(),
            "short conversation must NOT preserve a tail"
        );
        let summary_text = result.summary_messages[0].text_content();
        assert!(
            !summary_text.contains("Recent messages are preserved verbatim."),
            "no preserved-tail sentence on the full-replacement path: {summary_text}"
        );
    }

    #[tokio::test]
    async fn compact_no_assistant_prefix_has_no_preserved_tail() {
        // [u, u, aA]: only ONE assistant. select_preserved_tail's i=1 split
        // preserves the last group [aA] but then the summarize prefix [u,u] has
        // NO assistant → no valid split → full replacement, empty tail.
        let prefix = vec![user_msg("a"), user_msg("b"), assistant_text("only reply")];
        let (compactor, _client) = wired("<summary>S</summary>", prefix.clone()).await;

        let result = compactor.compact(prefix).await.expect("compact");
        assert!(
            result.messages_to_preserve.is_empty(),
            "a prefix with no assistant message cannot partial-compact"
        );
    }
}
