//! Azure AI Foundry Claude codec.
//!
//! Azure AI Foundry hosts the Anthropic Messages API under an Azure resource
//! endpoint. The wire format is **byte-identical to the first-party Anthropic
//! Messages API** — same `POST {base}/v1/messages` shape, same
//! `anthropic-version: 2023-06-01` request header, same request/response body.
//! Only two things differ from first-party Anthropic:
//!
//! 1. **Base URL** — the profile's `base_url` is the Foundry Anthropic-messages
//!    root, e.g. `https://{resource}.services.ai.azure.com/anthropic/`
//!    (see [`foundry_messages_base_url`](crate::foundry_messages_base_url)); the
//!    codec appends `v1/messages`, matching the Foundry SDK's
//!    `https://{resource}.services.ai.azure.com/anthropic/v1/messages`.
//! 2. **Auth header** — Foundry selects the auth header by credential kind
//!    (CC 2.1.207 `AnthropicFoundry.authHeaders()`):
//!    - a plain API key (`ANTHROPIC_FOUNDRY_API_KEY`, a *string*) → `x-api-key`
//!      (same header first-party Anthropic uses), and
//!    - an AAD token (`ANTHROPIC_FOUNDRY_AUTH_TOKEN`, supplied via the SDK's
//!      `azureADTokenProvider` *function*) → `Authorization: Bearer <token>`.
//!    This is applied at the [`AuthStrategy`](crate::AuthStrategy) layer, not by
//!    the codec: [`AuthStrategy::ApiKey`] + [`ProtocolFamily::FoundryClaude`]
//!    emits `x-api-key`; [`AuthStrategy::Bearer`] emits `Authorization: Bearer`.
//!
//! Because the wire is identical, this codec is a thin identity wrapper over
//! [`AnthropicMessagesCodec`] — it exists so Foundry has its own
//! [`ProtocolFamily`](crate::ProtocolFamily)/[`ProviderId`](crate::ProviderId)
//! identity for routing, auth-header selection, and pricing, mirroring how
//! [`VertexClaudeCodec`](crate::VertexClaudeCodec) wraps the same inner codec.
//!
//! [`AuthStrategy::ApiKey`]: crate::AuthStrategy::ApiKey
//! [`AuthStrategy::Bearer`]: crate::AuthStrategy::Bearer
//! [`ProtocolFamily::FoundryClaude`]: crate::ProtocolFamily::FoundryClaude

use super::AnthropicMessagesCodec;
use crate::{
    LlmError, LlmRequest, LlmResponse, ProviderRequest, ProviderResponse, StreamDecoder, WireCodec,
};

/// The Anthropic API version sent on Foundry requests — the standard Anthropic
/// SDK default (`2023-06-01`), identical to the first-party header. Foundry is
/// the vanilla Anthropic client pointed at an Azure resource, so it carries the
/// same `anthropic-version` header as first-party (unlike Vertex/Bedrock, which
/// use an in-body `anthropic_version`).
const FOUNDRY_ANTHROPIC_VERSION: &str = "2023-06-01";

/// Azure AI Foundry Claude codec.
///
/// Identity wrapper over [`AnthropicMessagesCodec`]: Foundry's wire format is
/// byte-identical to first-party Anthropic Messages, so encode/decode/stream all
/// delegate unchanged. The Foundry-specific base URL is carried by the profile
/// and the Foundry auth header is applied by the auth layer.
#[derive(Debug, Clone)]
pub struct FoundryClaudeCodec {
    /// Inner codec — Foundry uses the standard Anthropic Messages wire verbatim.
    inner: AnthropicMessagesCodec,
}

impl FoundryClaudeCodec {
    /// Create a new Foundry Claude codec.
    ///
    /// - `base_url` – the Foundry Anthropic-messages root, e.g.
    ///   `https://{resource}.services.ai.azure.com/anthropic/`. The codec
    ///   appends `/v1/messages` (via the inner Anthropic codec).
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            inner: AnthropicMessagesCodec::new(base_url.into(), FOUNDRY_ANTHROPIC_VERSION),
        }
    }
}

impl WireCodec for FoundryClaudeCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        // Foundry wire == Anthropic Messages wire: no URL/body rewrite.
        self.inner.encode_request(request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        self.inner.decode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        self.inner.stream_decoder()
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    // Foundry base URL derived from ANTHROPIC_FOUNDRY_RESOURCE — the
    // `foundry_messages_base_url` shape (trailing slash retained).
    const BASE: &str = "https://my-res.services.ai.azure.com/anthropic/";

    /// The messages URL is `{base}/v1/messages`, matching the CC 2.1.207 Foundry
    /// SDK endpoint `https://{resource}.services.ai.azure.com/anthropic/v1/messages`
    /// (the inner codec normalizes the trailing slash).
    #[test]
    fn encode_targets_foundry_v1_messages() {
        let codec = FoundryClaudeCodec::new(BASE);
        let req = LlmRequest::new("claude-sonnet-4-5").with_user_text("hi");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.url, "https://my-res.services.ai.azure.com/anthropic/v1/messages",
            "Foundry messages URL must be {{base}}/v1/messages"
        );
    }

    /// Foundry carries the standard `anthropic-version` HEADER (unlike Vertex,
    /// which moves it in-body). `model` stays in the body (no Vertex-style strip).
    #[test]
    fn encode_keeps_anthropic_version_header_and_model_body() {
        let codec = FoundryClaudeCodec::new(BASE);
        let req = LlmRequest::new("claude-opus-4-6").with_user_text("hi");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req
                .headers
                .get("anthropic-version")
                .map(String::as_str),
            Some("2023-06-01"),
            "Foundry sends the standard anthropic-version header"
        );
        assert_eq!(
            provider_req.body_json.get("model").and_then(Value::as_str),
            Some("claude-opus-4-6"),
            "Foundry keeps model in the body (no Vertex-style strip)"
        );
        // No in-body anthropic_version (that is Vertex/Bedrock-only).
        assert!(
            provider_req.body_json.get("anthropic_version").is_none(),
            "Foundry must NOT carry an in-body anthropic_version"
        );
    }

    /// Streaming stays plain SSE and preserves the header/URL shape.
    #[test]
    fn streaming_stays_sse() {
        let codec = FoundryClaudeCodec::new(BASE);
        let mut req = LlmRequest::new("claude-sonnet-4-5");
        req.stream = true;

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.stream_framing,
            crate::StreamFraming::Sse,
            "Foundry streaming must stay SSE"
        );
        assert_eq!(
            provider_req.url,
            "https://my-res.services.ai.azure.com/anthropic/v1/messages"
        );
    }
}
