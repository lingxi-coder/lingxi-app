//! Bridge: adapt a `providers::ModelRouter` to the orchestrator's
//! `OrchestratorApiClient` / `StreamingApiClient` traits. Each call parses the
//! model string, resolves the provider via the router, and delegates with the
//! provider-local model id.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::BoxStream;
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

/// Whether any message carries an image content block.
fn messages_contain_image(msgs: &[ConversationMessage]) -> bool {
    msgs.iter().any(|m| match m {
        ConversationMessage::User { content, .. } | ConversationMessage::Assistant { content, .. } => {
            content.iter().any(|b| matches!(b, ContentBlock::Image { .. }))
        }
        ConversationMessage::System { .. } => false,
    })
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        let resolved = self.router.resolve(model)?;
        if messages_contain_image(&msgs) && !resolved.provider.capabilities().vision {
            return Err(ApiError::Http(traits::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support image input; \
                 select a vision-capable model or remove images",
                resolved.provider.id()
            ))));
        }
        let mut req = CanonicalRequest::new(resolved.model);
        req.system = system.map(str::to_string);
        req.messages = msgs;
        resolved.provider.complete(req).await
    }

    fn available_models(&self) -> Vec<String> {
        self.router.available_models()
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let resolved = self.router.resolve(model)?;
        if messages_contain_image(&messages) && !resolved.provider.capabilities().vision {
            return Err(ApiError::Http(traits::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support image input; \
                 select a vision-capable model or remove images",
                resolved.provider.id()
            ))));
        }
        if !tools.is_empty() && !resolved.provider.capabilities().native_tools {
            return Err(ApiError::Http(traits::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support tool use; \
                 select a tool-capable model or run without tools",
                resolved.provider.id()
            ))));
        }
        let mut req = CanonicalRequest::new(resolved.model);
        req.system = system.map(str::to_string);
        req.messages = messages;
        req.tools = tools;
        req.stream = true;
        resolved.provider.stream(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, UsageApi};
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
    async fn bridge_resolves_and_forwards_local_model() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router.clone());
        let resp = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new())
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
    async fn stream_with_tools_on_non_tool_model_fails_fast() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let result = adapter
            .stream("custom/no-tool-model", None, Vec::new(), tools)
            .await;
        assert!(
            result.is_err(),
            "must reject tools on a non-tool-capable model"
        );
        let Err(err) = result else { panic!("expected Err") };
        assert!(matches!(
            err,
            ApiError::Http(traits::HttpError::InvalidRequest(_))
        ));
    }

    #[tokio::test]
    async fn stream_without_tools_on_non_tool_model_is_allowed() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let _s = adapter
            .stream("custom/no-tool-model", None, Vec::new(), Vec::new())
            .await
            .expect("no tools → allowed");
    }

    #[tokio::test]
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
        let result = adapter.messages_create("custom/x", None, msgs).await;
        assert!(result.is_err(), "image to a non-vision model must fail fast");
        assert!(matches!(
            result,
            Err(ApiError::Http(traits::HttpError::InvalidRequest(_)))
        ));
    }

    #[tokio::test]
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
        adapter.messages_create("anthropic/claude", None, msgs).await.expect("vision model accepts image");
    }
}
