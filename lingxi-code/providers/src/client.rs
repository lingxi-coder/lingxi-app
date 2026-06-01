//! `GenericClient` drives a `WireCodec` over a `traits::HttpTransport`. This
//! is the harness the OpenAI/Gemini codecs plug into (P3/P4).

use crate::authenticator::Authenticator;
use crate::capabilities::Capabilities;
use crate::codec::{SseDecoder, WireCodec};
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::{self, BoxStream, StreamExt};
use std::collections::VecDeque;
use std::sync::Arc;
use traits::http::SseStream;
use traits::{HttpError, HttpTransport};

/// A codec-driven provider: encode → transport → decode.
pub struct GenericClient<C: WireCodec> {
    codec: C,
    authenticator: Arc<dyn Authenticator>,
    transport: Arc<dyn HttpTransport>,
    id: ProviderId,
    capabilities: Capabilities,
}

impl<C: WireCodec> GenericClient<C> {
    /// Construct a client from a codec, authenticator, transport, id, and caps.
    #[must_use]
    pub fn new(
        codec: C,
        authenticator: Arc<dyn Authenticator>,
        transport: Arc<dyn HttpTransport>,
        id: ProviderId,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            codec,
            authenticator,
            transport,
            id,
            capabilities,
        }
    }
}

/// State threaded through the streaming pump.
struct StreamPump {
    wire: SseStream,
    decoder: Box<dyn SseDecoder>,
    queue: VecDeque<Result<StreamEvent, ApiError>>,
    done: bool,
}

/// Flatten a transport SSE stream through a stateful decoder into canonical
/// events. Owns `wire` + `decoder`, so the result is `'static`.
fn pump_stream(
    wire: SseStream,
    decoder: Box<dyn SseDecoder>,
) -> BoxStream<'static, Result<StreamEvent, ApiError>> {
    let init = StreamPump {
        wire,
        decoder,
        queue: VecDeque::new(),
        done: false,
    };
    stream::unfold(init, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                return Some((item, st));
            }
            if st.done {
                return None;
            }
            match st.wire.next().await {
                Some(Ok(sse)) => {
                    for ev in st.decoder.push(&sse.data) {
                        st.queue.push_back(Ok(ev));
                    }
                }
                Some(Err(e)) => {
                    st.done = true;
                    return Some((Err(ApiError::Http(e)), st));
                }
                None => {
                    for ev in st.decoder.finish() {
                        st.queue.push_back(Ok(ev));
                    }
                    st.done = true;
                }
            }
        }
    })
    .boxed()
}

#[async_trait]
impl<C: WireCodec + 'static> LlmProvider for GenericClient<C> {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut http = self
            .codec
            .encode_request(&req)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        self.authenticator.authorize(&mut http).await?;
        let resp = self.transport.request(http).await.map_err(ApiError::Http)?;
        self.codec.decode_response(resp.status, &resp.body)
    }

    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut req = req;
        req.stream = true;
        let mut http = self
            .codec
            .encode_request(&req)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        self.authenticator.authorize(&mut http).await?;
        let wire = self
            .transport
            .stream_sse(http)
            .await
            .map_err(ApiError::Http)?;
        let decoder = self.codec.new_stream_decoder();
        Ok(pump_stream(wire, decoder))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CodecError;
    use crate::testutil::MockTransport;
    use api_client::types::{ContentBlockApi, ContentDelta, UsageApi};
    use futures::StreamExt;
    use protocol::{HttpMethod, HttpRequest};
    use std::sync::Arc;

    struct MockCodec;

    impl WireCodec for MockCodec {
        fn encode_request(&self, _req: &CanonicalRequest) -> Result<HttpRequest, CodecError> {
            Ok(HttpRequest {
                method: HttpMethod::Post,
                url: "https://mock.local/v1/chat".to_string(),
                headers: Vec::new(),
                body: Some("{}".to_string()),
                timeout: None,
            })
        }

        fn decode_response(&self, _status: u16, body: &str) -> Result<MessageResponse, ApiError> {
            Ok(MessageResponse {
                id: "m".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlockApi::Text {
                    text: body.to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }

        fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
            Box::new(MockDecoder)
        }
    }

    struct MockDecoder;

    impl SseDecoder for MockDecoder {
        fn push(&mut self, data: &str) -> Vec<StreamEvent> {
            vec![StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta {
                    text: data.to_string(),
                },
            }]
        }

        fn finish(&mut self) -> Vec<StreamEvent> {
            vec![StreamEvent::MessageStop]
        }
    }

    fn client(transport: MockTransport) -> GenericClient<MockCodec> {
        GenericClient::new(
            MockCodec,
            Arc::new(crate::authenticator::StaticAuth::new(crate::auth::Auth::None)),
            Arc::new(transport),
            cost::ProviderId::OpenAI,
            Capabilities::anthropic(),
        )
    }

    fn client_with_auth(
        transport: MockTransport,
        authenticator: Arc<dyn crate::authenticator::Authenticator>,
    ) -> GenericClient<MockCodec> {
        GenericClient::new(
            MockCodec,
            authenticator,
            Arc::new(transport),
            cost::ProviderId::OpenAI,
            Capabilities::anthropic(),
        )
    }

    #[tokio::test]
    async fn complete_runs_encode_request_decode() {
        let c = client(MockTransport::responding(200, "PONG"));
        let resp = c
            .complete(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("ok");
        match &resp.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "PONG"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_flattens_decoder_events_then_finish() {
        let c = client(MockTransport::streaming(vec!["a", "b"]));
        let s = c
            .stream(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("stream");
        let events: Vec<_> = s.collect().await;
        assert_eq!(events.len(), 3, "2 deltas + MessageStop");
        assert!(matches!(
            events[0],
            Ok(StreamEvent::ContentBlockDelta { .. })
        ));
        assert!(matches!(events[2], Ok(StreamEvent::MessageStop)));
    }

    #[tokio::test]
    async fn stream_empty_yields_only_finish() {
        let c = client(MockTransport::streaming(vec![]));
        let s = c
            .stream(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("stream");
        let events: Vec<_> = s.collect().await;
        assert_eq!(events.len(), 1, "only the finish() MessageStop");
        assert!(matches!(events[0], Ok(StreamEvent::MessageStop)));
    }

    #[tokio::test]
    async fn stream_transport_error_is_terminal() {
        let c = client(MockTransport::erroring("boom"));
        let s = c
            .stream(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("stream");
        let events: Vec<_> = s.collect().await;
        assert_eq!(
            events.len(),
            1,
            "the error item, then terminate (no finish, no re-poll)"
        );
        assert!(matches!(events[0], Err(ApiError::Http(_))));
    }

    /// Regression guard: `GenericClient::complete` must call
    /// `authenticator.authorize` between encode and transport so the
    /// `Authorization` header is visible to the transport.
    #[tokio::test]
    async fn complete_attaches_auth_header_via_authenticator() {
        let transport = MockTransport::responding(200, "PONG");
        let handle = transport.captured_handle();
        let auth = Arc::new(crate::authenticator::StaticAuth::new(
            crate::auth::Auth::Bearer("sk-x".to_string()),
        ));
        let c = client_with_auth(transport, auth);
        c.complete(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("ok");
        let captured = handle.lock().unwrap();
        let req = captured.as_ref().expect("transport should have received a request");
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer sk-x"),
            "authorization header not found in captured request; headers: {:?}",
            req.headers
        );
    }

    /// Regression guard: `GenericClient::stream` must call
    /// `authenticator.authorize` between encode and transport so the
    /// `Authorization` header is visible to the transport.
    #[tokio::test]
    async fn stream_attaches_auth_header_via_authenticator() {
        let transport = MockTransport::streaming(vec!["hello"]);
        let handle = transport.captured_handle();
        let auth = Arc::new(crate::authenticator::StaticAuth::new(
            crate::auth::Auth::Bearer("sk-x".to_string()),
        ));
        let c = client_with_auth(transport, auth);
        let s = c
            .stream(CanonicalRequest::new("gpt-4o"))
            .await
            .expect("stream");
        // Drain the stream so the transport call is fully exercised.
        let _events: Vec<_> = s.collect().await;
        let captured = handle.lock().unwrap();
        let req = captured.as_ref().expect("transport should have received a request");
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer sk-x"),
            "authorization header not found in captured request; headers: {:?}",
            req.headers
        );
    }
}
