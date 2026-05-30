//! Stub [`HttpTransport`] — M1.22 does not wire a real client because the
//! cli-demo uses the engine's effect-only path (no live API call).
//!
//! Both methods return [`HttpError::Connection`] so any accidental
//! engine-side use during M1 surfaces as a transport-level failure rather
//! than a silent no-op. Plan 17 swaps this for a `reqwest`-backed transport.

use async_trait::async_trait;
use lingxi_protocol::{HttpRequest, HttpResponse};
use lingxi_traits::http::SseStream;
use lingxi_traits::{HttpError, HttpTransport};

/// Stub HTTP transport — see module docs.
#[derive(Default)]
pub struct PosixHttp;

impl PosixHttp {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HttpTransport for PosixHttp {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Connection(
            "posix-minimal: HTTP stub (Plan 17 wires the real client)".into(),
        ))
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        Err(HttpError::Connection(
            "posix-minimal: SSE stub (Plan 17 wires the real client)".into(),
        ))
    }
}
