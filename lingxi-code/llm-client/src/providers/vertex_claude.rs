//! Vertex AI Claude codec.
//!
//! Vertex AI wraps the Anthropic Messages API with a different URL layout and
//! GCP Bearer-token authentication.  The wire format is otherwise identical to
//! the standard Anthropic API.
//!
//! ## URL shape
//!
//! `base_url` in the profile is the **full Vertex AI prefix**:
//!
//! ```text
//! https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}
//! ```
//!
//! The codec appends the publisher/model path segment:
//!
//! ```text
//! Non-stream: {base_url}/publishers/anthropic/models/{model_id}:rawPredict
//! Stream:     {base_url}/publishers/anthropic/models/{model_id}:streamRawPredict
//! ```
//!
//! Streaming stays SSE (no binary event-stream framing change).
//!
//! ## Body changes
//!
//! - `model` key is **removed** from the request body (it is already encoded in
//!   the URL path).
//! - `anthropic_version: "vertex-2023-10-16"` is **inserted** into the body.
//! - The `anthropic-version` request header (injected by
//!   [`AnthropicMessagesCodec`]) is **removed** (Vertex does not accept it).
//!
//! ## Authentication
//!
//! `AuthStrategy::GcpToken` (Bearer token).  Use `CredentialConfig::Env { var:
//! "<ENV_VAR>" }` to supply the bearer token via environment variable — the
//! `EnvCredentialProvider` loads the variable as `Credential::ApiKey(value)`,
//! and the `GcpToken` authenticate arm accepts both `Credential::ApiKey` and
//! `Credential::BearerToken` as the bearer value (via `load_secret`).  No
//! separate config struct is needed.

use serde_json::Value;

use super::AnthropicMessagesCodec;
use crate::{
    LlmError, LlmRequest, LlmResponse, ProviderRequest, ProviderResponse, StreamDecoder, WireCodec,
};

/// The Anthropic API version inserted into Vertex AI request bodies.
const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

/// Vertex AI Claude codec.
///
/// Thin wrapper over [`AnthropicMessagesCodec`] that rewrites the URL, removes
/// the `model` body key, inserts `anthropic_version`, and removes the
/// `anthropic-version` header.  Streaming stays SSE.
#[derive(Debug, Clone)]
pub struct VertexClaudeCodec {
    base_url: String,
    /// Inner codec — used for body encoding and response decoding.
    inner: AnthropicMessagesCodec,
}

impl VertexClaudeCodec {
    /// Create a new Vertex Claude codec.
    ///
    /// - `base_url` – Full Vertex AI prefix, e.g.
    ///   `https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1`.
    ///   Do **not** include the `/publishers/…` segment; the codec constructs
    ///   that from the request model.
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        // The inner codec's `anthropic_version` value is irrelevant here because
        // `encode_request` replaces the body-level version and removes the header.
        let inner = AnthropicMessagesCodec::new(base_url.clone(), VERTEX_ANTHROPIC_VERSION);
        Self { base_url, inner }
    }

    /// Build the Vertex AI rawPredict URL for a given model id.
    fn predict_url(&self, model_id: &str, stream: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        let action = if stream {
            "streamRawPredict"
        } else {
            "rawPredict"
        };
        format!("{base}/publishers/anthropic/models/{model_id}:{action}")
    }
}

impl WireCodec for VertexClaudeCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        // Delegate to the inner Anthropic codec for body construction.
        let mut provider_request = self.inner.encode_request(request)?;

        // 1. Rewrite URL to the Vertex AI rawPredict endpoint.
        provider_request.url = self.predict_url(&request.model, request.stream);

        // 2. Remove `model` from body — it is already in the URL path.
        if let Some(body_obj) = provider_request.body_json.as_object_mut() {
            body_obj.remove("model");
            // 3. Insert `anthropic_version` (body-level, snake_case, Vertex-specific).
            body_obj.insert(
                "anthropic_version".to_string(),
                Value::String(VERTEX_ANTHROPIC_VERSION.to_string()),
            );
        }

        // 4. Remove the `anthropic-version` header that the inner codec injects.
        //    Vertex takes `anthropic_version` in-body; the header is not accepted.
        provider_request.headers.remove("anthropic-version");

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        // Vertex non-streaming responses use the same JSON shape as Anthropic.
        self.inner.decode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        // Vertex streaming is standard SSE; delegate to the inner Anthropic decoder.
        self.inner.stream_decoder()
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

// ── Stream decoder pass-through ───────────────────────────────────────────────
//
// VertexClaude streaming is plain SSE — the Anthropic stream decoder handles
// it directly via `self.inner.stream_decoder()` above.  No extra wrapper needed.

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str =
        "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1";

    // ── Codec encode shape ─────────────────────────────────────────────────────

    /// Non-streaming encode: URL must end with `:rawPredict`, body has
    /// `anthropic_version`, `model` is absent, `anthropic-version` header gone.
    #[test]
    fn encode_non_streaming_shape() {
        let codec = VertexClaudeCodec::new(BASE);
        let req = LlmRequest::new("claude-sonnet-4@20250514").with_user_text("hello");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        // URL check: must end with :rawPredict.
        assert_eq!(
            provider_req.url,
            format!("{BASE}/publishers/anthropic/models/claude-sonnet-4@20250514:rawPredict"),
            "non-streaming URL must end with :rawPredict"
        );

        // model key must be absent.
        assert!(
            provider_req.body_json.get("model").is_none(),
            "model must be removed from body; got: {}",
            provider_req.body_json
        );

        // anthropic_version must be present (body-level, snake_case, Vertex value).
        assert_eq!(
            provider_req
                .body_json
                .get("anthropic_version")
                .and_then(Value::as_str),
            Some("vertex-2023-10-16"),
            "anthropic_version must be \"vertex-2023-10-16\" in body"
        );

        // anthropic-version header must be absent.
        assert!(
            !provider_req.headers.contains_key("anthropic-version"),
            "anthropic-version header must be removed"
        );
    }

    /// Streaming encode: URL must end with `:streamRawPredict`.
    #[test]
    fn encode_streaming_url() {
        let codec = VertexClaudeCodec::new(BASE);
        let mut req = LlmRequest::new("claude-sonnet-4@20250514");
        req.stream = true;

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.url,
            format!("{BASE}/publishers/anthropic/models/claude-sonnet-4@20250514:streamRawPredict"),
            "streaming URL must end with :streamRawPredict"
        );

        // anthropic_version still in body.
        assert_eq!(
            provider_req
                .body_json
                .get("anthropic_version")
                .and_then(Value::as_str),
            Some("vertex-2023-10-16")
        );

        // anthropic-version header still absent.
        assert!(
            !provider_req.headers.contains_key("anthropic-version"),
            "anthropic-version header must be removed even for streaming"
        );

        // model still absent.
        assert!(provider_req.body_json.get("model").is_none());
    }

    /// Streaming framing is SSE (default) — no `AwsEventStream` for Vertex Claude.
    #[test]
    fn streaming_framing_stays_sse() {
        let codec = VertexClaudeCodec::new(BASE);
        let mut req = LlmRequest::new("claude-sonnet-4@20250514");
        req.stream = true;

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.stream_framing,
            crate::StreamFraming::Sse,
            "VertexClaude streaming must stay SSE (not AwsEventStream)"
        );
    }
}
