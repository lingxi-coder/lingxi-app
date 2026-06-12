//! HTTP transport abstraction. Provider-neutral; the api-client crate uses
//! this trait so it never imports reqwest directly.

use async_trait::async_trait;
use futures_core::stream::Stream;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;

/// A pinned, boxed stream of SSE events (each item is `Result<SseEvent, HttpError>`).
///
/// Returned by [`HttpTransport::stream_sse`]; the caller drives it to completion.
pub type SseStream = Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>>;

/// A pinned, boxed stream of raw response-body byte chunks (each item is
/// `Result<Vec<u8>, HttpError>`).
///
/// Returned by [`HttpTransport::stream_raw_bytes`] for binary response protocols
/// (e.g. the AWS event-stream used by Bedrock streaming); the caller frames and
/// interprets the bytes.
pub type RawByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, HttpError>> + Send>>;

/// SSE stream together with the HTTP response metadata that preceded it.
///
/// Returned by [`HttpTransport::stream_sse_with_meta`]. The status and headers
/// are captured from the response line/headers before any SSE data arrives, so
/// callers can inspect them immediately — for example, to honour a
/// `retry-after` header on a connect-phase 429.
///
/// # Note on the default implementation
///
/// The default [`HttpTransport::stream_sse_with_meta`] loses metadata: it
/// delegates to [`HttpTransport::stream_sse`] which surfaces neither status nor
/// headers.  Real transports (e.g. `ReqwestHttp` in `platform-common`)
/// override this method to capture the metadata before handing the
/// byte-stream to the SSE decoder.  Use the override wherever accurate
/// retry-after / rate-limit tracking matters.
pub struct SseStreamWithMeta {
    /// HTTP status of the streaming response (e.g. 200, 429).
    pub status: u16,
    /// Response headers, lowercased names (e.g. `"retry-after"`).
    pub headers: Vec<(String, String)>,
    /// The SSE event stream; drive to completion as usual.
    pub stream: SseStream,
}

/// Raw byte stream together with the HTTP response metadata that preceded it.
///
/// Returned by [`HttpTransport::stream_raw_bytes_with_meta`]. The status and
/// headers are captured from the response line/headers before any body bytes
/// arrive — exactly as [`SseStreamWithMeta`] does for SSE — so callers can
/// inspect rate-limit and other headers immediately.
///
/// Used by the bridge's AWS event-stream path: the bridge calls this method
/// instead of [`HttpTransport::stream_raw_bytes`] so it can forward real
/// response headers (e.g. `retry-after`) upstream even for binary-framed
/// responses.
///
/// # Note on the default implementation
///
/// The default [`HttpTransport::stream_raw_bytes_with_meta`] delegates to
/// [`HttpTransport::stream_raw_bytes`] with a synthetic `status: 200` and
/// empty headers — it loses metadata that a real transport would surface.
/// Override in production transports (e.g. `ReqwestHttp`) to capture the
/// metadata before streaming body bytes.
pub struct RawByteStreamWithMeta {
    /// HTTP status of the streaming response (e.g. 200, 429).
    pub status: u16,
    /// Response headers, lowercased names (e.g. `"retry-after"`).
    pub headers: Vec<(String, String)>,
    /// The raw byte chunk stream; drive to completion and interpret framing.
    pub stream: RawByteStream,
}

/// A `Stream` that yields a single chunk then ends. Backs the default
/// [`HttpTransport::stream_raw_bytes`] (buffer-the-body) impl without pulling a
/// stream-combinator dependency into this leaf crate.
struct OnceBytes(Option<Result<Vec<u8>, HttpError>>);

impl Stream for OnceBytes {
    type Item = Result<Vec<u8>, HttpError>;
    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take())
    }
}

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

    /// Open an SSE stream, also capturing the HTTP status and response headers.
    ///
    /// The default implementation wraps [`Self::stream_sse`] with a synthetic
    /// `status: 200` and empty headers — it loses metadata that the platform
    /// transport would otherwise surface.  Override in production transports so
    /// connect-phase rate-limit headers (`retry-after`, etc.) are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError`] on connection failure or a non-2xx status that the
    /// transport surfaces as an error (behaviour depends on the implementation).
    async fn stream_sse_with_meta(
        &self,
        req: HttpRequest,
    ) -> Result<SseStreamWithMeta, HttpError> {
        Ok(SseStreamWithMeta {
            status: 200,
            headers: Vec::new(),
            stream: self.stream_sse(req).await?,
        })
    }

    /// Open a raw byte stream for a binary response protocol (e.g. the AWS
    /// event-stream used by Bedrock streaming). The caller drives it to
    /// completion and interprets the framing.
    ///
    /// The default impl buffers the full response via [`Self::request`] and
    /// yields it as a single chunk — correct for decoders that frame a complete
    /// buffer, and for test transports. Production transports override this for
    /// true incremental byte streaming. Additive (default-provided) so existing
    /// implementations compile unchanged.
    async fn stream_raw_bytes(&self, req: HttpRequest) -> Result<RawByteStream, HttpError> {
        let resp = self.request(req).await?;
        if resp.status >= 400 {
            return Err(HttpError::Status {
                status: resp.status,
                body: resp.body,
            });
        }
        Ok(Box::pin(OnceBytes(Some(Ok(resp.body.into_bytes())))))
    }

    /// Open a raw byte stream, also capturing the HTTP status and response
    /// headers before any body bytes arrive.
    ///
    /// The default implementation wraps [`Self::stream_raw_bytes`] with a
    /// synthetic `status: 200` and empty headers — it loses metadata that the
    /// platform transport would otherwise surface.  Override in production
    /// transports so connect-phase rate-limit headers (`retry-after`, etc.)
    /// are preserved for binary-framed streaming responses.
    ///
    /// # ≥400 error-arm behaviour
    ///
    /// Production overrides (e.g. `ReqwestHttp`) return `Ok` with the real
    /// status and headers even for error responses, mirroring the
    /// [`Self::stream_sse_with_meta`] contract.  The default falls back to
    /// [`Self::stream_raw_bytes`], which surfaces `Err(HttpError::Status)` for
    /// non-2xx — callers of the default lose headers on error paths.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError`] on connection failure. Non-2xx handling depends
    /// on the implementation (see above).
    async fn stream_raw_bytes_with_meta(
        &self,
        req: HttpRequest,
    ) -> Result<RawByteStreamWithMeta, HttpError> {
        Ok(RawByteStreamWithMeta {
            status: 200,
            headers: Vec::new(),
            stream: self.stream_raw_bytes(req).await?,
        })
    }
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
    use async_trait::async_trait;
    use protocol::HttpMethod;

    #[test]
    fn http_error_display_includes_status() {
        let e = HttpError::Status {
            status: 429,
            body: "rate limited".into(),
        };
        assert!(format!("{e}").contains("429"));
    }

    /// Minimal transport whose `request` returns a fixed body, to exercise the
    /// default `stream_raw_bytes` (buffer-the-body) impl.
    struct OneShot;

    #[async_trait]
    impl HttpTransport for OneShot {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: "hello".to_string(),
            })
        }
        async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
            Err(HttpError::InvalidRequest("sse not supported in this mock".to_string()))
        }
    }

    fn get_req() -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Get,
            url: "http://x.local".to_string(),
            headers: vec![],
            body: None,
            body_bytes: None,
            timeout: None,
        }
    }

    /// The default `stream_sse_with_meta` wraps `stream_sse` with status 200
    /// and empty headers — it must compile and forward events correctly.
    #[tokio::test]
    async fn default_stream_sse_with_meta_uses_status_200_empty_headers() {
        struct SseOnce;

        #[async_trait]
        impl HttpTransport for SseOnce {
            async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
                Err(HttpError::InvalidRequest("not used".to_string()))
            }
            async fn stream_sse(
                &self,
                _req: HttpRequest,
            ) -> Result<SseStream, HttpError> {
                // Return an empty stream.
                use futures_core::stream::Stream;
                use std::pin::Pin;
                use std::task::{Context, Poll};
                struct Empty;
                impl Stream for Empty {
                    type Item = Result<protocol::SseEvent, HttpError>;
                    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                        Poll::Ready(None)
                    }
                }
                Ok(Box::pin(Empty))
            }
        }

        let t = SseOnce;
        let meta = t
            .stream_sse_with_meta(get_req())
            .await
            .expect("default must succeed");
        assert_eq!(meta.status, 200, "default status must be 200");
        assert!(
            meta.headers.is_empty(),
            "default headers must be empty"
        );
    }

    #[tokio::test]
    async fn default_stream_raw_bytes_yields_full_body_once() {
        let t = OneShot;
        let mut s = t.stream_raw_bytes(get_req()).await.unwrap();
        let first = std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await;
        assert_eq!(first.unwrap().unwrap(), b"hello".to_vec());
        // stream ends after the single chunk
        let second = std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await;
        assert!(second.is_none());
    }

    /// The default `stream_raw_bytes_with_meta` wraps `stream_raw_bytes` with a
    /// synthetic status 200 and empty headers — it must compile, forward the body
    /// chunk, and report the synthetic metadata.
    #[tokio::test]
    async fn default_stream_raw_bytes_with_meta_uses_status_200_empty_headers() {
        let t = OneShot;
        let meta = t
            .stream_raw_bytes_with_meta(get_req())
            .await
            .expect("default must succeed");
        assert_eq!(meta.status, 200, "default status must be 200");
        assert!(meta.headers.is_empty(), "default headers must be empty");

        let mut s = meta.stream;
        let first = std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await;
        assert_eq!(
            first.unwrap().unwrap(),
            b"hello".to_vec(),
            "body chunk must be forwarded"
        );
        let second = std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await;
        assert!(second.is_none(), "stream must end after single chunk");
    }
}
