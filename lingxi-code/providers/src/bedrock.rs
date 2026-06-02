//! Claude-on-Bedrock provider: the Anthropic Messages body over Bedrock's
//! `InvokeModel` endpoints + `SigV4` auth.
//!
//! `complete()` uses `/invoke` (a single Anthropic-shaped JSON response).
//! `stream()` uses `/invoke-with-response-stream`, decoding the AWS binary
//! event-stream (`application/vnd.amazon.eventstream`) frame-by-frame with the
//! crate-local [`crate::eventstream`] decoder: each `chunk` frame wraps a base64
//! blob whose bytes are an Anthropic stream event, mapped to a canonical
//! [`StreamEvent`] via the shared [`crate::anthropic_wire`] mapper.

use crate::authenticator::Authenticator;
use crate::capabilities::Capabilities;
use crate::eventstream::{EventStreamDecoder, Frame};
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::{self, BoxStream, StreamExt};
use protocol::{HttpMethod, HttpRequest};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
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

    fn invoke_stream_url(&self, model: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{model}/invoke-with-response-stream",
            self.region
        )
    }
}

/// Decode a standard (RFC 4648) base64 string into bytes. Bedrock wraps each
/// streamed chunk as `{"bytes":"<base64 anthropic-event-json>"}`. Whitespace and
/// `=` padding are ignored; returns `None` on an invalid character.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn sextet(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' || c == b' ' {
            continue;
        }
        acc = (acc << 6) | sextet(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            // Mask to the low 8 bits; `try_from` is then infallible and avoids a
            // truncating `as` cast.
            out.push(u8::try_from((acc >> bits) & 0xFF).unwrap_or(0));
        }
    }
    Some(out)
}

/// Map one decoded event-stream frame to zero or more canonical [`StreamEvent`]s.
///
/// Bedrock emits `:message-type = event` frames; a `:event-type = chunk` frame's
/// JSON payload is `{"bytes":"<base64>"}` whose decoded bytes are an Anthropic
/// stream event. `exception`/`error` frames surface as an [`ApiError`].
fn frame_to_events(frame: &Frame) -> Result<Vec<StreamEvent>, ApiError> {
    let message_type = frame.header(":message-type");
    if message_type == Some("exception")
        || message_type == Some("error")
        || frame.header(":exception-type").is_some()
    {
        let body = String::from_utf8_lossy(&frame.payload).to_string();
        return Err(ApiError::Server { status: 500, body });
    }
    match frame.header(":event-type") {
        Some("chunk") => {
            let outer: Value = serde_json::from_slice(&frame.payload)
                .map_err(|e| ApiError::MalformedStream(format!("bedrock chunk envelope: {e}")))?;
            let b64 = outer.get("bytes").and_then(Value::as_str).ok_or_else(|| {
                ApiError::MalformedStream("bedrock chunk missing `bytes`".to_string())
            })?;
            let inner = base64_decode(b64)
                .ok_or_else(|| ApiError::MalformedStream("bedrock chunk bad base64".to_string()))?;
            Ok(vec![crate::anthropic_wire::map_event_json(&inner)?])
        }
        // Other event types (e.g. metadata pings) carry no canonical event.
        _ => Ok(vec![]),
    }
}

/// Decode an AWS event-stream byte stream into canonical [`StreamEvent`]s.
fn decode_event_stream(
    byte_stream: traits::http::RawByteStream,
) -> impl futures::Stream<Item = Result<StreamEvent, ApiError>> + Send {
    struct State {
        stream: traits::http::RawByteStream,
        decoder: EventStreamDecoder,
        pending: VecDeque<Result<StreamEvent, ApiError>>,
        done: bool,
    }
    let state = State {
        stream: byte_stream,
        decoder: EventStreamDecoder::new(),
        pending: VecDeque::new(),
        done: false,
    };
    stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.pending.pop_front() {
                return Some((item, st));
            }
            if st.done {
                return None;
            }
            match st.decoder.next_frame() {
                Ok(Some(frame)) => match frame_to_events(&frame) {
                    Ok(events) => st.pending.extend(events.into_iter().map(Ok)),
                    Err(e) => {
                        st.done = true;
                        st.pending.push_back(Err(e));
                    }
                },
                Ok(None) => match st.stream.next().await {
                    Some(Ok(chunk)) => st.decoder.extend(&chunk),
                    Some(Err(e)) => {
                        st.done = true;
                        return Some((Err(ApiError::Http(e)), st));
                    }
                    None => return None,
                },
                Err(e) => {
                    st.done = true;
                    return Some((
                        Err(ApiError::MalformedStream(format!(
                            "bedrock event-stream frame: {e}"
                        ))),
                        st,
                    ));
                }
            }
        }
    })
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
        let body = Self::build_body(&req);
        let mut http = HttpRequest {
            method: HttpMethod::Post,
            url: self.invoke_stream_url(&req.model),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some(body.to_string()),
            timeout: Some(Duration::from_secs(600)),
        };
        self.authenticator.authorize(&mut http).await?;
        let bytes = self
            .transport
            .stream_raw_bytes(http)
            .await
            .map_err(ApiError::Http)?;
        Ok(decode_event_stream(bytes).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::ContentDelta;
    use crate::eventstream::encode_frame;

    #[test]
    fn build_body_has_anthropic_version_no_model() {
        let body = BedrockProvider::build_body(&CanonicalRequest::new("anthropic.claude-3-5-sonnet-20241022-v2:0"));
        assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
        assert!(body.get("max_tokens").is_some());
        assert!(body.get("messages").is_some());
        assert!(body.get("model").is_none());
    }

    #[test]
    fn base64_decode_roundtrips_standard_alphabet() {
        // "hello world!" → known base64 (with padding).
        assert_eq!(
            base64_decode("aGVsbG8gd29ybGQh").unwrap(),
            b"hello world!".to_vec()
        );
        assert_eq!(base64_decode("YWI=").unwrap(), b"ab".to_vec());
        assert_eq!(base64_decode("YQ==").unwrap(), b"a".to_vec());
        assert!(base64_decode("not*base64").is_none());
    }

    /// Minimal standard base64 encoder (test-only) to build chunk payloads.
    fn b64_encode(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0];
            let b1 = *chunk.get(1).unwrap_or(&0);
            let b2 = *chunk.get(2).unwrap_or(&0);
            out.push(T[(b0 >> 2) as usize] as char);
            out.push(T[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 {
                T[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                T[(b2 & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    /// Build one Bedrock `chunk` event-stream frame wrapping `anthropic_event`.
    fn chunk_frame(anthropic_event: &str) -> Vec<u8> {
        let payload =
            serde_json::json!({ "bytes": b64_encode(anthropic_event.as_bytes()) }).to_string();
        encode_frame(
            &[
                (":message-type", "event"),
                (":event-type", "chunk"),
                (":content-type", "application/json"),
            ],
            payload.as_bytes(),
        )
    }

    #[tokio::test]
    async fn decodes_bedrock_event_stream_to_canonical_events() {
        let f1 = chunk_frame(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        );
        let f2 = chunk_frame(r#"{"type":"message_stop"}"#);
        let mut all = Vec::new();
        all.extend_from_slice(&f1);
        all.extend_from_slice(&f2);
        // Split mid-frame so the decoder must reassemble across chunk boundaries.
        let mid = all.len() / 2;
        let chunks: Vec<Result<Vec<u8>, traits::HttpError>> =
            vec![Ok(all[..mid].to_vec()), Ok(all[mid..].to_vec())];
        let byte_stream: traits::http::RawByteStream = Box::pin(stream::iter(chunks));
        let events: Vec<StreamEvent> = decode_event_stream(byte_stream)
            .map(|r| r.expect("event decodes"))
            .collect()
            .await;
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hi"
        )));
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }
}
