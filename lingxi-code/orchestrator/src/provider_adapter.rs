//! Bridge: adapt any `providers::LlmProvider` to the orchestrator's
//! `OrchestratorApiClient` / `StreamingApiClient` traits. The model string
//! flows through unchanged; provider routing by prefix is wired in P2.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::BoxStream;
use protocol::ConversationMessage;
use providers::{CanonicalRequest, LlmProvider};
use std::sync::Arc;

/// Adapts an `Arc<dyn LlmProvider>` to the orchestrator's API-client traits.
pub struct ProviderApiAdapter {
    provider: Arc<dyn LlmProvider>,
}

impl ProviderApiAdapter {
    /// Wrap a provider.
    #[must_use]
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        let mut req = CanonicalRequest::new(model);
        req.system = system.map(str::to_string);
        req.messages = msgs;
        self.provider.complete(req).await
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
        let mut req = CanonicalRequest::new(model);
        req.system = system.map(str::to_string);
        req.messages = messages;
        req.tools = tools;
        req.stream = true;
        self.provider.stream(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, UsageApi};
    use futures::StreamExt;
    use providers::Capabilities;
    use std::sync::Mutex;

    /// Records the request it received and returns a canned response.
    struct StubProvider {
        seen_model: Mutex<Option<String>>,
        seen_system: Mutex<Option<String>>,
        caps: Capabilities,
    }

    impl StubProvider {
        fn new() -> Self {
            Self {
                seen_model: Mutex::new(None),
                seen_system: Mutex::new(None),
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
                content: vec![ContentBlockApi::Text { text: "ok".to_string() }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }
        async fn stream(
            &self,
            _req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    #[tokio::test]
    async fn bridge_forwards_model_and_system_to_provider() {
        let stub = Arc::new(StubProvider::new());
        let adapter = ProviderApiAdapter::new(stub.clone());
        let resp = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new())
            .await
            .expect("ok");
        assert_eq!(resp.model, "openai/gpt-4o");
        assert_eq!(stub.seen_model.lock().unwrap().as_deref(), Some("openai/gpt-4o"));
        assert_eq!(stub.seen_system.lock().unwrap().as_deref(), Some("sys"));
    }
}
