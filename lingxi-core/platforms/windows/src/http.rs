//! `reqwest`-backed [`HttpTransport`] for Windows hosts.
//!
//! Mirrors the posix implementation verbatim — `notify`-style duplication
//! keeps the two platform crates library-less. SSE streaming uses
//! `reqwest::Response::bytes_stream()` and a buffered boundary scanner that
//! defers framing to `lingxi_api_client::sse::parse_sse_chunks`.

use async_trait::async_trait;
use bytes::BytesMut;
use futures_core::stream::Stream;
use futures_util::stream::StreamExt;
use lingxi_protocol::{HttpRequest, HttpResponse, SseEvent};
use lingxi_traits::http::SseStream;
use lingxi_traits::{HttpError, HttpTransport};

/// Production HTTP transport using `reqwest::Client`.
pub struct WindowsHttp {
    client: reqwest::Client,
}

impl WindowsHttp {
    /// Build a new `WindowsHttp` with a fresh `reqwest::Client`.
    ///
    /// # Panics
    /// Panics if the underlying TLS stack cannot be initialised — this is a
    /// fatal startup error and the process should not continue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .build()
                .expect("reqwest client init"),
        }
    }
}

impl Default for WindowsHttp {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for WindowsHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = match req.method {
            lingxi_protocol::HttpMethod::Get => reqwest::Method::GET,
            lingxi_protocol::HttpMethod::Post => reqwest::Method::POST,
            lingxi_protocol::HttpMethod::Put => reqwest::Method::PUT,
            lingxi_protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            lingxi_protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            lingxi_protocol::HttpMethod::Head => reqwest::Method::HEAD,
            lingxi_protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(body) = req.body {
            rb = rb.body(body);
        }
        if let Some(timeout) = req.timeout {
            rb = rb.timeout(timeout);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp
            .text()
            .await
            .map_err(|e| HttpError::InvalidResponse(e.to_string()))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        let method = match req.method {
            lingxi_protocol::HttpMethod::Get => reqwest::Method::GET,
            lingxi_protocol::HttpMethod::Post => reqwest::Method::POST,
            lingxi_protocol::HttpMethod::Put => reqwest::Method::PUT,
            lingxi_protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            lingxi_protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            lingxi_protocol::HttpMethod::Head => reqwest::Method::HEAD,
            lingxi_protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(body) = req.body {
            rb = rb.body(body);
        }
        if let Some(timeout) = req.timeout {
            rb = rb.timeout(timeout);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, body });
        }

        let byte_stream = resp.bytes_stream();
        let event_stream = sse_event_stream(byte_stream);
        Ok(Box::pin(event_stream))
    }
}

/// Adapt a byte stream into a stream of complete `SseEvent`s.
///
/// Buffers raw bytes until an event boundary (`\n\n` or `\r\n\r\n`) is found,
/// then hands the buffered slice off to `parse_sse_chunks`. Each emitted
/// element is a single fully-formed `SseEvent` (or a connection error).
pub(crate) fn sse_event_stream<S>(
    byte_stream: S,
) -> impl Stream<Item = Result<SseEvent, HttpError>> + Send
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let buffer = BytesMut::new();
    futures_util::stream::unfold(
        (Box::pin(byte_stream), buffer),
        |(mut s, mut buf)| async move {
            loop {
                if let Some((event_len, boundary_len)) = find_event_boundary(&buf) {
                    let event_bytes = buf.split_to(event_len).to_vec();
                    drop(buf.split_to(boundary_len));
                    let chunk = String::from_utf8_lossy(&event_bytes).to_string();
                    let events = lingxi_api_client::sse::parse_sse_chunks(&format!("{chunk}\n\n"));
                    if let Some(ev) = events.into_iter().next() {
                        return Some((Ok(ev), (s, buf)));
                    }
                    continue;
                }
                match s.next().await {
                    Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                    Some(Err(e)) => {
                        return Some((Err(HttpError::Connection(e.to_string())), (s, buf)))
                    }
                    None => return None,
                }
            }
        },
    )
}

/// Locate the first SSE event boundary in `buf`.
///
/// Returns `(event_len, boundary_len)` where `event_len` is the byte length of
/// the event payload (everything before the boundary) and `boundary_len` is
/// the length of the boundary itself — `2` for `\n\n` or `4` for `\r\n\r\n`.
pub(crate) fn find_event_boundary(buf: &BytesMut) -> Option<(usize, usize)> {
    let bytes = buf.as_ref();
    let lf_pos = bytes.windows(2).position(|w| w == b"\n\n");
    let crlf_pos = bytes.windows(4).position(|w| w == b"\r\n\r\n");
    match (lf_pos, crlf_pos) {
        (Some(lf), Some(crlf)) => {
            if crlf <= lf {
                Some((crlf, 4))
            } else {
                Some((lf, 2))
            }
        }
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}
