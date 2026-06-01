//! `OpenAI` / OpenAI-compatible chat-completions codec.

pub mod decode;
pub mod encode;
pub mod stream;

use crate::codec::{SseDecoder, WireCodec};
use crate::error::CodecError;
use crate::request::CanonicalRequest;
use api_client::types::MessageResponse;
use api_client::ApiError;
use protocol::{HttpMethod, HttpRequest};

use encode::OPENAI_DEFAULT_BASE;

/// `WireCodec` for `OpenAI` chat-completions. `base_url` is the API base (no
/// trailing slash); `None` uses [`OPENAI_DEFAULT_BASE`].
pub struct OpenAiCodec {
    base_url: String,
}

impl OpenAiCodec {
    /// Construct a codec for the given base URL (or the `OpenAI` default).
    #[must_use]
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| OPENAI_DEFAULT_BASE.to_string()),
        }
    }
}

impl WireCodec for OpenAiCodec {
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
        let body = encode::encode_chat_body(req);
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/chat/completions", self.base_url),
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }

    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError> {
        decode::decode_chat_response(status, body)
    }

    fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
        Box::new(stream::OpenAiSseDecoder::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_request_targets_chat_completions() {
        let codec = OpenAiCodec::new(None);
        let req = CanonicalRequest::new("gpt-4o");
        let http = codec.encode_request(&req).unwrap();
        assert_eq!(http.url, "https://api.openai.com/v1/chat/completions");
        assert!(http
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));
    }

    #[test]
    fn custom_base_url_is_used() {
        let codec = OpenAiCodec::new(Some("https://api.groq.com/openai/v1".to_string()));
        let http = codec
            .encode_request(&CanonicalRequest::new("llama"))
            .unwrap();
        assert_eq!(http.url, "https://api.groq.com/openai/v1/chat/completions");
    }
}
