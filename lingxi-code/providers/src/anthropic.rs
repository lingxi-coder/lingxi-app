//! Anthropic as an `LlmProvider`. Delegates verbatim to
//! `api_client::AnthropicProvider` — the same entry points the orchestrator's
//! Anthropic adapter calls — so the wire stays byte-identical and covered by
//! the existing parity fixtures. Generic over the concrete transport `T`;
//! erased to `Arc<dyn LlmProvider>` by the caller.

use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::{AnthropicProvider, ApiError};
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;
use std::sync::Arc;
use traits::HttpTransport;

/// Anthropic provider backed by `api_client::AnthropicProvider` + a transport.
pub struct AnthropicLlmProvider<T: HttpTransport + Send + Sync + 'static> {
    inner: AnthropicProvider,
    transport: Arc<T>,
    capabilities: Capabilities,
}

impl<T: HttpTransport + Send + Sync + 'static> AnthropicLlmProvider<T> {
    /// Construct from an API key, optional base URL, and a transport.
    #[must_use]
    pub fn new(api_key: impl Into<String>, base_url: Option<String>, transport: Arc<T>) -> Self {
        Self {
            inner: AnthropicProvider::new(api_key, base_url),
            transport,
            capabilities: Capabilities::anthropic(),
        }
    }
}

#[async_trait]
impl<T: HttpTransport + Send + Sync + 'static> LlmProvider for AnthropicLlmProvider<T> {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        // `messages_create_non_stream` has no `tools` parameter, so `req.tools`
        // is intentionally not forwarded here — tool use flows through `stream`.
        // (The orchestrator's non-streaming `messages_create` carries no tools.)
        self.inner
            .messages_create_non_stream(
                &req.model,
                req.system.as_deref(),
                req.messages,
                self.transport.as_ref(),
            )
            .await
    }

    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        self.inner
            .messages_create_stream(
                &req.model,
                req.system.as_deref(),
                req.messages,
                req.tools,
                self.transport.clone(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockTransport;
    use api_client::types::ContentBlockApi;
    use futures::StreamExt;
    use std::sync::Arc;

    const CANNED_200: &str = r#"{"id":"msg_x","model":"claude-opus-4-7","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#;

    fn provider(t: MockTransport) -> AnthropicLlmProvider<MockTransport> {
        AnthropicLlmProvider::new(
            "sk-test",
            Some("https://mock.local".to_string()),
            Arc::new(t),
        )
    }

    #[test]
    fn id_is_anthropic() {
        let p = provider(MockTransport::responding(200, ""));
        assert_eq!(p.id(), cost::ProviderId::Anthropic);
        assert!(p.capabilities().native_tools);
    }

    #[tokio::test]
    async fn complete_decodes_canned_anthropic_response() {
        let p = provider(MockTransport::responding(200, CANNED_200));
        let resp = p
            .complete(CanonicalRequest::new("claude-opus-4-7"))
            .await
            .expect("ok");
        assert_eq!(resp.id, "msg_x");
        match &resp.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hi"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_yields_decoded_events() {
        let frames = vec![
            r#"{"type":"message_start","message":{"id":"msg_x","model":"claude-opus-4-7","content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let p = provider(MockTransport::streaming(frames));
        let s = p
            .stream(CanonicalRequest::new("claude-opus-4-7"))
            .await
            .expect("stream");
        let events: Vec<_> = s.collect().await;
        assert!(matches!(
            events.first(),
            Some(Ok(StreamEvent::MessageStart { .. }))
        ));
        assert!(matches!(events.last(), Some(Ok(StreamEvent::MessageStop))));
    }
}
