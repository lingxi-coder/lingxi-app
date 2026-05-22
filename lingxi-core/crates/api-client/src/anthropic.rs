//! Anthropic provider — builds API requests and maps SSE events to engine
//! events.
//!
//! This module only constructs request shapes and decodes individual SSE
//! payloads; all network I/O is delegated to the `HttpTransport` trait
//! (wired in Tasks 16–18).

use crate::types::StreamEvent;
use lingxi_protocol::{HttpMethod, HttpRequest};
use serde_json::Value;
use std::fmt;

/// Default Anthropic API base URL. Override via [`AnthropicProvider::new`].
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Value sent in the `anthropic-version` header on every request.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Provider that builds Anthropic Messages API requests and parses
/// streaming events.
///
/// The `api_key` is stored as a plain `String` for Plan 1; Plan 2 swaps in
/// `secrets::SecretBox<String>`. The custom [`fmt::Debug`] impl redacts the
/// key in all current diagnostic output.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl AnthropicProvider {
    /// Construct a new provider. Passing `None` for `base_url` uses
    /// [`DEFAULT_BASE_URL`].
    #[must_use]
    pub fn new(api_key: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        }
    }

    /// Build a non-streaming `POST /v1/messages` request. The caller owns
    /// the JSON body; this method only attaches headers and metadata.
    #[must_use]
    pub fn build_request(&self, body: &Value) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/v1/messages", self.base_url),
            headers: vec![
                ("x-api-key".into(), self.api_key.clone()),
                ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "application/json".into()),
            ],
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(120)),
        }
    }

    /// Build a streaming variant of [`Self::build_request`]: sets
    /// `stream: true` in the JSON body and swaps the `accept` header to
    /// `text/event-stream`.
    ///
    /// # Panics
    ///
    /// The two internal `expect` calls assume `body` round-trips through
    /// `serde_json` (it just came from `to_string`) and that the `accept`
    /// header set above is present. Both invariants hold by construction.
    #[must_use]
    pub fn build_streaming_request(&self, body: &Value) -> HttpRequest {
        let mut req = self.build_request(body);
        // Re-parse the body we just serialised so we can flip `stream: true`.
        // `unwrap` is safe: it was produced by `serde_json::Value::to_string`
        // a few lines above, which always emits valid JSON.
        let mut body_val: Value =
            serde_json::from_str(req.body.as_ref().expect("build_request always sets a body"))
                .expect("body was just serialised from a Value");
        body_val["stream"] = Value::Bool(true);
        req.body = Some(body_val.to_string());
        if let Some((_, v)) = req.headers.iter_mut().find(|(k, _)| k == "accept") {
            *v = "text/event-stream".to_string();
        }
        req
    }

    /// Parse a single SSE event payload into a [`StreamEvent`]. Used by the
    /// streaming consumer when iterating over events produced by
    /// `HttpTransport::stream_sse`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ApiError::MalformedStream`] when the payload is not
    /// valid JSON for the [`StreamEvent`] enum.
    pub fn parse_stream_event(data: &str) -> Result<StreamEvent, crate::ApiError> {
        serde_json::from_str::<StreamEvent>(data)
            .map_err(|e| crate::ApiError::MalformedStream(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::HttpMethod;

    #[test]
    fn build_request_includes_auth_and_version_headers() {
        let provider = AnthropicProvider::new("sk-ant-test", None);
        let body = serde_json::json!({"model": "claude-opus-4-6"});
        let req = provider.build_request(&body);
        let header_keys: Vec<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(header_keys.contains(&"x-api-key"));
        assert!(header_keys.contains(&"anthropic-version"));
        assert!(header_keys.contains(&"content-type"));
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn build_request_redacts_api_key_in_debug() {
        let provider = AnthropicProvider::new("sk-ant-secret", None);
        let s = format!("{provider:?}");
        assert!(!s.contains("sk-ant-secret"), "api key leaked: {s}");
    }
}
