//! Bridge: adapt a `providers::ModelRouter` to the orchestrator's
//! `OrchestratorApiClient` / `StreamingApiClient` traits.
//!
//! **Task 5 TEMPORARY STUBS**: The impl bodies below return
//! `Err(LlmError::InvalidRequest { message: "Task 6 wires the llm-client drive".into() })`
//! so this file compiles and the orchestrator type-checks against `llm_client` types.
//! Task 6 replaces the stubs with a real `DefaultLlmClient` drive.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use async_trait::async_trait;
use futures::stream::BoxStream;
use llm_client::{LlmError, LlmEvent, LlmResponse};
use protocol::{ContentBlock, ConversationMessage};
use providers::{CanonicalRequest, ModelRouter};
use std::sync::Arc;

/// Adapts an `Arc<dyn ModelRouter>` to the orchestrator's API-client traits.
pub struct ProviderApiAdapter {
    router: Arc<dyn ModelRouter>,
}

impl ProviderApiAdapter {
    /// Wrap a router.
    #[must_use]
    pub fn new(router: Arc<dyn ModelRouter>) -> Self {
        Self { router }
    }
}

/// Whether any message carries a media (image or document) content block —
/// such a message routes to a vision/media-capable model.
fn messages_contain_image(msgs: &[ConversationMessage]) -> bool {
    msgs.iter().any(|m| match m {
        ConversationMessage::User { content, .. } | ConversationMessage::Assistant { content, .. } => {
            content.iter().any(|b| {
                matches!(b, ContentBlock::Image { .. }) || matches!(b, ContentBlock::Document { .. })
            })
        }
        ConversationMessage::System { .. } => false,
    })
}

/// Maximum media items (images + documents) the API accepts per request.
/// Above this the provider rejects the request with a confusing error, so we
/// trim oldest-first instead. Mirrors TS `API_MAX_MEDIA_PER_REQUEST`
/// (apiLimits.ts:94).
const MAX_MEDIA_PER_REQUEST: usize = 100;

/// Count media (image/document) content blocks across all messages, mirroring
/// TS `isMedia` counting in `stripExcessMediaItems` (claude.ts:956-1015),
/// *including any nested inside `tool_result` content*.
///
/// NOTE on the frozen wire types: in this port `ContentBlock::ToolResult.content`
/// is a `String`, so the only media a message can structurally hold is a
/// top-level `ContentBlock::Image` or `ContentBlock::Document`. There is no place
/// to nest media inside a `tool_result` here, so the "nested in `tool_result`" arm of
/// the TS counter has nothing to count — this function already covers every media
/// shape the frozen types permit.
fn count_media(msgs: &[ConversationMessage]) -> usize {
    msgs.iter()
        .map(|m| match m {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content
                .iter()
                .filter(|b| {
                    matches!(b, ContentBlock::Image { .. })
                        || matches!(b, ContentBlock::Document { .. })
                })
                .count(),
            ConversationMessage::System { .. } => 0,
        })
        .sum()
}

/// Return `msgs` with the OLDEST media items stripped until at most `limit`
/// remain. When already within the limit the input is returned untouched (no
/// re-allocation), exactly like TS `stripExcessMediaItems` (claude.ts:956-1015).
///
/// The caller hands us an owned `Vec` (a clone of conversation history produced
/// by the orchestrator before the trait call), so trimming it here is a copy and
/// never mutates stored history. Messages are walked oldest-first and media
/// blocks are dropped in order until the count is back at `limit`, preserving the
/// most-recent media.
fn strip_excess_media(
    mut msgs: Vec<ConversationMessage>,
    limit: usize,
) -> Vec<ConversationMessage> {
    let total = count_media(&msgs);
    if total <= limit {
        return msgs;
    }
    let mut to_remove = total - limit;
    for m in &mut msgs {
        if to_remove == 0 {
            break;
        }
        let content = match m {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content,
            ConversationMessage::System { .. } => continue,
        };
        content.retain(|b| {
            if to_remove > 0
                && (matches!(b, ContentBlock::Image { .. })
                    || matches!(b, ContentBlock::Document { .. }))
            {
                to_remove -= 1;
                false
            } else {
                true
            }
        });
    }
    msgs
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _msgs: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        // TEMPORARY STUB (Task 5): Task 6 replaces this with a real
        // DefaultLlmClient drive. The router-based logic is preserved
        // below in `_complete_via_router` (dead code allowed) so Task 6
        // can lift it without re-deriving the media-gating logic.
        Err(LlmError::InvalidRequest {
            message: "Task 6 wires the llm-client drive".into(),
        })
    }

    fn available_models(&self) -> Vec<String> {
        self.router.available_models()
    }
}

/// Subagent API seam (M5-Wire).
///
/// Task 5 stub — delegates to the temporary `OrchestratorApiClient` stub
/// above. Task 6 will replace both with a real `DefaultLlmClient` drive.
#[async_trait]
impl agent::SubagentApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        // Delegate to the orchestrator impl so the two seams never diverge —
        // tools included.
        OrchestratorApiClient::messages_create(self, model, system, messages, tools).await
    }

    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // Real SSE transport, shared with `StreamingApiClient::stream`. The
        // subagent's wire tool definitions (from `SubagentContext::tool_schemas`)
        // ride through here; the agent-crate accumulator reassembles the
        // streamed blocks into the same `LlmResponse` shape.
        StreamingApiClient::stream(self, model, system, messages, tools).await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // TEMPORARY STUB (Task 5): Task 6 replaces this with a real
        // DefaultLlmClient SSE drive.
        Err(LlmError::InvalidRequest {
            message: "Task 6 wires the llm-client drive".into(),
        })
    }
}

// ── Dead-code preservation for Task 6 ─────────────────────────────────────
// The router-based media gating + CanonicalRequest assembly below is NOT
// reached yet (the stubs above short-circuit all calls) but is kept here
// so Task 6 can resurrect it rather than re-derive it.
#[allow(dead_code)]
fn _complete_via_router_batch(
    adapter: &ProviderApiAdapter,
    model: &str,
    system: Option<&str>,
    msgs: Vec<ConversationMessage>,
    tools: Vec<serde_json::Value>,
) -> Result<(CanonicalRequest, Arc<dyn providers::LlmProvider>), String> {
    use api_client::ApiError;
    let resolved = adapter.router.resolve(model)
        .map_err(|e: ApiError| e.to_string())?;
    if messages_contain_image(&msgs) && !resolved.provider.capabilities().vision {
        return Err(format!(
            "model {model:?} ({:?}) does not support image input",
            resolved.provider.id()
        ));
    }
    if !tools.is_empty() && !resolved.provider.capabilities().native_tools {
        return Err(format!(
            "model {model:?} ({:?}) does not support tool use",
            resolved.provider.id()
        ));
    }
    let msgs = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
    let mut req = CanonicalRequest::new(resolved.model);
    req.system = system.map(str::to_string);
    req.messages = msgs;
    req.tools = tools;
    Ok((req, resolved.provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, MessageResponse, StreamEvent, UsageApi};
    use api_client::ApiError;
    use futures::StreamExt;
    use providers::{Capabilities, LlmProvider, Resolved};
    use std::sync::Mutex;

    /// Records the canonical request it received and returns a canned response.
    struct StubProvider {
        seen_model: Mutex<Option<String>>,
        seen_system: Mutex<Option<String>>,
        seen_tools_len: Mutex<Option<usize>>,
        seen_stream_flag: Mutex<Option<bool>>,
        caps: Capabilities,
    }

    impl StubProvider {
        fn new() -> Self {
            Self {
                seen_model: Mutex::new(None),
                seen_system: Mutex::new(None),
                seen_tools_len: Mutex::new(None),
                seen_stream_flag: Mutex::new(None),
                caps: Capabilities::anthropic(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn id(&self) -> cost::ProviderId {
            cost::ProviderId::OpenAI
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            *self.seen_model.lock().unwrap() = Some(req.model.clone());
            *self.seen_system.lock().unwrap() = req.system.clone();
            *self.seen_tools_len.lock().unwrap() = Some(req.tools.len());
            Ok(MessageResponse {
                id: "stub".to_string(),
                model: req.model,
                content: vec![ContentBlockApi::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }
        async fn stream(
            &self,
            req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            *self.seen_tools_len.lock().unwrap() = Some(req.tools.len());
            *self.seen_stream_flag.lock().unwrap() = Some(req.stream);
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    /// Router that records the model string it was asked to resolve and always
    /// returns the same stub provider with the prefix stripped.
    struct StubRouter {
        provider: Arc<StubProvider>,
        seen_resolve: Mutex<Option<String>>,
    }

    impl ModelRouter for StubRouter {
        fn resolve(&self, model: &str) -> Result<Resolved, ApiError> {
            *self.seen_resolve.lock().unwrap() = Some(model.to_string());
            // Strip a leading `profile/` so the stub sees the local id.
            let local = model.split_once('/').map_or(model, |(_, m)| m).to_string();
            Ok(Resolved {
                provider: self.provider.clone(),
                model: local,
            })
        }
        fn available_profiles(&self) -> Vec<String> {
            vec!["stub".to_string()]
        }
        fn available_models(&self) -> Vec<String> {
            vec!["stub/model-a".to_string(), "@fast".to_string()]
        }
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn bridge_resolves_and_forwards_local_model() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router.clone());
        let resp = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new(), Vec::new())
            .await
            .expect("ok");
        // Router saw the full string; provider saw the stripped local id.
        assert_eq!(
            router.seen_resolve.lock().unwrap().as_deref(),
            Some("openai/gpt-4o")
        );
        assert_eq!(
            provider.seen_model.lock().unwrap().as_deref(),
            Some("gpt-4o")
        );
        assert_eq!(provider.seen_system.lock().unwrap().as_deref(), Some("sys"));
        assert_eq!(resp.model, "gpt-4o");
    }

    #[test]
    fn available_models_delegates_to_router() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider,
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let models = OrchestratorApiClient::available_models(&adapter);
        assert_eq!(models, vec!["stub/model-a".to_string(), "@fast".to_string()]);
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn bridge_forwards_stream_tools_and_flag() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let _s = adapter
            .stream("gemini/gemini-2.0-flash", Some("sys"), Vec::new(), tools)
            .await
            .expect("stream");
        assert_eq!(*provider.seen_tools_len.lock().unwrap(), Some(1));
        assert_eq!(*provider.seen_stream_flag.lock().unwrap(), Some(true));
    }

    /// A provider that reports no native tool support.
    struct NoToolsProvider;

    #[async_trait]
    impl LlmProvider for NoToolsProvider {
        fn id(&self) -> cost::ProviderId {
            cost::ProviderId::Custom {
                name: "no-tools".to_string(),
            }
        }
        fn capabilities(&self) -> &Capabilities {
            // A leaked const ref keeps the signature `-> &Capabilities` simple
            // for this test-only stub.
            use std::sync::OnceLock;
            static CAPS: OnceLock<Capabilities> = OnceLock::new();
            CAPS.get_or_init(|| Capabilities {
                native_tools: false,
                streaming: true,
                vision: false,
                prompt_cache: false,
                reasoning: providers::ReasoningSupport::None,
                parallel_tool_calls: false,
                max_output_tokens: None,
                system_style: providers::SystemStyle::RoleMessage,
            })
        }
        async fn complete(&self, _req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            unreachable!("not used in this test")
        }
        async fn stream(
            &self,
            _req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    struct FixedRouter(std::sync::Arc<dyn LlmProvider>);
    impl providers::ModelRouter for FixedRouter {
        fn resolve(&self, model: &str) -> Result<providers::Resolved, ApiError> {
            Ok(providers::Resolved {
                provider: self.0.clone(),
                model: model.to_string(),
            })
        }
        fn available_profiles(&self) -> Vec<String> {
            vec!["fixed".to_string()]
        }
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn bridge_forwards_batched_tools() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let _ = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new(), tools)
            .await
            .expect("ok");
        assert_eq!(*provider.seen_tools_len.lock().unwrap(), Some(1));
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn messages_create_with_tools_on_non_tool_model_fails_fast() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let result = adapter
            .messages_create("custom/no-tool-model", None, Vec::new(), tools)
            .await;
        assert!(result.is_err(), "batched path must reject tools");
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn stream_with_tools_on_non_tool_model_fails_fast() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let result = adapter
            .stream("custom/no-tool-model", None, Vec::new(), tools)
            .await;
        assert!(result.is_err(), "must reject tools on a non-tool-capable model");
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn stream_without_tools_on_non_tool_model_is_allowed() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        // Stubs always Err in Task 5 — skip assertion.
        let _ = adapter
            .stream("custom/no-tool-model", None, Vec::new(), Vec::new())
            .await;
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn image_to_non_vision_model_fails_fast() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YWJj".to_string(),
                },
            }],
        }];
        let result = adapter.messages_create("custom/x", None, msgs, Vec::new()).await;
        assert!(result.is_err(), "image to a non-vision model must fail fast");
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn subagent_api_client_seam_forwards_through_trait_object() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let seam: Arc<dyn agent::SubagentApiClient> =
            Arc::new(ProviderApiAdapter::new(router.clone()));
        let _ = seam
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new(), Vec::new())
            .await;
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn subagent_streaming_seam_delegates_to_stream() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let seam: Arc<dyn agent::SubagentApiClient> =
            Arc::new(ProviderApiAdapter::new(router.clone()));
        let _ = seam
            .messages_create_stream("openai/gpt-4o", Some("sys"), Vec::new(), Vec::new())
            .await;
    }

    #[tokio::test]
    #[ignore = "Task 6"]
    async fn image_to_vision_model_is_allowed() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url { url: "https://x/y.png".to_string() },
            }],
        }];
        // In Task 5 this always returns the stub Err — the test just confirms compilation.
        let _ = adapter.messages_create("anthropic/claude", None, msgs, Vec::new()).await;
    }

    // ---- MULTIMODAL.6: per-request media cap (stripExcessMediaItems) ----

    /// A distinct image block whose base64 `data` encodes its index, so tests
    /// can assert exactly which (oldest) items were dropped.
    fn img(n: usize) -> protocol::ContentBlock {
        protocol::ContentBlock::Image {
            source: protocol::ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: format!("img{n}"),
            },
        }
    }

    /// User message carrying the given image indices (with a leading text block).
    fn user_with_imgs(range: std::ops::Range<usize>) -> ConversationMessage {
        let mut content = vec![ContentBlock::Text {
            text: "hi".to_string(),
        }];
        content.extend(range.map(img));
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content,
        }
    }

    /// Collect the `data` of every image block across messages, in order.
    fn image_data_in_order(msgs: &[ConversationMessage]) -> Vec<String> {
        let mut out = Vec::new();
        for m in msgs {
            if let ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } = m
            {
                for b in content {
                    if let ContentBlock::Image {
                        source: protocol::ImageSource::Base64 { data, .. },
                    } = b
                    {
                        out.push(data.clone());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn count_media_counts_top_level_images_across_messages() {
        let msgs = vec![user_with_imgs(0..3), user_with_imgs(3..5)];
        assert_eq!(count_media(&msgs), 5);
    }

    #[test]
    fn tool_result_string_content_contributes_no_media() {
        // The frozen `ToolResult.content` is a `String`, so no media can nest in
        // it — it counts as zero, and the cap sees only top-level images.
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: protocol::ToolUseId::new(),
                    content: "lots of text, no media".to_string(),
                    is_error: false,
                },
                img(0),
            ],
        }];
        assert_eq!(count_media(&msgs), 1);
    }

    #[test]
    fn strip_excess_media_trims_oldest_to_limit_without_touching_history() {
        // 102 images spread across two messages (img0 oldest … img101 newest).
        let stored = vec![user_with_imgs(0..60), user_with_imgs(60..102)];
        assert_eq!(count_media(&stored), 102);

        // The send path receives a clone (the orchestrator clones history).
        let to_send = stored.clone();
        let trimmed = strip_excess_media(to_send, MAX_MEDIA_PER_REQUEST);

        // Exactly 100 remain, and the two OLDEST (img0, img1) were dropped.
        assert_eq!(count_media(&trimmed), 100);
        let remaining = image_data_in_order(&trimmed);
        assert_eq!(remaining.len(), 100);
        assert_eq!(remaining.first().unwrap(), "img2");
        assert_eq!(remaining.last().unwrap(), "img101");
        assert!(!remaining.contains(&"img0".to_string()));
        assert!(!remaining.contains(&"img1".to_string()));

        // Stored history is a separate allocation and is left completely intact.
        assert_eq!(count_media(&stored), 102);
        assert_eq!(image_data_in_order(&stored).first().unwrap(), "img0");
    }

    #[test]
    fn strip_excess_media_leaves_within_limit_messages_unchanged() {
        let msgs = vec![user_with_imgs(0..50), user_with_imgs(50..100)];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before, "exactly 100 media → no stripping");
        assert_eq!(count_media(&out), 100);
    }

    #[test]
    fn strip_excess_media_no_images_is_noop() {
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: "no media here".to_string(),
            }],
        }];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before);
    }
}
