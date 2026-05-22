//! HTTP and SSE transport DTOs (no I/O — pure data).

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// HTTP request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// HTTP `GET`.
    Get,
    /// HTTP `POST`.
    Post,
    /// HTTP `PUT`.
    Put,
    /// HTTP `PATCH`.
    Patch,
    /// HTTP `DELETE`.
    Delete,
    /// HTTP `HEAD`.
    Head,
    /// HTTP `OPTIONS`.
    Options,
}

/// An HTTP request, expressed as pure data (no I/O).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// Request method.
    pub method: HttpMethod,
    /// Fully-qualified target URL.
    pub url: String,
    /// Request headers as ordered (name, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Optional request body (typically UTF-8 JSON).
    pub body: Option<String>,
    /// Optional overall request timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
}

/// An HTTP response, expressed as pure data (no I/O).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// Numeric HTTP status code.
    pub status: u16,
    /// Response headers as ordered (name, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Response body (typically UTF-8 JSON or SSE text).
    pub body: String,
}

/// A single Server-Sent Events frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    /// Optional event type (the `event:` field).
    pub event_type: Option<String>,
    /// Event payload (the `data:` field).
    pub data: String,
    /// Optional event ID (the `id:` field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_roundtrip() {
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: "https://api.anthropic.com/v1/messages".into(),
            headers: vec![("authorization".into(), "Bearer xyz".into())],
            body: Some(r#"{"model":"claude-opus-4-6"}"#.into()),
            timeout: Some(Duration::from_secs(30)),
        };
        let s = serde_json::to_string(&req).unwrap();
        let req2: HttpRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(req.method, req2.method);
        assert_eq!(req.url, req2.url);
    }

    #[test]
    fn sse_event_default_type_omitted() {
        let e = SseEvent { event_type: None, data: "{}".into(), id: None };
        let s = serde_json::to_string(&e).unwrap();
        assert!(!s.contains("id"));
    }
}
