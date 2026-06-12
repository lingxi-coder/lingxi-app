//! Shared `reqwest`-backed [`HttpTransport`] for native hosts.
//!
//! This is the single source of truth for the production HTTP/SSE client.
//! `platforms/posix` (desktop) re-exports [`ReqwestHttp`] as `PosixHttp`, and
//! the mobile device platforms (`platforms/ios`, `platforms/android`) use it
//! directly so a keyed conversation streams against the real provider.
//!
//! Implements request/response via `reqwest::Client`. SSE streaming is wired
//! through `reqwest::Response::bytes_stream()` and a buffered `\n\n` /
//! `\r\n\r\n` boundary scanner that parses SSE events via the local
//! `parse_sse_chunks` helper (ported verbatim from the former api-client crate).

use async_trait::async_trait;
use bytes::BytesMut;
use futures_core::stream::Stream;
use futures_util::stream::StreamExt;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::http::{RawByteStream, RawByteStreamWithMeta, SseStream, SseStreamWithMeta};
use traits::{HttpError, HttpTransport};

/// Production HTTP transport using `reqwest::Client`.
///
/// Built on `reqwest` with the `rustls-tls` backend (no OpenSSL), which
/// cross-compiles to `aarch64-apple-ios` and Android targets. Shared by all
/// native platforms.
pub struct ReqwestHttp {
    client: reqwest::Client,
}

impl ReqwestHttp {
    /// Build a new `ReqwestHttp` with a fresh `reqwest::Client`.
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

impl Default for ReqwestHttp {
    fn default() -> Self {
        Self::new()
    }
}

fn to_reqwest_method(method: protocol::HttpMethod) -> reqwest::Method {
    match method {
        protocol::HttpMethod::Get => reqwest::Method::GET,
        protocol::HttpMethod::Post => reqwest::Method::POST,
        protocol::HttpMethod::Put => reqwest::Method::PUT,
        protocol::HttpMethod::Patch => reqwest::Method::PATCH,
        protocol::HttpMethod::Delete => reqwest::Method::DELETE,
        protocol::HttpMethod::Head => reqwest::Method::HEAD,
        protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
    }
}

/// Build a `reqwest::RequestBuilder` from a protocol `HttpRequest`.
///
/// Shared by all `HttpTransport` method impls on [`ReqwestHttp`] so headers,
/// body, and timeout are applied identically regardless of which method
/// ([`HttpTransport::request`] / [`HttpTransport::stream_sse`] /
/// [`HttpTransport::stream_sse_with_meta`] / [`HttpTransport::stream_raw_bytes`])
/// calls it.
fn build_reqwest(
    client: &reqwest::Client,
    req: HttpRequest,
) -> reqwest::RequestBuilder {
    let mut rb = client.request(to_reqwest_method(req.method), &req.url);
    for (k, v) in &req.headers {
        rb = rb.header(k, v);
    }
    if let Some(body) = req.body {
        rb = rb.body(body);
    }
    if let Some(timeout) = req.timeout {
        rb = rb.timeout(timeout);
    }
    rb
}

#[async_trait]
impl HttpTransport for ReqwestHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let resp = build_reqwest(&self.client, req)
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
        let resp = build_reqwest(&self.client, req)
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            // Surface the status to the caller — claude-code returns this as
            // an error event on the same stream, but we keep them separate
            // because the trait promises `Result<SseStream, HttpError>` at the
            // open boundary, not after the first event.
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, body });
        }

        let byte_stream = resp.bytes_stream();
        let event_stream = sse_event_stream(byte_stream);
        Ok(Box::pin(event_stream))
    }

    /// Override that captures the real HTTP status and response headers
    /// (lowercased) before the SSE event stream begins.
    ///
    /// Unlike the default (which loses metadata by delegating to
    /// [`Self::stream_sse`]), this override reads the response line and
    /// headers before handing the byte-stream to the SSE decoder — so callers
    /// can immediately inspect rate-limit headers such as `retry-after`.
    ///
    /// # ≥400 error-arm behaviour
    ///
    /// For non-2xx responses this method returns `Ok(SseStreamWithMeta{status:
    /// 4xx, headers: <real headers>, stream: <body-as-single-raw-data-frame>})`
    /// so the bridge's existing ≥400 drain path receives real response headers
    /// (including `retry-after`, `anthropic-ratelimit-*`, etc.) instead of an
    /// empty `BTreeMap`. The body is emitted as a single SSE-`data:`-style raw
    /// frame that the bridge drains and passes to the codec.
    ///
    /// The bare `Err(HttpError::Status)` arm is now only reached by transports
    /// that do NOT override `stream_sse_with_meta` (i.e. the default
    /// implementation in `traits`). Those callers produce empty headers as
    /// before — no behaviour change for default-impl transports.
    async fn stream_sse_with_meta(
        &self,
        req: HttpRequest,
    ) -> Result<SseStreamWithMeta, HttpError> {
        let resp = build_reqwest(&self.client, req)
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        // Capture headers (lowercased) BEFORE consuming the body, whether the
        // response is a success or an error — this is the key improvement over
        // `stream_sse`: error responses now carry real headers.
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();

        if status >= 400 {
            // Return Ok so the bridge receives real headers alongside the body.
            // The body is delivered as a single raw SSE data frame so the
            // bridge's drain loop (which calls `next_frame()` until `None`)
            // accumulates it into the JSON body handed to `decode_response`.
            let body = resp.bytes().await.unwrap_or_default();
            let body_bytes = body.to_vec();
            let stream = body_error_stream(body_bytes);
            return Ok(SseStreamWithMeta {
                status,
                headers,
                stream: Box::pin(stream),
            });
        }

        let byte_stream = resp.bytes_stream();
        let event_stream = sse_event_stream(byte_stream);
        Ok(SseStreamWithMeta {
            status,
            headers,
            stream: Box::pin(event_stream),
        })
    }

    async fn stream_raw_bytes(
        &self,
        req: HttpRequest,
    ) -> Result<RawByteStream, HttpError> {
        let resp = build_reqwest(&self.client, req)
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, body });
        }
        // Map reqwest's `Bytes` chunks to owned `Vec<u8>` for true incremental
        // streaming (AWS event-stream frames arrive across chunks).
        let s = resp.bytes_stream().map(|r| {
            r.map(|b| b.to_vec())
                .map_err(|e| HttpError::Connection(e.to_string()))
        });
        Ok(Box::pin(s))
    }

    /// Override that captures the real HTTP status and response headers before
    /// the raw byte stream begins.
    ///
    /// Mirrors the [`Self::stream_sse_with_meta`] override: status and headers
    /// are captured from the response line before any body bytes are consumed,
    /// so callers can immediately inspect rate-limit headers.
    ///
    /// # ≥400 error-arm behaviour
    ///
    /// For non-2xx responses this method returns
    /// `Ok(RawByteStreamWithMeta{status: 4xx, headers: <real headers>, stream:
    /// <body-as-one-chunk>})` so the bridge's drain path receives real response
    /// headers instead of an empty set.  The body is emitted as a single chunk.
    async fn stream_raw_bytes_with_meta(
        &self,
        req: HttpRequest,
    ) -> Result<RawByteStreamWithMeta, HttpError> {
        let resp = build_reqwest(&self.client, req)
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        // Capture headers (lowercased) BEFORE consuming the body.
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();

        if status >= 400 {
            // Return Ok so the bridge can receive real headers alongside the body.
            let body_bytes = resp.bytes().await.unwrap_or_default().to_vec();
            let stream: RawByteStream = Box::pin(futures_util::stream::once(async move {
                Ok::<Vec<u8>, HttpError>(body_bytes)
            }));
            return Ok(RawByteStreamWithMeta {
                status,
                headers,
                stream,
            });
        }

        // Success: map reqwest's `Bytes` chunks to owned `Vec<u8>`.
        let byte_stream = resp.bytes_stream().map(|r| {
            r.map(|b| b.to_vec())
                .map_err(|e| HttpError::Connection(e.to_string()))
        });
        Ok(RawByteStreamWithMeta {
            status,
            headers,
            stream: Box::pin(byte_stream),
        })
    }
}

/// Parse one or more complete SSE events out of a raw chunk.
///
/// The chunk MUST end with `\n\n` to terminate the last event; partial events
/// are dropped. Conforms to the HTML SSE spec (event / data / id fields;
/// comment lines starting with `:` are skipped).
///
/// Ported verbatim from the former `api-client/src/sse.rs` so the wire
/// behaviour is byte-identical after the api-client crate is removed.
///
/// NOTE: duplicated in `platforms/windows/src/http.rs` — keep in sync.
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
                continue; // comment / keepalive
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
                // First, drain any complete events already in the buffer.
                if let Some((event_len, boundary_len)) = find_event_boundary(&buf) {
                    let event_bytes = buf.split_to(event_len).to_vec();
                    // Consume the boundary itself.
                    drop(buf.split_to(boundary_len));
                    let chunk = String::from_utf8_lossy(&event_bytes).to_string();
                    let events = parse_sse_chunks(&format!("{chunk}\n\n"));
                    if let Some(ev) = events.into_iter().next() {
                        return Some((Ok(ev), (s, buf)));
                    }
                    // Empty event (comment / keepalive only); keep draining.
                    continue;
                }
                // Otherwise pull more bytes.
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

/// Adapt an error-response body (raw bytes) into a single-item `SseEvent` stream.
///
/// The body is delivered as one `SseEvent{data: <body-as-utf8>, …}` frame so the
/// bridge's `SseFrames::next_frame` path yields the raw bytes that the codec
/// `decode_response` call expects.  The stream terminates immediately after that
/// single frame (returns `None` on the second poll) so the bridge drain loop sees
/// exactly one frame.
///
/// This is the companion to the `stream_sse_with_meta` ≥400 path — it lets the
/// bridge receive real headers and the error body without an `Err(HttpError::Status)`
/// short-circuit.
pub(crate) fn body_error_stream(
    body: Vec<u8>,
) -> impl Stream<Item = Result<SseEvent, HttpError>> + Send {
    futures_util::stream::once(async move {
        let data = String::from_utf8_lossy(&body).into_owned();
        Ok(SseEvent {
            event_type: None,
            data,
            id: None,
        })
    })
}

/// Locate the first SSE event boundary in `buf`.
///
/// Returns `(event_len, boundary_len)` where `event_len` is the byte length of
/// the event payload (everything before the boundary) and `boundary_len` is
/// the length of the boundary itself — `2` for `\n\n` or `4` for `\r\n\r\n`.
///
/// `\n\n` and `\r\n\r\n` are searched independently; the earlier match wins so
/// a server that mixes line endings is still framed correctly.
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

#[cfg(test)]
mod tests {
    use super::*;
    use traits::http::SseStreamWithMeta;

    /// `ReqwestHttp::stream_sse_with_meta` must capture the real status and headers
    /// (including `retry-after`) before the SSE event stream begins, and still
    /// deliver the SSE events normally.
    ///
    /// Uses the existing axum-based in-process test harness pattern.
    #[tokio::test]
    async fn stream_sse_with_meta_captures_status_and_headers() {
        use axum::response::{IntoResponse, Response};
        use axum::routing::post;
        use axum::Router;
        use futures_util::StreamExt as _;
        use protocol::HttpMethod;
        use tokio::net::TcpListener;

        async fn handler() -> Response {
            (
                axum::http::StatusCode::OK,
                [
                    ("content-type", "text/event-stream"),
                    ("retry-after", "7"),
                    ("x-custom", "meta"),
                ],
                "event: test\ndata: hello\n\n",
            )
                .into_response()
        }

        let app = Router::new().route("/stream", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = ReqwestHttp::new();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{addr}/stream"),
            headers: vec![],
            body: None,
            timeout: None,
        };

        let SseStreamWithMeta {
            status,
            headers,
            mut stream,
        } = transport.stream_sse_with_meta(req).await.unwrap();

        assert_eq!(status, 200, "status must be captured");
        let has_retry = headers
            .iter()
            .any(|(k, v)| k == "retry-after" && v == "7");
        assert!(has_retry, "retry-after header must be captured; got: {headers:?}");
        let has_custom = headers.iter().any(|(k, _)| k == "x-custom");
        assert!(has_custom, "x-custom header must be captured");

        // Events still arrive normally.
        let event = stream.next().await.expect("at least one event").unwrap();
        assert_eq!(event.event_type.as_deref(), Some("test"));
        assert_eq!(event.data, "hello");
    }

    /// `stream_sse_with_meta` on a 429 response must return `Ok` carrying the real
    /// status, real headers (including `retry-after`), and the JSON error body as
    /// a single frame.  This lets the bridge + codec see the full picture rather
    /// than an empty-header `Err(HttpError::Status)`.
    #[tokio::test]
    async fn stream_sse_with_meta_429_returns_ok_with_real_headers_and_body() {
        use axum::response::{IntoResponse, Response};
        use axum::routing::post;
        use axum::Router;
        use futures_util::StreamExt as _;
        use protocol::HttpMethod;
        use tokio::net::TcpListener;

        async fn handler_429() -> Response {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [
                    ("content-type", "application/json"),
                    ("retry-after", "42"),
                    ("x-custom-error", "rate-limit-hit"),
                ],
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}"#,
            )
                .into_response()
        }

        let app = Router::new().route("/stream429", post(handler_429));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = ReqwestHttp::new();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{addr}/stream429"),
            headers: vec![],
            body: None,
            timeout: None,
        };

        // Must be Ok — not Err — so the bridge receives real headers.
        let meta = transport
            .stream_sse_with_meta(req)
            .await
            .expect("429 must return Ok with real headers");

        assert_eq!(meta.status, 429);
        let has_retry = meta
            .headers
            .iter()
            .any(|(k, v)| k == "retry-after" && v == "42");
        assert!(has_retry, "retry-after header must be present; got: {:?}", meta.headers);
        let has_custom = meta
            .headers
            .iter()
            .any(|(k, _)| k == "x-custom-error");
        assert!(has_custom, "x-custom-error header must be present");

        // The error body arrives as a single SSE data frame.
        let event = meta.stream.boxed().next().await.expect("one frame").unwrap();
        assert!(
            event.data.contains("rate_limit_error"),
            "error body must be in frame data; got: {}",
            event.data
        );
    }

    /// Keepalive comment lines (`:`) are silently skipped; the event is still
    /// yielded. Ported from the deleted `api-client/src/sse.rs` test suite.
    #[test]
    fn ignore_keepalive_lines() {
        let raw = ": this is a comment\nevent: a\ndata: 1\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("a"));
        assert_eq!(events[0].data, "1");
    }

    /// Multiple `data:` lines within one event are joined with `\n`.
    #[test]
    fn multi_line_data_concatenates() {
        let raw = "event: x\ndata: line1\ndata: line2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    /// Two `\n\n`-separated events in a single chunk both decode.
    #[test]
    fn parse_multiple_events() {
        let raw = "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type.as_deref(), Some("a"));
        assert_eq!(events[1].event_type.as_deref(), Some("b"));
    }

    /// A keepalive-only block (no `data:` line) is silently dropped — it does
    /// not produce a spurious event. This is the "partial drop" behaviour from
    /// the original `api-client/src/sse.rs` test suite: blocks without data
    /// are ignored regardless of whether they contain comment or event-type
    /// lines.
    #[test]
    fn partial_event_dropped() {
        // A block that contains only a comment and an event-type line but no
        // data line must not yield an event.
        let raw = ": keepalive\nevent: noop\n\nevent: real\ndata: value\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("real"));
        assert_eq!(events[0].data, "value");
    }

    /// `stream_raw_bytes_with_meta` must capture real status + headers before
    /// the byte stream begins, and deliver body bytes incrementally.
    #[tokio::test]
    async fn stream_raw_bytes_with_meta_captures_status_and_headers() {
        use axum::body::Body;
        use axum::response::{IntoResponse, Response};
        use axum::routing::post;
        use axum::Router;
        use futures_util::StreamExt as _;
        use protocol::HttpMethod;
        use tokio::net::TcpListener;

        async fn binary_handler() -> Response {
            let body = Body::from(b"\x00\x01\x02\x03".as_ref());
            (
                axum::http::StatusCode::OK,
                [
                    ("content-type", "application/octet-stream"),
                    ("x-binary-meta", "yes"),
                ],
                body,
            )
                .into_response()
        }

        let app = Router::new().route("/binary", post(binary_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = ReqwestHttp::new();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{addr}/binary"),
            headers: vec![],
            body: None,
            timeout: None,
        };

        let meta = transport
            .stream_raw_bytes_with_meta(req)
            .await
            .expect("binary stream must succeed");

        assert_eq!(meta.status, 200, "status must be captured");
        let has_meta = meta
            .headers
            .iter()
            .any(|(k, v)| k == "x-binary-meta" && v == "yes");
        assert!(has_meta, "x-binary-meta header must be captured; got: {:?}", meta.headers);

        // Collect all chunks.
        let mut all_bytes: Vec<u8> = Vec::new();
        let mut stream = meta.stream;
        while let Some(chunk) = stream.next().await {
            all_bytes.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(all_bytes, &[0x00, 0x01, 0x02, 0x03], "binary body must arrive intact");
    }

    /// `stream_raw_bytes_with_meta` on a ≥400 response must return `Ok` with
    /// real status, real headers, and body as a single chunk — mirroring the
    /// `stream_sse_with_meta` error-arm contract.
    #[tokio::test]
    async fn stream_raw_bytes_with_meta_error_returns_ok_with_headers() {
        use axum::response::{IntoResponse, Response};
        use axum::routing::post;
        use axum::Router;
        use futures_util::StreamExt as _;
        use protocol::HttpMethod;
        use tokio::net::TcpListener;

        async fn error_handler() -> Response {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [
                    ("content-type", "application/json"),
                    ("retry-after", "30"),
                ],
                r#"{"error":"rate_limit"}"#,
            )
                .into_response()
        }

        let app = Router::new().route("/err", post(error_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = ReqwestHttp::new();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{addr}/err"),
            headers: vec![],
            body: None,
            timeout: None,
        };

        let meta = transport
            .stream_raw_bytes_with_meta(req)
            .await
            .expect("429 must return Ok — not Err — so bridge gets real headers");
        assert_eq!(meta.status, 429);
        let has_retry = meta
            .headers
            .iter()
            .any(|(k, v)| k == "retry-after" && v == "30");
        assert!(has_retry, "retry-after must be present; got: {:?}", meta.headers);

        // Body arrives as a single chunk.
        let chunk = meta.stream.boxed().next().await.expect("one chunk").unwrap();
        assert!(
            chunk.windows(b"rate_limit".len()).any(|w| w == b"rate_limit"),
            "body chunk must contain error; got: {chunk:?}"
        );
    }
}
