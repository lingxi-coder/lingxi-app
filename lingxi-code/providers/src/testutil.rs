//! Test-only `HttpTransport` mock. Returns canned responses / SSE frames.

use async_trait::async_trait;
use futures::stream;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use std::sync::{Arc, Mutex};
use traits::http::SseStream;
use traits::{HttpError, HttpTransport};

/// A canned transport for unit tests.
pub(crate) struct MockTransport {
    status: u16,
    body: String,
    sse_frames: Vec<String>,
    error: Option<String>,
    /// Interior-mutable slot that records the last [`HttpRequest`] received by
    /// either [`request`](HttpTransport::request) or
    /// [`stream_sse`](HttpTransport::stream_sse). Tests acquire a handle via
    /// [`MockTransport::captured_handle`] before handing the transport to the
    /// client under test.
    captured: Arc<Mutex<Option<HttpRequest>>>,
}

impl MockTransport {
    /// Return one non-streaming response with `status` and `body`.
    pub(crate) fn responding(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            sse_frames: Vec::new(),
            error: None,
            captured: Arc::new(Mutex::new(None)),
        }
    }

    /// Return `frames` as successive SSE `data:` payloads (status 200).
    pub(crate) fn streaming(frames: Vec<&str>) -> Self {
        Self {
            status: 200,
            body: String::new(),
            sse_frames: frames.into_iter().map(String::from).collect(),
            error: None,
            captured: Arc::new(Mutex::new(None)),
        }
    }

    /// Yield a single transport error from `stream_sse`.
    pub(crate) fn erroring(msg: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: String::new(),
            sse_frames: Vec::new(),
            error: Some(msg.into()),
            captured: Arc::new(Mutex::new(None)),
        }
    }

    /// Return a cloned [`Arc`] handle to the captured-request slot.
    ///
    /// Call this **before** handing the transport to the client; after the call
    /// under test completes the slot holds the last [`HttpRequest`] that reached
    /// the transport.
    #[must_use]
    pub(crate) fn captured_handle(&self) -> Arc<Mutex<Option<HttpRequest>>> {
        Arc::clone(&self.captured)
    }
}

#[async_trait]
impl HttpTransport for MockTransport {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        *self.captured.lock().unwrap() = Some(req.clone());
        Ok(HttpResponse {
            status: self.status,
            headers: Vec::new(),
            body: self.body.clone(),
        })
    }

    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        *self.captured.lock().unwrap() = Some(req.clone());
        if let Some(msg) = &self.error {
            let one: Vec<Result<SseEvent, HttpError>> =
                vec![Err(HttpError::Connection(msg.clone()))];
            return Ok(Box::pin(stream::iter(one)));
        }
        let frames: Vec<Result<SseEvent, HttpError>> = self
            .sse_frames
            .iter()
            .map(|d| {
                Ok(SseEvent {
                    event_type: None,
                    data: d.clone(),
                    id: None,
                })
            })
            .collect();
        Ok(Box::pin(stream::iter(frames)))
    }
}
