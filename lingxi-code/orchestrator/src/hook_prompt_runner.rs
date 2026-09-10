//! Orchestrator-side [`hooks::HookPromptRunner`] implementation.
//!
//! The hooks crate runs a `prompt` hook (`execPromptHook.ts`) through the
//! [`hooks::HookPromptRunner`] seam WITHOUT depending on the api-client. This
//! adapter closes that seam from the orchestrator side: it reuses the exact
//! one-shot, non-streaming [`OrchestratorApiClient::messages_create`] call the
//! orchestrator already makes for its other non-conversational LLM passes
//! (compaction summary / conversation-title generation), so the prompt hook
//! rides the same provider / retry / telemetry plumbing.
//!
//! Wiring: the composition root constructs an [`ApiClientHookPromptRunner`]
//! over the same `Arc<dyn OrchestratorApiClient>` it hands the orchestrator,
//! then injects it via `HookExecutorImpl::with_prompt_runner` (an `Option`,
//! default `None`).
//!
//! Model resolution mirrors `getSmallFastModel()`
//! (`claude-code/src/utils/model/model.ts:36-37` →
//! `getDefaultHaikuModel():131-139`): the hook's `model` override wins; else
//! `$ANTHROPIC_SMALL_FAST_MODEL`; else `$ANTHROPIC_DEFAULT_HAIKU_MODEL`; else
//! the default Haiku 4.5 string.

use std::sync::Arc;

use async_trait::async_trait;
use hooks::{HookPromptRunner, PromptHookError, PromptHookRequest};
use llm_client::{ContentBlock as LlmContentBlock, LlmError, LlmResponse};
use protocol::{ConversationMessage, MessageId};

use crate::conversation::OrchestratorApiClient;

/// Default small-fast model when no override / env var is set
/// (`getDefaultHaikuModel()` → `getModelStrings().haiku45`;
/// `model.ts:137`). Matches the orchestrator's `list_available_models` haiku id.
const DEFAULT_SMALL_FAST_MODEL: &str = "claude-haiku-4-5";

/// Implements [`HookPromptRunner`] over the orchestrator's one-shot
/// non-streaming `messages_create` seam.
pub struct ApiClientHookPromptRunner {
    api: Arc<dyn OrchestratorApiClient>,
}

impl ApiClientHookPromptRunner {
    /// Build a runner over the shared api-client handle. Pass the SAME
    /// `Arc<dyn OrchestratorApiClient>` the orchestrator uses so the prompt hook
    /// shares the provider routing / auth / telemetry.
    #[must_use]
    pub fn new(api: Arc<dyn OrchestratorApiClient>) -> Self {
        Self { api }
    }

    /// Resolve the effective model: the hook's override, then
    /// `ANTHROPIC_SMALL_FAST_MODEL`, then `ANTHROPIC_DEFAULT_HAIKU_MODEL`, then
    /// the default Haiku string (`getSmallFastModel()`; `model.ts:36-37`).
    fn resolve_model(override_model: Option<&str>) -> String {
        if let Some(m) = override_model {
            return m.to_string();
        }
        if let Ok(m) = std::env::var("ANTHROPIC_SMALL_FAST_MODEL") {
            if !m.is_empty() {
                return m;
            }
        }
        if let Ok(m) = std::env::var("ANTHROPIC_DEFAULT_HAIKU_MODEL") {
            if !m.is_empty() {
                return m;
            }
        }
        DEFAULT_SMALL_FAST_MODEL.to_string()
    }

    /// Concatenate the assistant message's text blocks (the analog of
    /// `extractTextContent(response.message.content)`; `execPromptHook.ts:105`).
    /// `Text` and `ConnectorText` blocks contribute; tool-use / reasoning blocks
    /// are ignored, matching `extractTextContent`'s text-only projection.
    fn extract_text(response: &LlmResponse) -> String {
        let mut out = String::new();
        for block in &response.content {
            match block {
                LlmContentBlock::Text { text, .. } => out.push_str(text),
                LlmContentBlock::ConnectorText { connector_text, .. } => {
                    out.push_str(connector_text);
                }
                _ => {}
            }
        }
        out
    }

    /// Map an [`LlmError`] to a [`PromptHookError`]. A transport timeout becomes
    /// [`PromptHookError::Timeout`] (the `execPromptHook.ts` aborted-signal
    /// path); every other failure is a [`PromptHookError::Query`]
    /// (`outcome: 'non_blocking_error'`).
    fn map_error(err: LlmError) -> PromptHookError {
        match err {
            LlmError::Transport { ref message }
                if message.contains("timeout") || message.contains("Timeout") =>
            {
                // Best-effort: `LlmError::Transport` doesn't carry a Duration,
                // so we synthesize a zero-duration timeout for the hook error.
                PromptHookError::Timeout(std::time::Duration::ZERO)
            }
            other => PromptHookError::Query(other.to_string()),
        }
    }
}

#[async_trait]
impl HookPromptRunner for ApiClientHookPromptRunner {
    async fn run(&self, req: PromptHookRequest) -> Result<String, PromptHookError> {
        let model = Self::resolve_model(req.model.as_deref());
        // Single user turn carrying the (already `$ARGUMENTS`-substituted)
        // hook prompt; the fixed evaluation system prompt is passed via
        // `system`. No tools are advertised — the prompt hook only needs the
        // model's `{ok, reason?}` JSON text (`execPromptHook.ts:62-100`).
        let hooks::PromptHookTranscript {
            messages: transcript,
            last_usage_tokens: last_usage,
            message_grouping,
        } = match req.transcript {
            Some(transcript) => transcript,
            None => load_hook_transcript(req.transcript_path.as_deref()).await?,
        };
        let budget = hook_transcript_budget(&model);
        let query = async {
            let mut messages = if last_usage <= budget {
                transcript.clone()
            } else {
                bound_hook_transcript(&transcript, budget, &message_grouping)
            };
            messages.push(ConversationMessage::user(
                MessageId::new(),
                req.prompt.clone(),
            ));
            let response = self
                .api
                .messages_create_hook_prompt(&model, &req.system_prompt, messages)
                .await;
            let response = match response {
                Err(LlmError::ContextOverflow { .. }) if !transcript.is_empty() => {
                    let mut messages = if last_usage <= budget / 2 {
                        transcript.clone()
                    } else {
                        bound_hook_transcript(&transcript, budget / 2, &message_grouping)
                    };
                    messages.push(ConversationMessage::user(
                        MessageId::new(),
                        req.prompt.clone(),
                    ));
                    self.api
                        .messages_create_hook_prompt(&model, &req.system_prompt, messages)
                        .await
                }
                other => other,
            };
            response.map_err(Self::map_error)
        };
        let query = llm_client::thinking_scope::scope_thinking_recovery(
            llm_client::thinking_scope::ThinkingRecoveryScope::default(),
            query,
        );
        let response = tokio::time::timeout(req.timeout, query)
            .await
            .map_err(|_| PromptHookError::Timeout(req.timeout))??;
        Ok(Self::extract_text(&response))
    }
}

fn hook_transcript_budget(model: &str) -> usize {
    use llm_client::model::context_window::{has_1m_context, model_native_1m};
    if has_1m_context(model) || model_native_1m(model) {
        500_000
    } else {
        100_000
    }
}

/// Restore the host-selected transcript through the same tolerant, branch-aware
/// reader as resume. A missing transcript is empty; other I/O errors fail the
/// evaluator instead of silently judging an incomplete view.
async fn load_hook_transcript(
    path: Option<&std::path::Path>,
) -> Result<hooks::PromptHookTranscript, PromptHookError> {
    let Some(path) = path else {
        return Ok(hooks::PromptHookTranscript::default());
    };
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(hooks::PromptHookTranscript::default())
        }
        Err(e) => {
            return Err(PromptHookError::Query(format!(
                "Cannot read hook transcript: {e}"
            )));
        }
    };
    let loaded = session::jsonl::reader::route_lines(&text);
    let session_id = loaded
        .messages_in_order
        .last()
        .map(|m| m.session_id.as_str())
        .unwrap_or("");
    let (chain, _) = session::jsonl::loader::build_conversation_chain(&loaded, session_id);
    let rows = if chain.is_empty() {
        &loaded.messages_in_order
    } else {
        &chain
    };
    let usage = rows
        .iter()
        .rev()
        .find(|r| {
            r.message_type == "assistant"
                && r.message.get("usage").is_some()
                && r.message.get("model").and_then(serde_json::Value::as_str) != Some("<synthetic>")
        })
        .and_then(|r| r.message.get("usage"));
    let last_usage = usage
        .map(|usage| {
            [
                "input_tokens",
                "output_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
            ]
            .iter()
            .map(|key| {
                usize::try_from(
                    usage
                        .get(*key)
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                )
                .unwrap_or(usize::MAX)
            })
            .fold(0usize, usize::saturating_add)
        })
        .unwrap_or(0);
    let state = crate::resume::state_from_messages(uuid::Uuid::nil(), rows);
    Ok(hooks::PromptHookTranscript {
        messages: state.history,
        last_usage_tokens: last_usage,
        message_grouping: state.hook_message_grouping,
    })
}

// 2.1.263 `vc` / `C0` / `qXr`: UTF-16 Math.round(length / 4),
// fixed media charge, and recursively sized tool-result bodies.
fn estimate_hook_content(value: &serde_json::Value) -> usize {
    fn text(value: &str) -> usize {
        (value.encode_utf16().count() + 2) / 4
    }
    if let Some(value) = value.as_str() {
        return text(value);
    }
    if let Some(values) = value.as_array() {
        return values.iter().map(estimate_hook_content).sum();
    }
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("image" | "document") => 2000,
        Some("text") => value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(text)
            .unwrap_or(0),
        Some("thinking") => value
            .get("thinking")
            .and_then(serde_json::Value::as_str)
            .map(text)
            .unwrap_or(0),
        Some("redacted_thinking") => value
            .get("data")
            .and_then(serde_json::Value::as_str)
            .map(text)
            .unwrap_or(0),
        Some("tool_result") => value.get("content").map(estimate_hook_content).unwrap_or(0),
        Some("tool_use") => text(&format!(
            "{}{}",
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
            value
                .get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}))
        )),
        _ => text(&value.to_string()),
    }
}

/// Keep complete assistant turns, including their following tool results. The
/// latest turn is always retained, even if it alone exceeds the budget (`Wps`).
fn bound_hook_transcript(
    messages: &[ConversationMessage],
    token_budget: usize,
    metadata: &std::collections::HashMap<MessageId, (bool, bool)>,
) -> Vec<ConversationMessage> {
    let mut groups = Vec::new();
    let mut start = 0;
    let mut assistant_id = None;
    for (index, message) in messages.iter().enumerate() {
        if let ConversationMessage::Assistant { id, .. } = message {
            let (is_virtual, resumed) = metadata.get(id).copied().unwrap_or_default();
            if is_virtual {
                continue;
            }
            if index > start && assistant_id != Some(*id) && !resumed {
                groups.push(start..index);
                start = index;
            }
            assistant_id = Some(*id);
        }
    }
    if start < messages.len() {
        groups.push(start..messages.len());
    }
    let mut kept = messages.len();
    let mut tokens = 0;
    for group in groups.into_iter().rev() {
        let size: usize = messages[group.clone()]
            .iter()
            .map(|m| match m {
                ConversationMessage::User { content, .. }
                | ConversationMessage::Assistant { content, .. } => {
                    estimate_hook_content(&serde_json::to_value(content).unwrap_or_default())
                }
                _ => serde_json::to_string(m)
                    .map(|s| s.encode_utf16().count().div_ceil(4))
                    .unwrap_or(0),
            })
            .sum();
        if kept < messages.len() && tokens + size > token_budget {
            break;
        }
        tokens += size;
        kept = group.start;
    }
    if kept == 0 || messages.is_empty() {
        return messages.to_vec();
    }
    let mut result = vec![ConversationMessage::user(
        MessageId::new(),
        format!(
            "[Earlier conversation truncated to fit the hook evaluator's context window — {kept} earlier messages omitted. Evaluate the condition against the recent transcript below; if the required evidence may be in the omitted prefix, return {{\"ok\": false, \"reason\": \"insufficient evidence in transcript\"}}.]"
        ),
    )];
    result.extend_from_slice(&messages[kept..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{LlmResponse, Usage};
    use std::sync::Mutex;

    /// One recorded `messages_create` call: `(model, system, messages)`.
    type RecordedCall = (String, Option<String>, Vec<ConversationMessage>);

    fn make_text_response(body: &str) -> LlmResponse {
        LlmResponse {
            id: "msg_1".into(),
            model: "claude-haiku-4-5".into(),
            content: vec![LlmContentBlock::Text {
                text: body.into(),
                cache_control: None,
            }],
            stop_reason: Some("end_turn".into()),
            stop_details: None,
            usage: Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    /// Records each `messages_create` call and returns a scripted response.
    struct MockApi {
        recorded: Mutex<Vec<RecordedCall>>,
        response: Mutex<Option<Result<LlmResponse, LlmError>>>,
    }
    impl MockApi {
        fn text(model_echo: &str, body: &str) -> Arc<Self> {
            let _ = model_echo;
            Arc::new(Self {
                recorded: Mutex::new(Vec::new()),
                response: Mutex::new(Some(Ok(make_text_response(body)))),
            })
        }
    }
    #[async_trait]
    impl OrchestratorApiClient for MockApi {
        async fn messages_create(
            &self,
            model: &str,
            _profile: Option<&str>,
            system: Option<&str>,
            msgs: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<LlmResponse, LlmError> {
            self.recorded.lock().unwrap().push((
                model.to_string(),
                system.map(str::to_owned),
                msgs,
            ));
            self.response
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(LlmError::Transport {
                    message: "exhausted".into(),
                }))
        }
    }

    fn req(prompt: &str, model: Option<&str>) -> PromptHookRequest {
        PromptHookRequest {
            transcript: None,
            transcript_path: None,
            prompt: prompt.into(),
            system_prompt: "SYS".into(),
            model: model.map(str::to_owned),
            timeout: std::time::Duration::from_secs(30),
        }
    }

    #[test]
    fn transcript_content_estimator_matches_c0_utf16_and_media() {
        assert_eq!(estimate_hook_content(&serde_json::json!("😀😀😀")), 2);
        assert_eq!(
            estimate_hook_content(
                &serde_json::json!([{"type":"image"},{"type":"tool_result","content":"123456"}])
            ),
            2002
        );
    }

    #[tokio::test]
    async fn transcript_reader_replays_host_path_and_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let sid = uuid::Uuid::new_v4().to_string();
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let rows = [
            serde_json::json!({"type":"user","uuid":first,"parentUuid":null,"sessionId":sid,"message":{"role":"user","content":"run tests"}}),
            serde_json::json!({"type":"assistant","uuid":second,"parentUuid":first,"sessionId":sid,"message":{"role":"assistant","content":[{"type":"text","text":"tests passed"}],"usage":{"input_tokens":10,"output_tokens":5}}}),
        ];
        tokio::fs::write(
            &path,
            rows.iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .await
        .unwrap();
        let transcript = load_hook_transcript(Some(&path)).await.unwrap();
        let messages = transcript.messages;
        let usage = transcript.last_usage_tokens;
        assert_eq!(messages.len(), 2);
        assert_eq!(usage, 15);
        assert!(
            matches!(&messages[1], ConversationMessage::Assistant { content, .. } if matches!(&content[0], protocol::ContentBlock::Text { text } if text == "tests passed"))
        );
    }

    #[test]
    fn transcript_budget_keeps_last_assistant_and_its_tool_results() {
        let messages = vec![
            ConversationMessage::user(MessageId::new(), "old".repeat(100)),
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![protocol::ContentBlock::Text {
                    text: "latest".into(),
                }],
                stop_reason: None,
            },
            ConversationMessage::user(MessageId::new(), "tool result".into()),
        ];
        let kept = bound_hook_transcript(&messages, 1, &Default::default());
        assert_eq!(kept.len(), 3);
        assert!(matches!(&kept[1], ConversationMessage::Assistant { .. }));
        let ConversationMessage::User { content, .. } = &kept[0] else {
            panic!("truncation preface")
        };
        assert!(
            matches!(&content[0], protocol::ContentBlock::Text { text } if text.contains("1 earlier messages omitted"))
        );
        assert!(bound_hook_transcript(&[], 1, &Default::default()).is_empty());
    }

    #[test]
    fn transcript_budget_preserves_split_assistant_identity_across_tool_results() {
        let id = MessageId::new();
        let assistant = |text: &str| ConversationMessage::Assistant {
            id,
            content: vec![protocol::ContentBlock::Text { text: text.into() }],
            stop_reason: None,
        };
        let messages = vec![
            ConversationMessage::user(MessageId::new(), "old".repeat(100)),
            assistant("first split block"),
            ConversationMessage::user(MessageId::new(), "result".into()),
            assistant("second split block"),
        ];
        let kept = bound_hook_transcript(&messages, 1, &Default::default());
        assert_eq!(kept.len(), 4);
        assert_eq!(&kept[1..], &messages[1..]);
    }

    #[test]
    fn resumed_and_virtual_rows_keep_original_grouping_after_resume_projection() {
        let sid = uuid::Uuid::new_v4().to_string();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let mut rows = vec![
            serde_json::json!({"type":"user","uuid":ids[0],"sessionId":sid,"message":{"role":"user","content":"old".repeat(100)}}),
        ];
        for (index, (virtual_row, resumed)) in [(false, false), (true, false), (false, true)]
            .into_iter()
            .enumerate()
        {
            rows.push(serde_json::json!({"type":"assistant","uuid":ids[index + 1],"sessionId":sid,"isVirtual":virtual_row,"resumedFromIncompleteThinking":resumed,"message":{"id":ids[index + 1],"role":"assistant","content":[{"type":"text","text":"part"}]}}));
        }
        let loaded = session::jsonl::reader::route_lines(
            &rows
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let state =
            crate::resume::state_from_messages(uuid::Uuid::nil(), &loaded.messages_in_order);
        assert_eq!(state.hook_message_grouping.len(), 2);
        let kept = bound_hook_transcript(&state.history, 1, &state.hook_message_grouping);
        assert_eq!(kept.len(), 4);
        assert_eq!(&kept[1..], &state.history[1..]);
    }

    #[test]
    fn evaluator_budget_recognizes_native_1m_without_suffix() {
        assert_eq!(hook_transcript_budget("claude-opus-4-8"), 500_000);
        assert_eq!(hook_transcript_budget("claude-sonnet-5"), 500_000);
        assert_eq!(hook_transcript_budget("claude-haiku-4-5"), 100_000);
    }

    #[tokio::test]
    async fn live_transcript_wins_over_unreadable_persisted_path() {
        let api = MockApi::text("x", r#"{"ok":true}"#);
        let runner = ApiClientHookPromptRunner::new(api.clone());
        let dir = tempfile::tempdir().unwrap();
        let mut request = req("judge", Some("claude-opus-4-8"));
        // A directory is not a readable JSONL file. A live snapshot must never
        // consult it, even when the writer hasn't persisted the new turn yet.
        request.transcript_path = Some(dir.path().to_path_buf());
        let messages = vec![
            ConversationMessage::user(MessageId::new(), "unpersisted evidence".into()),
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![protocol::ContentBlock::Text {
                    text: "just completed".into(),
                }],
                stop_reason: None,
            },
        ];
        request.transcript = Some(hooks::PromptHookTranscript {
            messages: messages.clone(),
            last_usage_tokens: 300_000,
            ..Default::default()
        });
        runner.run(request).await.unwrap();
        let recorded = api.recorded.lock().unwrap();
        assert_eq!(&recorded[0].2[..2], messages.as_slice());
        assert_eq!(
            recorded[0].2.len(),
            3,
            "native 1M keeps the full transcript at 300k usage"
        );
    }

    #[tokio::test]
    async fn run_calls_messages_create_with_prompt_and_system_and_extracts_text() {
        let api = MockApi::text("claude-haiku-4-5", r#"{"ok": true}"#);
        let runner = ApiClientHookPromptRunner::new(api.clone());

        let out = runner.run(req("is this safe?", None)).await.unwrap();

        assert_eq!(out, r#"{"ok": true}"#);
        let recorded = api.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        let (model, system, msgs) = &recorded[0];
        // Default model resolves to the small-fast haiku string (no env set in
        // the typical test environment).
        assert_eq!(model, "claude-haiku-4-5");
        assert_eq!(system.as_deref(), Some("SYS"));
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            ConversationMessage::User { content, .. } => {
                assert_eq!(
                    content,
                    &vec![protocol::ContentBlock::Text {
                        text: "is this safe?".into()
                    }]
                );
            }
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_override_is_passed_through() {
        let api = MockApi::text("x", r#"{"ok": true}"#);
        let runner = ApiClientHookPromptRunner::new(api.clone());

        let _ = runner
            .run(req("p", Some("claude-sonnet-4-6")))
            .await
            .unwrap();

        let recorded = api.recorded.lock().unwrap();
        assert_eq!(recorded[0].0, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn timeout_error_maps_to_prompt_timeout() {
        let api = Arc::new(MockApi {
            recorded: Mutex::new(Vec::new()),
            response: Mutex::new(Some(Err(LlmError::Transport {
                message: "request timeout after 30s".into(),
            }))),
        });
        let runner = ApiClientHookPromptRunner::new(api);

        let err = runner.run(req("p", None)).await.unwrap_err();
        assert!(matches!(err, PromptHookError::Timeout(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn other_error_maps_to_query_error() {
        let api = Arc::new(MockApi {
            recorded: Mutex::new(Vec::new()),
            response: Mutex::new(Some(Err(LlmError::Authentication {
                message: String::new(),
            }))),
        });
        let runner = ApiClientHookPromptRunner::new(api);

        let err = runner.run(req("p", None)).await.unwrap_err();
        assert!(matches!(err, PromptHookError::Query(_)), "got {err:?}");
    }

    #[test]
    fn resolve_model_prefers_override() {
        assert_eq!(
            ApiClientHookPromptRunner::resolve_model(Some("custom-model")),
            "custom-model"
        );
    }
}
