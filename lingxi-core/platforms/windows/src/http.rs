//! `reqwest`-backed [`HttpTransport`] for Windows hosts.
//!
//! Implements blocking-style request/response via `reqwest::Client`. SSE
//! streaming is deferred to a follow-up — `stream_sse` returns
//! [`HttpError::InvalidRequest`] for now.

use async_trait::async_trait;
use lingxi_protocol::{HttpRequest, HttpResponse};
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

    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        // TODO(M2-followup): wire SSE streaming using `reqwest::Response::bytes_stream()`.
        Err(HttpError::InvalidRequest(
            "windows: stream_sse not yet wired".into(),
        ))
    }
}
