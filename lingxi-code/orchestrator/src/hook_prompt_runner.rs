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

use api_client::types::{ContentBlockApi, MessageResponse};
use api_client::ApiError;
use async_trait::async_trait;
use hooks::{HookPromptRunner, PromptHookError, PromptHookRequest};
use protocol::{ConversationMessage, MessageId};
use traits::HttpError;

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
    /// `Text` and `ConnectorText` blocks contribute; tool-use / thinking blocks
    /// are ignored, matching `extractTextContent`'s text-only projection.
    fn extract_text(response: &MessageResponse) -> String {
        let mut out = String::new();
        for block in &response.content {
            match block {
                ContentBlockApi::Text { text } => out.push_str(text),
                ContentBlockApi::ConnectorText { connector_text, .. } => {
                    out.push_str(connector_text);
                }
                _ => {}
            }
        }
        out
    }

    /// Map an [`ApiError`] to a [`PromptHookError`]. A transport timeout becomes
    /// [`PromptHookError::Timeout`] (the `execPromptHook.ts` aborted-signal
    /// path); every other failure is a [`PromptHookError::Query`]
    /// (`outcome: 'non_blocking_error'`).
    fn map_error(err: ApiError) -> PromptHookError {
        match err {
            ApiError::Http(HttpError::Timeout(d)) => PromptHookError::Timeout(d),
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
        let messages = vec![ConversationMessage::user(MessageId::new(), req.prompt)];
        let response = self
            .api
            .messages_create(&model, Some(req.system_prompt.as_str()), messages, Vec::new())
            .await
            .map_err(Self::map_error)?;
        Ok(Self::extract_text(&response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::UsageApi;
    use std::sync::Mutex;

    /// One recorded `messages_create` call: `(model, system, messages)`.
    type RecordedCall = (String, Option<String>, Vec<ConversationMessage>);

    /// Records each `messages_create` call and returns a scripted response.
    struct MockApi {
        recorded: Mutex<Vec<RecordedCall>>,
        response: Mutex<Option<Result<MessageResponse, ApiError>>>,
    }
    impl MockApi {
        fn text(model_echo: &str, body: &str) -> Arc<Self> {
            let _ = model_echo;
            Arc::new(Self {
                recorded: Mutex::new(Vec::new()),
                response: Mutex::new(Some(Ok(MessageResponse {
                    id: "msg_1".into(),
                    model: "claude-haiku-4-5".into(),
                    content: vec![ContentBlockApi::Text { text: body.into() }],
                    stop_reason: Some("end_turn".into()),
                    usage: UsageApi::default(),
                }))),
            })
        }
    }
    #[async_trait]
    impl OrchestratorApiClient for MockApi {
        async fn messages_create(
            &self,
            model: &str,
            system: Option<&str>,
            msgs: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<MessageResponse, ApiError> {
            self.recorded
                .lock()
                .unwrap()
                .push((model.to_string(), system.map(str::to_owned), msgs));
            self.response
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(ApiError::UnexpectedStreamEnd))
        }
    }

    fn req(prompt: &str, model: Option<&str>) -> PromptHookRequest {
        PromptHookRequest {
            prompt: prompt.into(),
            system_prompt: "SYS".into(),
            model: model.map(str::to_owned),
            timeout: std::time::Duration::from_secs(30),
        }
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

        let _ = runner.run(req("p", Some("claude-sonnet-4-6"))).await.unwrap();

        let recorded = api.recorded.lock().unwrap();
        assert_eq!(recorded[0].0, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn timeout_error_maps_to_prompt_timeout() {
        let api = Arc::new(MockApi {
            recorded: Mutex::new(Vec::new()),
            response: Mutex::new(Some(Err(ApiError::Http(HttpError::Timeout(
                std::time::Duration::from_secs(30),
            ))))),
        });
        let runner = ApiClientHookPromptRunner::new(api);

        let err = runner.run(req("p", None)).await.unwrap_err();
        assert!(matches!(err, PromptHookError::Timeout(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn other_error_maps_to_query_error() {
        let api = Arc::new(MockApi {
            recorded: Mutex::new(Vec::new()),
            response: Mutex::new(Some(Err(ApiError::Unauthorized("bad key".into())))),
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
