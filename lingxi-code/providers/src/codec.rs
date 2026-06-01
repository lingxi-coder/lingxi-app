//! The pure translation contract. No I/O lives here — `encode_request`
//! builds a request, `decode_response` parses a body, and `SseDecoder`
//! turns provider SSE frames into canonical stream events.

use crate::error::CodecError;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use protocol::HttpRequest;

/// Pure encode/decode for one provider's wire format.
pub trait WireCodec: Send + Sync {
    /// Build the native HTTP request for `req` (auth-agnostic — the
    /// `GenericClient`'s `Authenticator` attaches credentials afterward).
    ///
    /// # Errors
    /// Returns [`CodecError`] if the request cannot be represented.
    fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError>;

    /// Decode a non-streaming response body into the canonical shape.
    ///
    /// # Errors
    /// Returns [`ApiError`] for non-success responses or malformed bodies.
    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError>;

    /// Create a fresh per-stream decoder for this provider's SSE format.
    fn new_stream_decoder(&self) -> Box<dyn SseDecoder>;
}

/// A stateful per-stream decoder. `push` is called once per SSE `data:`
/// payload; `finish` flushes any trailing state at end-of-stream.
pub trait SseDecoder: Send {
    /// Decode one SSE `data:` payload into zero or more canonical events.
    fn push(&mut self, data: &str) -> Vec<StreamEvent>;

    /// Flush any buffered trailing events when the stream ends.
    fn finish(&mut self) -> Vec<StreamEvent>;
}
