//! Azure `OpenAI` Chat Completions codec.
//!
//! Azure `OpenAI` exposes the same wire format as `OpenAI` Chat Completions but
//! with a different URL layout and authentication header.
//!
//! ## URL shape
//!
//! ```text
//! {base_url}/openai/deployments/{deployment}/chat/completions?api-version={api_version}
//! ```
//!
//! where `{deployment}` is the **`request_model`** value from the
//! [`ModelProfile`] (i.e. the model string that arrives in [`LlmRequest`]).
//!
//! Unlike standard `OpenAI`, the `model` key is **omitted** from the JSON
//! request body because the deployment is already encoded in the URL.
//!
//! Per Azure documentation:
//! <https://learn.microsoft.com/en-us/azure/ai-services/openai/reference#chat-completions>
//!
//! ## Authentication
//!
//! Azure uses `api-key: <key>` rather than `Authorization: Bearer ...`.
//! The authenticator in `client.rs` injects this header; the codec is
//! authentication-agnostic.
//!
//! ## Streaming
//!
//! Identical SSE shape to `OpenAI` Chat Completions; the `OpenAiChatCodec`
//! stream decoder is reused directly.

use crate::{
    LlmError, LlmRequest, ProviderRequest, ProviderResponse, LlmResponse, StreamDecoder, WireCodec,
};
use super::OpenAiChatCodec;

/// Azure `OpenAI` Chat Completions codec.
///
/// Thin wrapper over [`OpenAiChatCodec`] that rewrites the URL and omits the
/// `model` key from the request body (deployment is in the URL).
#[derive(Debug, Clone)]
pub struct AzureOpenAiCodec {
    base_url: String,
    api_version: String,
    /// Inner codec — used for body encoding logic and decoding.
    inner: OpenAiChatCodec,
}

impl AzureOpenAiCodec {
    /// Create a new Azure `OpenAI` codec.
    ///
    /// - `base_url` – Azure resource endpoint, e.g.
    ///   `https://<resource>.openai.azure.com`.  Do **not** include the
    ///   `/openai/deployments/…` segment; the codec constructs that from the
    ///   request model.
    /// - `api_version` – Azure API version string, e.g. `"2024-02-01"`.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_version: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let api_version = api_version.into();
        // The inner codec is constructed with the base_url only for structural
        // reasons; we never call its encode_request (which would inject a wrong URL).
        let inner = OpenAiChatCodec::new(base_url.clone());
        Self { base_url, api_version, inner }
    }

    /// Build the Azure deployment URL for a given model (deployment name).
    fn deployment_url(&self, deployment: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!(
            "{base}/openai/deployments/{deployment}/chat/completions?api-version={}",
            self.api_version
        )
    }
}

impl WireCodec for AzureOpenAiCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        // Delegate to the OpenAI codec to build the body, then:
        // 1. Replace the URL with the Azure deployment URL.
        // 2. Remove the `model` key from the body (it's in the URL).
        let mut provider_request = self.inner.encode_request(request)?;

        // Override URL: use the request.model as the deployment name.
        provider_request.url = self.deployment_url(&request.model);

        // Remove `model` from the body — Azure deployment is in the URL.
        if let Some(body_obj) = provider_request.body_json.as_object_mut() {
            body_obj.remove("model");
        }

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        // Azure returns the same JSON shape as OpenAI.
        self.inner.decode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        // Azure SSE is identical to OpenAI SSE.
        self.inner.stream_decoder()
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}
