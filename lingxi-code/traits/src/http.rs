//! HTTP transport abstraction. Provider-neutral; the api-client crate uses
//! this trait so it never imports reqwest directly.

use async_trait::async_trait;
use futures_core::stream::Stream;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use std::pin::Pin;
use thiserror::Error;

/// A pinned, boxed stream of SSE events (each item is `Result<SseEvent, HttpError>`).
///
/// Returned by [`HttpTransport::stream_sse`]; the caller drives it to completion.
pub type SseStream = Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>>;

/// Asynchronous HTTP transport used by `lingxi-api-client`.
///
/// Implementations live in platform crates and wrap a concrete HTTP client
/// (e.g. `reqwest` on posix, native APIs on iOS). Engine code never imports a
/// concrete HTTP library directly — see D17.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Send a request and await the full response.
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;

    /// Open an SSE stream. Caller drives the stream to completion.
    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError>;
}

/// Failure modes for [`HttpTransport`] calls.
#[derive(Debug, Clone, Error)]
pub enum HttpError {
    /// Request did not complete within the timeout.
    #[error("request timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// Transport-level connection failure (DNS, TCP, TLS).
    #[error("connection failed: {0}")]
    Connection(String),

    /// Non-2xx HTTP response.
    #[error("non-success HTTP status {status}: {body}")]
    Status {
        /// Numeric HTTP status code returned by the server.
        status: u16,
        /// Response body verbatim (truncated at the implementation's discretion).
        body: String,
    },

    /// Request could not be assembled or violated client preconditions.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Response could not be decoded or violated expected shape.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Operation was cancelled before completion.
    #[error("cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_display_includes_status() {
        let e = HttpError::Status {
            status: 429,
            body: "rate limited".into(),
        };
        assert!(format!("{e}").contains("429"));
    }
}
