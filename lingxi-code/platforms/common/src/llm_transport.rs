//! Bridge from `traits::HttpTransport` to `llm_client::Transport`.
//!
//! One generic adapter serves every platform HTTP implementation
//! (`ReqwestHttp` on desktop, native transports on mobile).

use std::collections::BTreeMap;

use llm_client::{
    BoxFuture, FrameStream, LlmError, ProviderRequest, ProviderResponse, RawStreamFrame,
    StreamFraming, StreamingResponse,
};
use protocol::{HttpMethod, HttpRequest, HttpResponse};
use traits::http::{RawByteStream, RawByteStreamWithMeta, SseStream, SseStreamWithMeta};
use traits::{HttpError, HttpTransport};

/// Adapter exposing a [`traits::HttpTransport`] as an [`llm_client::Transport`].
pub struct LlmTransportBridge<T> {
    inner: T,
}

impl<T> LlmTransportBridge<T> {
    /// Wrap a platform HTTP transport.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Access the wrapped transport (used by hosts and tests).
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

fn to_http_request(request: &ProviderRequest) -> Result<HttpRequest, LlmError> {
    let method = match request.method.as_str() {
        "POST" => HttpMethod::Post,
        "GET" => HttpMethod::Get,
        other => {
            return Err(LlmError::InvalidRequest {
                message: format!("unsupported provider request method: {other}"),
            })
        }
    };
    Ok(HttpRequest {
        method,
        url: request.url.clone(),
        headers: request
            .headers
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        body: Some(request.body_json.to_string()),
        // ProviderRequest carries no timeout field yet; deadline enforcement
        // lives above this seam in the retry layer.
        timeout: None,
    })
}

fn lowercase_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn request_id(headers: &BTreeMap<String, String>) -> Option<String> {
    headers
        .get("request-id")
        .or_else(|| headers.get("x-request-id"))
        .cloned()
}

fn to_provider_response(response: &HttpResponse) -> ProviderResponse {
    let headers = lowercase_headers(&response.headers);
    let body_json =
        serde_json::from_str(&response.body).unwrap_or(serde_json::Value::Null);
    ProviderResponse {
        status: response.status,
        request_id: request_id(&headers),
        headers,
        body_json,
    }
}

fn status_error_response(status: u16, body: &str) -> ProviderResponse {
    ProviderResponse {
        status,
        headers: BTreeMap::new(),
        body_json: serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
        request_id: None,
    }
}

fn map_http_error(error: &HttpError) -> LlmError {
    LlmError::Transport {
        message: error.to_string(),
    }
}

impl<T: HttpTransport> llm_client::Transport for LlmTransportBridge<T> {
    fn execute<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        Box::pin(async move {
            let http_request = to_http_request(request)?;
            match self.inner.request(http_request).await {
                Ok(response) => Ok(to_provider_response(&response)),
                // Some transports surface non-2xx as an error variant; keep
                // it data so llm-client's taxonomy does the classification.
                Err(HttpError::Status { status, body }) => Ok(status_error_response(status, &body)),
                Err(error) => Err(map_http_error(&error)),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
        Box::pin(async move {
            match request.stream_framing {
                StreamFraming::AwsEventStream => {
                    // Raw binary path: pass byte chunks directly to the codec's
                    // StreamDecoder (no SSE splitting). Used by Bedrock.
                    let http_request = to_http_request(request)?;
                    match self.inner.stream_raw_bytes_with_meta(http_request).await {
                        Ok(RawByteStreamWithMeta {
                            status,
                            headers,
                            stream,
                        }) => Ok(StreamingResponse {
                            status,
                            headers: lowercase_headers(&headers),
                            frames: Box::new(RawFrames { stream }),
                        }),
                        // Default transport impl (no override) may surface Err for ≥400.
                        Err(HttpError::Status { status, body }) => Ok(StreamingResponse {
                            status,
                            headers: BTreeMap::new(),
                            frames: Box::new(BodyFrame {
                                body: Some(body.into_bytes()),
                            }),
                        }),
                        Err(error) => Err(map_http_error(&error)),
                    }
                }
                StreamFraming::Sse => {
                    let http_request = to_http_request(request)?;
                    match self.inner.stream_sse_with_meta(http_request).await {
                        Ok(SseStreamWithMeta {
                            status,
                            headers,
                            stream,
                        }) => Ok(StreamingResponse {
                            status,
                            // Vec<(String,String)> → BTreeMap<String,String>; names are
                            // already lowercased by the SseStreamWithMeta contract.
                            headers: lowercase_headers(&headers),
                            frames: Box::new(SseFrames { stream }),
                        }),
                        // Error path: `reqwest`'s error arm has no headers at this
                        // point (the response was consumed into the Status variant
                        // before headers could be captured), so headers remain empty.
                        Err(HttpError::Status { status, body }) => Ok(StreamingResponse {
                            status,
                            headers: BTreeMap::new(),
                            frames: Box::new(BodyFrame {
                                body: Some(body.into_bytes()),
                            }),
                        }),
                        Err(error) => Err(map_http_error(&error)),
                    }
                }
            }
        })
    }
}

/// Raw byte stream wrapped as a [`FrameStream`] for the AWS event-stream path.
///
/// Each byte chunk from the transport is surfaced as one [`RawStreamFrame`].
/// The codec's [`StreamDecoder`] is responsible for reassembling binary frames
/// from the raw chunks (typically by feeding them to an [`EventStreamSplitter`]).
struct RawFrames {
    stream: RawByteStream,
}

impl FrameStream for RawFrames {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        Box::pin(async move {
            use futures_util::StreamExt;
            match self.stream.next().await {
                Some(Ok(bytes)) => Ok(Some(RawStreamFrame::new(bytes))),
                Some(Err(error)) => Err(map_http_error(&error)),
                None => Ok(None),
            }
        })
    }
}

struct SseFrames {
    stream: SseStream,
}

impl FrameStream for SseFrames {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        Box::pin(async move {
            use futures_util::StreamExt;
            match self.stream.next().await {
                Some(Ok(event)) => Ok(Some(RawStreamFrame::new(event.data.into_bytes()))),
                Some(Err(error)) => Err(map_http_error(&error)),
                None => Ok(None),
            }
        })
    }
}

/// Error-status body delivered as a single frame for llm-client to drain.
struct BodyFrame {
    body: Option<Vec<u8>>,
}

impl FrameStream for BodyFrame {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        let body = self.body.take();
        Box::pin(async move { Ok(body.map(RawStreamFrame::new)) })
    }
}
