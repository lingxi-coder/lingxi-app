//! Vertex AI Gemini codec.
//!
//! Vertex AI Gemini exposes the same generateContent wire format as the public
//! Gemini API but with a different URL layout and GCP Bearer-token
//! authentication rather than an API key header.
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
//! Non-stream: {base_url}/publishers/google/models/{model_id}:generateContent
//! Stream:     {base_url}/publishers/google/models/{model_id}:streamGenerateContent?alt=sse
//! ```
//!
//! ## Body / wire format
//!
//! The request and response body format is **identical** to the public Gemini
//! API.  This codec delegates all body encoding and decoding to [`GeminiCodec`]
//! and only overrides the URL.
//!
//! ## Authentication
//!
//! `AuthStrategy::GcpToken` (Bearer token).  Use `CredentialConfig::Env { var:
//! "<ENV_VAR>" }` to supply the bearer token via environment variable — the
//! `EnvCredentialProvider` loads the variable as `Credential::ApiKey(value)`,
//! and the `GcpToken` authenticate arm accepts both `Credential::ApiKey` and
//! `Credential::BearerToken` as the bearer value (via `load_secret`).  No
//! separate config struct is needed.
//!
//! ## Auth-header note (VERIFIED)
//!
//! `GeminiCodec::encode_request` does **not** inject any `x-goog-api-key` or
//! other auth header — only `content-type` is set at encode time.  Auth headers
//! are applied later by the `authenticate` step in `client.rs`.  No suppression
//! is needed in this wrapper.

use crate::{
    LlmError, LlmRequest, LlmResponse, ProviderRequest, ProviderResponse,
    StreamDecoder, WireCodec,
};
use super::GeminiCodec;

/// Vertex AI Gemini codec.
///
/// Thin wrapper over [`GeminiCodec`] that rewrites the URL to the Vertex AI
/// publisher path.  All body encoding and decoding is delegated to the inner
/// codec unchanged.
#[derive(Debug, Clone)]
pub struct VertexGeminiCodec {
    base_url: String,
    /// Inner codec — body encoding and decoding are fully delegated.
    inner: GeminiCodec,
}

impl VertexGeminiCodec {
    /// Create a new Vertex Gemini codec.
    ///
    /// - `base_url` – Full Vertex AI prefix, e.g.
    ///   `https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1`.
    ///   Do **not** include the `/publishers/…` segment; the codec constructs
    ///   that from the request model.
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        // GeminiCodec needs a base_url for URL construction, but VertexGeminiCodec
        // overrides the URL after delegation, so the inner base_url is irrelevant.
        // We pass the same base_url for consistency; the URL is replaced anyway.
        let inner = GeminiCodec::new(base_url.clone());
        Self { base_url, inner }
    }

    /// Build the Vertex AI generateContent URL for a given model id.
    fn generate_content_url(&self, model_id: &str, stream: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        if stream {
            format!("{base}/publishers/google/models/{model_id}:streamGenerateContent?alt=sse")
        } else {
            format!("{base}/publishers/google/models/{model_id}:generateContent")
        }
    }
}

impl WireCodec for VertexGeminiCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        // Delegate to the inner Gemini codec for body construction.
        let mut provider_request = self.inner.encode_request(request)?;

        // Override URL to the Vertex AI publisher path.
        // The inner codec builds `{inner_base}/models/{model}:generateContent[?alt=sse]`;
        // we replace it entirely with the Vertex AI path.
        provider_request.url = self.generate_content_url(&request.model, request.stream);

        // NOTE: GeminiCodec does NOT inject any x-goog-api-key or other auth
        // header at encode time (verified: only content-type is set). No header
        // suppression is needed here — auth headers are applied by the client's
        // authenticate step (GcpToken → Bearer, not api-key).

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        // Vertex Gemini responses use the same JSON shape as public Gemini.
        self.inner.decode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        // Vertex Gemini SSE is identical to public Gemini SSE.
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

    const BASE: &str = "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1";

    // ── Codec encode shape ─────────────────────────────────────────────────────

    /// Non-streaming encode: URL must end with `:generateContent`; body must be
    /// identical to what `GeminiCodec` produces (contents/generationConfig shape).
    #[test]
    fn encode_non_streaming_url() {
        let codec = VertexGeminiCodec::new(BASE);
        let req = LlmRequest::new("gemini-2.0-flash").with_user_text("hello");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.url,
            format!("{BASE}/publishers/google/models/gemini-2.0-flash:generateContent"),
            "non-streaming URL must end with :generateContent"
        );
    }

    /// Streaming encode: URL must end with `:streamGenerateContent?alt=sse`.
    #[test]
    fn encode_streaming_url() {
        let codec = VertexGeminiCodec::new(BASE);
        let mut req = LlmRequest::new("gemini-2.0-flash");
        req.stream = true;

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert_eq!(
            provider_req.url,
            format!("{BASE}/publishers/google/models/gemini-2.0-flash:streamGenerateContent?alt=sse"),
            "streaming URL must end with :streamGenerateContent?alt=sse"
        );
    }

    /// The body is delegated entirely to `GeminiCodec` — `contents` array must be
    /// present; no `model` key (Gemini never adds one).
    #[test]
    fn body_is_gemini_format() {
        let codec = VertexGeminiCodec::new(BASE);
        let req = LlmRequest::new("gemini-2.0-flash").with_user_text("hello vertex");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        // contents array must exist (Gemini body format).
        assert!(
            provider_req.body_json.get("contents").is_some(),
            "body must have 'contents' (Gemini format); got: {}",
            provider_req.body_json
        );

        // model key must NOT be present (Gemini codec never adds it).
        assert!(
            provider_req.body_json.get("model").is_none(),
            "Gemini body must not have 'model' key"
        );
    }

    /// Verify that no `x-goog-api-key` or other auth header is injected at
    /// encode time — auth is applied separately by the `GcpToken` authenticate arm.
    #[test]
    fn no_auth_header_injected_at_encode_time() {
        let codec = VertexGeminiCodec::new(BASE);
        let req = LlmRequest::new("gemini-2.0-flash").with_user_text("hello");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        assert!(
            !provider_req.headers.contains_key("x-goog-api-key"),
            "x-goog-api-key must NOT be injected at encode time for Vertex"
        );
        assert!(
            !provider_req.headers.contains_key("authorization"),
            "authorization must NOT be injected at encode time"
        );
    }

    /// The body content for Vertex Gemini is identical to what `GeminiCodec`
    /// produces for the same request (only the URL differs).
    #[test]
    fn body_matches_gemini_codec_output() {
        use crate::providers::GeminiCodec;

        let vertex_codec = VertexGeminiCodec::new(BASE);
        let gemini_codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");

        let req = LlmRequest::new("gemini-2.0-flash").with_user_text("test body parity");

        let vertex_req = vertex_codec.encode_request(&req).expect("vertex encode");
        let gemini_req = gemini_codec.encode_request(&req).expect("gemini encode");

        // Bodies must be identical.
        assert_eq!(
            vertex_req.body_json, gemini_req.body_json,
            "VertexGemini body must be identical to GeminiCodec body for same request"
        );
    }
}
