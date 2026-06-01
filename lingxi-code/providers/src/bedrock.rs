//! Claude-on-Bedrock provider: the Anthropic Messages body over Bedrock's
//! `InvokeModel` endpoint + `SigV4` auth. Non-streaming (`/invoke`); `stream()`
//! re-emits the completed response as a synthetic single-shot event stream
//! (the frozen `HttpTransport` exposes no raw byte stream for AWS event-stream
//! framing, so real Bedrock streaming is deferred).

use crate::authenticator::Authenticator;
use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::{self, BoxStream, StreamExt};
use protocol::{HttpMethod, HttpRequest};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;
use traits::HttpTransport;

/// A Claude-on-Bedrock provider (non-streaming `InvokeModel` + synthetic stream).
pub struct BedrockProvider {
    region: String,
    transport: Arc<dyn HttpTransport>,
    authenticator: Arc<dyn Authenticator>,
    capabilities: Capabilities,
}

impl BedrockProvider {
    /// Construct for an AWS region with a (`SigV4`) authenticator + transport.
    #[must_use]
    pub fn new(region: String, transport: Arc<dyn HttpTransport>, authenticator: Arc<dyn Authenticator>) -> Self {
        Self { region, transport, authenticator, capabilities: Capabilities::anthropic() }
    }

    /// Build the Anthropic-on-Bedrock body (no top-level `model`; carries `anthropic_version`).
    fn build_body(req: &CanonicalRequest) -> Value {
        let mut body = Map::new();
        body.insert("anthropic_version".to_string(), json!("bedrock-2023-05-31"));
        body.insert("max_tokens".to_string(), json!(req.max_tokens));
        body.insert("messages".to_string(), serde_json::to_value(&req.messages).unwrap_or(Value::Array(vec![])));
        if let Some(s) = &req.system {
            body.insert("system".to_string(), json!(s));
        }
        if !req.tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(req.tools.clone()));
        }
        Value::Object(body)
    }

    fn invoke_url(&self, model: &str) -> String {
        format!("https://bedrock-runtime.{}.amazonaws.com/model/{model}/invoke", self.region)
    }
}

/// Re-emit a completed `MessageResponse` as a valid single-shot event sequence.
fn synthesize_stream(resp: &MessageResponse) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    out.push(StreamEvent::MessageStart {
        message: MessageResponse {
            id: resp.id.clone(),
            model: resp.model.clone(),
            content: Vec::new(),
            stop_reason: None,
            usage: resp.usage,
        },
    });
    for (i, block) in resp.content.iter().enumerate() {
        let index = u32::try_from(i).unwrap_or(0);
        out.push(StreamEvent::ContentBlockStart { index, content_block: block.clone() });
        match block {
            ContentBlockApi::Text { text } => out.push(StreamEvent::ContentBlockDelta {
                index, delta: ContentDelta::TextDelta { text: text.clone() },
            }),
            ContentBlockApi::ToolUse { input, .. } => out.push(StreamEvent::ContentBlockDelta {
                index, delta: ContentDelta::InputJsonDelta { partial_json: input.to_string() },
            }),
            _ => {}
        }
        out.push(StreamEvent::ContentBlockStop { index });
    }
    out.push(StreamEvent::MessageDelta {
        delta: MessageDeltaPayload { stop_reason: resp.stop_reason.clone() },
        usage: Some(resp.usage),
    });
    out.push(StreamEvent::MessageStop);
    out
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> ProviderId { ProviderId::AmazonBedrock }
    fn capabilities(&self) -> &Capabilities { &self.capabilities }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let body = Self::build_body(&req);
        let mut http = HttpRequest {
            method: HttpMethod::Post,
            url: self.invoke_url(&req.model),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some(body.to_string()),
            timeout: Some(Duration::from_secs(600)),
        };
        self.authenticator.authorize(&mut http).await?;
        let resp = self.transport.request(http).await.map_err(ApiError::Http)?;
        if !(200..300).contains(&resp.status) {
            return Err(ApiError::Server { status: resp.status, body: resp.body });
        }
        serde_json::from_str::<MessageResponse>(&resp.body)
            .map_err(|e| ApiError::MalformedStream(format!("bedrock response decode: {e}")))
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let resp = self.complete(req).await?;
        Ok(stream::iter(synthesize_stream(&resp).into_iter().map(Ok)).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::UsageApi;

    #[test]
    fn build_body_has_anthropic_version_no_model() {
        let body = BedrockProvider::build_body(&CanonicalRequest::new("anthropic.claude-3-5-sonnet-20241022-v2:0"));
        assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
        assert!(body.get("max_tokens").is_some());
        assert!(body.get("messages").is_some());
        assert!(body.get("model").is_none());
    }

    #[test]
    fn synthesize_stream_emits_valid_sequence() {
        let resp = MessageResponse {
            id: "m".to_string(),
            model: "anthropic.claude-3".to_string(),
            content: vec![ContentBlockApi::Text { text: "hi".to_string() }],
            stop_reason: Some("end_turn".to_string()),
            usage: UsageApi::default(),
        };
        let ev = synthesize_stream(&resp);
        assert!(matches!(ev.first(), Some(StreamEvent::MessageStart { .. })));
        assert!(ev.iter().any(|e| matches!(e, StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hi")));
        assert_eq!(ev.iter().filter(|e| matches!(e, StreamEvent::MessageDelta { .. })).count(), 1);
        assert!(matches!(ev.last(), Some(StreamEvent::MessageStop)));
    }
}
