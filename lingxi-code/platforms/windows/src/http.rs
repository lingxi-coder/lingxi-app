//! `reqwest`-backed [`HttpTransport`] for Windows hosts.
//!
//! Mirrors the posix implementation verbatim — `notify`-style duplication
//! keeps the two platform crates library-less. SSE streaming uses
//! `reqwest::Response::bytes_stream()` and a buffered boundary scanner that
//! parses SSE events via the local `parse_sse_chunks` helper (ported verbatim
//! from the former api-client crate, same as `platform-common/src/http.rs`).

use async_trait::async_trait;
use bytes::BytesMut;
use futures_core::stream::Stream;
use futures_util::stream::StreamExt;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::http::SseStream;
use traits::{HttpError, HttpTransport};

/// Production HTTP transport using `reqwest::Client`.
pub struct WindowsHttp {
    /// Default client — follows redirects (reqwest's default policy).
    client: reqwest::Client,
    /// No-redirect client built with `reqwest::redirect::Policy::none()`. Backs
    /// [`HttpTransport::request_no_follow`] so a 3xx is surfaced verbatim
    /// (status + `Location`) — mirrors claude-code's `maxRedirects: 0`. Same
    /// override as `platform-common`'s `ReqwestHttp`.
    no_redirect_client: reqwest::Client,
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
            no_redirect_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest no-redirect client init"),
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
            protocol::HttpMethod::Get => reqwest::Method::GET,
            protocol::HttpMethod::Post => reqwest::Method::POST,
            protocol::HttpMethod::Put => reqwest::Method::PUT,
            protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            protocol::HttpMethod::Head => reqwest::Method::HEAD,
            protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        // Raw bytes take precedence over the string body (see `HttpRequest::body_bytes`).
        if let Some(bytes) = req.body_bytes {
            rb = rb.body(bytes);
        } else if let Some(body) = req.body {
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

    /// Override that sends via the [`Self::no_redirect_client`]
    /// (`redirect::Policy::none()`) so a 3xx is surfaced as `Ok(status=3xx)`
    /// with its `Location` header intact — mirrors claude-code's
    /// `maxRedirects: 0` and `platform-common`'s `ReqwestHttp` override. The
    /// request-build + response-map is identical to [`Self::request`]; only the
    /// client (and thus the redirect policy) differs.
    async fn request_no_follow(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = match req.method {
            protocol::HttpMethod::Get => reqwest::Method::GET,
            protocol::HttpMethod::Post => reqwest::Method::POST,
            protocol::HttpMethod::Put => reqwest::Method::PUT,
            protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            protocol::HttpMethod::Head => reqwest::Method::HEAD,
            protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.no_redirect_client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        // Raw bytes take precedence over the string body (see `HttpRequest::body_bytes`).
        if let Some(bytes) = req.body_bytes {
            rb = rb.body(bytes);
        } else if let Some(body) = req.body {
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
            protocol::HttpMethod::Get => reqwest::Method::GET,
            protocol::HttpMethod::Post => reqwest::Method::POST,
            protocol::HttpMethod::Put => reqwest::Method::PUT,
            protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            protocol::HttpMethod::Head => reqwest::Method::HEAD,
            protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        // Raw bytes take precedence over the string body (see `HttpRequest::body_bytes`).
        if let Some(bytes) = req.body_bytes {
            rb = rb.body(bytes);
        } else if let Some(body) = req.body {
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

    async fn stream_raw_bytes(
        &self,
        req: HttpRequest,
    ) -> Result<traits::http::RawByteStream, HttpError> {
        let method = match req.method {
            protocol::HttpMethod::Get => reqwest::Method::GET,
            protocol::HttpMethod::Post => reqwest::Method::POST,
            protocol::HttpMethod::Put => reqwest::Method::PUT,
            protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            protocol::HttpMethod::Head => reqwest::Method::HEAD,
            protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        // Raw bytes take precedence over the string body (see `HttpRequest::body_bytes`).
        if let Some(bytes) = req.body_bytes {
            rb = rb.body(bytes);
        } else if let Some(body) = req.body {
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
        let s = resp.bytes_stream().map(|r| {
            r.map(|b| b.to_vec())
                .map_err(|e| HttpError::Connection(e.to_string()))
        });
        Ok(Box::pin(s))
    }
}

/// Parse one or more complete SSE events out of a raw chunk.
///
/// The chunk MUST end with `\n\n` to terminate the last event; partial events
/// are dropped. Ported verbatim from the former `api-client/src/sse.rs`.
fn parse_sse_chunks(raw: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for block in raw.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut event_type: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();
        let mut id: Option<String> = None;
        for line in block.lines() {
            if line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("id:") {
                id = Some(rest.trim().to_string());
            }
        }
        if !data_lines.is_empty() {
            events.push(SseEvent {
                event_type,
                data: data_lines.join("\n"),
                id,
            });
        }
    }
    events
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
                    let events = parse_sse_chunks(&format!("{chunk}\n\n"));
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
