//! Amazon Bedrock Claude codec.
//!
//! Bedrock wraps the Anthropic Messages API with:
//! - A different URL layout: `{base_url}/model/{model_id}/invoke` (or
//!   `/invoke-with-response-stream` for streaming).
//! - The `model` key is **removed** from the request body (it is already
//!   encoded in the URL path).
//! - `anthropic_version: "bedrock-2023-05-31"` is **inserted** into the body.
//! - The `anthropic-version` request header (injected by
//!   [`AnthropicMessagesCodec`]) is **removed** (Bedrock does not accept it).
//! - Streaming uses AWS binary event-stream framing, not SSE.
//!
//! ## URL shape
//!
//! ```text
//! Non-stream: {base_url}/model/{model_id}/invoke
//! Stream:     {base_url}/model/{model_id}/invoke-with-response-stream
//! ```
//!
//! `model_id` may contain `:` (e.g.
//! `anthropic.claude-3-5-sonnet-20241022-v2:0`); Bedrock accepts the raw `:`
//! in the URL path and the `SigV4` canonicalization handles the percent-encoding
//! correctly.
//!
//! ## Stream framing
//!
//! Streaming responses arrive as AWS event-stream binary frames (see
//! [`crate::eventstream`]).  Each frame whose `:message-type` header equals
//! `"event"` carries a payload of the form `{"bytes":"<base64>"}`, where the
//! base64-decoded bytes are a single Anthropic streaming-event JSON object.
//! Frames with `:message-type` of `"exception"` or `"error"` are mapped to
//! [`LlmError`].
//!
//! ## Authentication
//!
//! `SigV4` (AWS Signature Version 4) is applied by the client for the `bedrock`
//! service.  Credentials are expected as [`crate::Credential::AwsSigV4`],
//! typically loaded from a
//! [`crate::StaticCredentialProvider`] (since the three-field `SigV4` credential
//! cannot be expressed by the single-env-variable [`crate::EnvCredentialProvider`]).

use base64::Engine;
use serde_json::Value;

use crate::{
    eventstream::EventStreamSplitter, LlmError, LlmEvent, LlmRequest, ProviderRequest,
    ProviderResponse, LlmResponse, RawStreamFrame, StreamDecoder, StreamFraming, WireCodec,
};
use super::AnthropicMessagesCodec;

/// The Anthropic API version inserted into Bedrock request bodies.
const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";

/// Amazon Bedrock Claude codec.
///
/// Thin wrapper over [`AnthropicMessagesCodec`] that rewrites the URL, removes
/// the `model` body key, inserts `anthropic_version`, and switches streaming to
/// AWS binary event-stream framing.
#[derive(Debug, Clone)]
pub struct BedrockClaudeCodec {
    base_url: String,
    /// Inner codec — used for body encoding and non-streaming decode.
    inner: AnthropicMessagesCodec,
}

impl BedrockClaudeCodec {
    /// Create a new Bedrock Claude codec.
    ///
    /// - `base_url` – Bedrock runtime endpoint, e.g.
    ///   `https://bedrock-runtime.us-east-1.amazonaws.com`. Do **not** include
    ///   the `/model/…` segment; the codec constructs that from the request
    ///   model.
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        // The inner codec's `anthropic_version` value is irrelevant here because
        // `encode_request` removes the `anthropic-version` header that the inner
        // codec inserts.  We still need to construct one for body encoding.
        let inner = AnthropicMessagesCodec::new(base_url.clone(), BEDROCK_ANTHROPIC_VERSION);
        Self { base_url, inner }
    }

    /// Build the Bedrock invoke URL for a given model id.
    fn invoke_url(&self, model_id: &str, stream: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        let suffix = if stream { "invoke-with-response-stream" } else { "invoke" };
        format!("{base}/model/{model_id}/{suffix}")
    }
}

impl WireCodec for BedrockClaudeCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        // Delegate to the inner Anthropic codec for body construction.
        let mut provider_request = self.inner.encode_request(request)?;

        // 1. Rewrite URL to the Bedrock invoke endpoint.
        provider_request.url = self.invoke_url(&request.model, request.stream);

        // 2. Remove `model` from body — it is already in the URL path.
        if let Some(body_obj) = provider_request.body_json.as_object_mut() {
            body_obj.remove("model");
            // 3. Insert `anthropic_version` (body-level field, snake_case, Bedrock-specific).
            //    The inner codec already omits this field; only the header is set.
            body_obj.insert(
                "anthropic_version".to_string(),
                Value::String(BEDROCK_ANTHROPIC_VERSION.to_string()),
            );
        }

        // 4. Remove the `anthropic-version` header that the inner codec injects.
        //    Bedrock takes `anthropic_version` in-body; the header is not accepted.
        provider_request.headers.remove("anthropic-version");

        // 5. Switch to AWS event-stream framing for streaming requests.
        if request.stream {
            provider_request.stream_framing = StreamFraming::AwsEventStream;
        }

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        // Bedrock non-streaming responses use the same JSON shape as Anthropic.
        self.inner.decode_response(response)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(BedrockClaudeStreamDecoder {
            splitter: EventStreamSplitter::default(),
            inner: crate::providers::bedrock_claude_inner_decoder(),
        })
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

// ── Stream decoder ─────────────────────────────────────────────────────────────

/// Per-stream decoder for Bedrock Claude streaming responses.
///
/// Feeds raw byte chunks through [`EventStreamSplitter`] to extract binary
/// event-stream frames, then unwraps each event frame's base64-encoded payload
/// and delegates to an inner Anthropic streaming decoder.
#[derive(Debug)]
pub struct BedrockClaudeStreamDecoder {
    splitter: EventStreamSplitter,
    inner: Box<dyn StreamDecoder>,
}

impl StreamDecoder for BedrockClaudeStreamDecoder {
    /// Decode one raw byte chunk from the Bedrock event-stream.
    ///
    /// The chunk is fed to the [`EventStreamSplitter`]; for each complete
    /// event-stream frame:
    ///
    /// - `:message-type` == `"event"`: payload is `{"bytes":"<base64>"}`;
    ///   the base64-decoded bytes are a single Anthropic streaming-event JSON
    ///   and are forwarded to the inner decoder as a [`RawStreamFrame`].
    /// - `:message-type` == `"exception"` or `"error"`: returns an error
    ///   naming the `:exception-type` header and/or the payload message.
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        let messages = self.splitter.feed(&frame.bytes)?;
        let mut events = Vec::new();

        for msg in messages {
            let message_type = msg
                .headers
                .iter()
                .find(|(n, _)| n == ":message-type")
                .map(|(_, v)| v.as_str());

            match message_type {
                Some("event") => {
                    // Payload is JSON `{"bytes": "<base64>"}`.
                    let payload_str =
                        std::str::from_utf8(&msg.payload).map_err(|_| LlmError::StreamInterrupted {
                            message: "Bedrock event-stream payload is not valid UTF-8".to_string(),
                        })?;
                    let payload_json: Value =
                        serde_json::from_str(payload_str).map_err(|e| LlmError::StreamInterrupted {
                            message: format!(
                                "Bedrock event-stream payload is not valid JSON: {e}"
                            ),
                        })?;
                    let b64 = payload_json
                        .get("bytes")
                        .and_then(Value::as_str)
                        .ok_or_else(|| LlmError::StreamInterrupted {
                            message: "Bedrock event-stream event payload missing \"bytes\" field"
                                .to_string(),
                        })?;
                    let anthropic_json_bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| LlmError::StreamInterrupted {
                            message: format!(
                                "Bedrock event-stream \"bytes\" field is not valid base64: {e}"
                            ),
                        })?;
                    // Forward the unwrapped Anthropic event JSON as a raw frame
                    // to the inner Anthropic decoder.
                    let inner_frame = RawStreamFrame::new(anthropic_json_bytes);
                    let mut frame_events = self.inner.decode_frame(inner_frame)?;
                    events.append(&mut frame_events);
                }
                Some("exception" | "error") => {
                    let exception_type = msg
                        .headers
                        .iter()
                        .find(|(n, _)| n == ":exception-type")
                        .map_or("unknown", |(_, v)| v.as_str());
                    let payload_str = std::str::from_utf8(&msg.payload)
                        .unwrap_or("")
                        .to_string();
                    return Err(LlmError::StreamInterrupted {
                        message: format!(
                            "Bedrock event-stream {}: {}: {}",
                            message_type.unwrap_or("error"),
                            exception_type,
                            payload_str
                        ),
                    });
                }
                // Unknown message type or missing header — tolerate per streaming contract.
                Some(_) | None => {}
            }
        }

        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        // Check the splitter has no partial frames outstanding.
        self.splitter.finish()?;
        // Flush the inner decoder.
        self.inner.finish()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventstream::{build_frame, encode_string_header};

    // ── Codec encode shape ─────────────────────────────────────────────────────

    /// Non-streaming encode: URL must be `.../invoke`, body has
    /// `anthropic_version`, `model` is absent, `anthropic-version` header gone.
    #[test]
    fn encode_non_streaming_shape() {
        let codec = BedrockClaudeCodec::new("https://bedrock-runtime.us-east-1.amazonaws.com");
        let req = LlmRequest::new("anthropic.claude-3-5-sonnet-20241022-v2:0")
            .with_user_text("hello");

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        // URL check: .../invoke (no -with-response-stream).
        assert_eq!(
            provider_req.url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-3-5-sonnet-20241022-v2:0/invoke",
            "non-streaming URL must end with /invoke"
        );

        // Model key must be absent.
        assert!(
            provider_req.body_json.get("model").is_none(),
            "model must be removed from body; got: {}",
            provider_req.body_json
        );

        // anthropic_version must be present (body-level, snake_case).
        assert_eq!(
            provider_req.body_json.get("anthropic_version").and_then(Value::as_str),
            Some("bedrock-2023-05-31"),
            "anthropic_version must be in body"
        );

        // anthropic-version header must be absent.
        assert!(
            !provider_req.headers.contains_key("anthropic-version"),
            "anthropic-version header must be removed"
        );

        // Stream framing must be SSE (default) for non-streaming.
        assert_eq!(
            provider_req.stream_framing,
            StreamFraming::Sse,
            "non-streaming must keep SSE framing"
        );
    }

    /// Streaming encode: URL must end with `/invoke-with-response-stream`
    /// and `stream_framing` must be `AwsEventStream`.
    ///
    /// The `:` in `anthropic.claude-3-5-sonnet-20241022-v2:0` must remain
    /// raw in the URL (Bedrock accepts it; `SigV4` canonicalization handles
    /// percent-encoding).
    #[test]
    fn encode_streaming_url_and_framing() {
        let codec = BedrockClaudeCodec::new("https://bedrock-runtime.us-east-1.amazonaws.com");
        let mut req = LlmRequest::new("anthropic.claude-3-5-sonnet-20241022-v2:0");
        req.stream = true;

        let provider_req = codec.encode_request(&req).expect("encode must succeed");

        // URL must use -with-response-stream suffix.
        assert_eq!(
            provider_req.url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-3-5-sonnet-20241022-v2:0/invoke-with-response-stream",
            "streaming URL must end with /invoke-with-response-stream"
        );

        // Colon in model id must remain raw (not percent-encoded).
        assert!(
            provider_req.url.contains("v2:0"),
            "URL must keep raw ':' in model id"
        );

        // Stream framing must be AwsEventStream for streaming requests.
        assert_eq!(
            provider_req.stream_framing,
            StreamFraming::AwsEventStream,
            "streaming must set AwsEventStream framing"
        );

        // anthropic_version still in body.
        assert_eq!(
            provider_req.body_json.get("anthropic_version").and_then(Value::as_str),
            Some("bedrock-2023-05-31")
        );

        // anthropic-version header still absent.
        assert!(
            !provider_req.headers.contains_key("anthropic-version"),
            "anthropic-version header must be removed even for streaming"
        );
    }

    // ── Stream decoder happy path ──────────────────────────────────────────────

    /// Build a synthetic event-stream frame wrapping a base64-encoded
    /// `message_start` JSON, feed it to the decoder, and assert that the
    /// resulting event is `LlmEvent::MessageStart`.
    #[test]
    fn decoder_happy_path_message_start() {
        // Minimal Anthropic `message_start` JSON (same as what the SSE stream would carry).
        let anthropic_event = r#"{"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-3-5-sonnet","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;

        // Build the base64-encoded payload that Bedrock wraps the event in.
        let b64 = base64::engine::general_purpose::STANDARD.encode(anthropic_event);
        let bedrock_payload = format!(r#"{{"bytes":"{b64}"}}"#);

        // Build a complete event-stream frame with :message-type = "event".
        let mut headers = encode_string_header(":message-type", "event");
        headers.extend(encode_string_header(":event-type", "chunk"));
        let frame_bytes = build_frame(&headers, bedrock_payload.as_bytes());

        // Create the decoder and feed the frame.
        let codec = BedrockClaudeCodec::new("https://bedrock-runtime.us-east-1.amazonaws.com");
        let mut decoder = codec.stream_decoder();
        let raw_frame = RawStreamFrame::new(frame_bytes);
        let events = decoder.decode_frame(raw_frame).expect("decode must succeed");

        assert_eq!(events.len(), 1, "expected exactly one event; got {events:?}");
        match &events[0] {
            LlmEvent::MessageStart { response } => {
                assert_eq!(response.id, "msg_01", "response id must match");
            }
            other => panic!("expected MessageStart, got: {other:?}"),
        }
    }

    /// An exception frame maps to `LlmError::StreamInterrupted` naming the
    /// `:exception-type`.
    #[test]
    fn decoder_exception_frame_returns_error() {
        let payload = br#"{"message":"the model is overloaded"}"#;

        let mut headers = encode_string_header(":message-type", "exception");
        headers.extend(encode_string_header(":exception-type", "ModelStreamErrorException"));
        let frame_bytes = build_frame(&headers, payload);

        let codec = BedrockClaudeCodec::new("https://bedrock-runtime.us-east-1.amazonaws.com");
        let mut decoder = codec.stream_decoder();
        let raw_frame = RawStreamFrame::new(frame_bytes);
        let err = decoder
            .decode_frame(raw_frame)
            .expect_err("exception frame must return an error");

        let msg = format!("{err}");
        assert!(
            msg.contains("ModelStreamErrorException"),
            "error must name the exception type; got: {msg}"
        );
    }
}
