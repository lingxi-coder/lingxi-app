//! Test-only `HttpTransport` mock. Returns canned responses / SSE frames.

use async_trait::async_trait;
use futures::stream;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::http::SseStream;
use traits::{HttpError, HttpTransport};

/// A canned transport for unit tests.
pub(crate) struct MockTransport {
    status: u16,
    body: String,
    sse_frames: Vec<String>,
}

impl MockTransport {
    /// Return one non-streaming response with `status` and `body`.
    pub(crate) fn responding(status: u16, body: impl Into<String>) -> Self {
        Self { status, body: body.into(), sse_frames: Vec::new() }
    }

    /// Return `frames` as successive SSE `data:` payloads (status 200).
    pub(crate) fn streaming(frames: Vec<&str>) -> Self {
        Self {
            status: 200,
            body: String::new(),
            sse_frames: frames.into_iter().map(String::from).collect(),
        }
    }
}

#[async_trait]
impl HttpTransport for MockTransport {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse { status: self.status, headers: Vec::new(), body: self.body.clone() })
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        let frames: Vec<Result<SseEvent, HttpError>> = self
            .sse_frames
            .iter()
            .map(|d| Ok(SseEvent { event_type: None, data: d.clone(), id: None }))
            .collect();
        Ok(Box::pin(stream::iter(frames)))
    }
}
