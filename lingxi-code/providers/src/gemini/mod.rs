//! Google `Gemini` `generateContent` codec.

pub mod decode;
pub mod encode;
pub mod stream;

use crate::codec::{SseDecoder, WireCodec};
use crate::error::CodecError;
use crate::request::CanonicalRequest;
use api_client::types::MessageResponse;
use api_client::ApiError;
use protocol::{HttpMethod, HttpRequest};

use encode::GEMINI_DEFAULT_BASE;

/// `WireCodec` for the native `Gemini` API. `base_url` is the API base (no
/// trailing slash); `None` uses [`GEMINI_DEFAULT_BASE`]. The model goes in the
/// URL path, and streaming uses a different endpoint than non-streaming.
pub struct GeminiCodec {
    base_url: String,
    /// Profile-level thinking-token budget override (applied when the request
    /// itself does not specify one).
    thinking_budget: Option<u32>,
}

impl GeminiCodec {
    /// Construct a codec for the given base URL (or the `Gemini` default) and
    /// an optional profile-level thinking-token budget.
    #[must_use]
    pub fn new(base_url: Option<String>, thinking_budget: Option<u32>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| GEMINI_DEFAULT_BASE.to_string()),
            thinking_budget,
        }
    }
}

impl WireCodec for GeminiCodec {
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
        let budget = req.thinking_budget.or(self.thinking_budget);
        let body = encode::encode_generate_body(req, budget);
        let url = if req.stream {
            format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, req.model
            )
        } else {
            format!("{}/models/{}:generateContent", self.base_url, req.model)
        };
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url,
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }

    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError> {
        decode::decode_generate_response(status, body)
    }

    fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
        Box::new(stream::GeminiSseDecoder::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_stream_url_has_model_and_generate_content() {
        let codec = GeminiCodec::new(None, None);
        let req = CanonicalRequest::new("gemini-2.0-flash");
        let http = codec.encode_request(&req).unwrap();
        assert_eq!(
            http.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert!(http
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));
    }

    #[test]
    fn stream_url_uses_stream_endpoint() {
        let codec = GeminiCodec::new(None, None);
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.stream = true;
        let http = codec.encode_request(&req).unwrap();
        assert!(http
            .url
            .ends_with("/models/gemini-2.0-flash:streamGenerateContent?alt=sse"));
    }
}
