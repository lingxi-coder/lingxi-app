//! Transport contract between prepared provider requests and host HTTP stacks.
//!
//! `llm-client` never talks HTTP itself. Hosts implement [`Transport`] over
//! their HTTP client and the client orchestrates encode → send → decode
//! around it.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use crate::{LlmError, ProviderRequest, ProviderResponse, RawStreamFrame};

/// Boxed future alias keeping [`Transport`] object-safe without an
/// `async_trait` dependency.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Asynchronous HTTP seam implemented by hosts.
///
/// Implementations own HTTP semantics only: connection, TLS, and timeout
/// failures map to [`LlmError::Transport`]. Provider payloads are never
/// interpreted here — non-2xx responses come back as data and the
/// orchestration layer routes them through the codec error taxonomy.
pub trait Transport: Send + Sync {
    /// Send a request and await the complete response.
    fn execute<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>>;

    /// Open a streaming response.
    ///
    /// On success each frame is one SSE `data:` payload without the field
    /// prefix (see [`crate::SseFrameSplitter`] for byte-stream hosts). For
    /// non-2xx statuses the frames carry raw body bytes instead.
    fn open_stream<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>>;

    /// Open a reusable OpenAI Responses WebSocket session.
    ///
    /// The default keeps existing transports source-compatible and reports the
    /// capability as unsupported. Hosts that support WebSocket reuse override
    /// this and return a session that can send sequential `response.create`
    /// messages over one upgraded connection.
    fn open_responses_websocket_session<'a>(
        &'a self,
        _request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<Box<dyn ResponsesWebSocketTransportSession>, LlmError>> {
        Box::pin(async {
            Err(LlmError::InvalidRequest {
                message: "Responses WebSocket session transport is not supported".to_string(),
            })
        })
    }
}

/// Reusable transport session for OpenAI Responses WebSocket requests.
pub trait ResponsesWebSocketTransportSession: Send {
    /// Send one prepared provider request over the already-open connection.
    fn send<'a>(
        &'a mut self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>>;

    /// Close the reusable transport connection.
    fn close<'a>(&'a mut self) -> BoxFuture<'a, Result<(), LlmError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Status line and headers of a streaming response, plus its frame stream.
pub struct StreamingResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers; lowercase names expected.
    pub headers: BTreeMap<String, String>,
    /// Frame stream, drained by the orchestration layer.
    pub frames: Box<dyn FrameStream>,
}

/// Pull-based stream of raw frames.
pub trait FrameStream: Send {
    /// Next frame; `Ok(None)` is the normal end of stream.
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>>;
}
