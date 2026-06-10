use llm_client::{
    validate_capabilities, Capabilities, ContentBlock, LlmEvent, LlmRequest, LlmResponse,
    PreparedBody, Protocol, RawResponse, RawStreamFrame, StreamDecoder, Usage,
};
use serde_json::Value;

#[derive(Debug)]
struct DummyProtocol;

impl Protocol for DummyProtocol {
    fn encode(&self, request: &LlmRequest) -> Result<PreparedBody, llm_client::LlmError> {
        Ok(PreparedBody::Json(serde_json::json!({
            "model": request.model,
            "messages": request.messages.len()
        })))
    }

    fn decode_response(&self, response: RawResponse) -> Result<LlmResponse, llm_client::LlmError> {
        Ok(LlmResponse {
            id: response.request_id.unwrap_or_else(|| "response-id".to_string()),
            model: "test-model".to_string(),
            content: vec![ContentBlock::Text {
                text: String::from_utf8(response.body).expect("utf8"),
            }],
            stop_reason: None,
            usage: Usage::default(),
            cost: None,
            provider_metadata: Value::default(),
        })
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(DummyStreamDecoder)
    }
}

#[derive(Debug)]
struct DummyStreamDecoder;

impl StreamDecoder for DummyStreamDecoder {
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, llm_client::LlmError> {
        Ok(vec![LlmEvent::TextDelta {
            text: String::from_utf8(frame.bytes).expect("utf8"),
        }])
    }
}

#[test]
fn protocol_encodes_request_into_prepared_body() {
    let request = LlmRequest::new("test-model").with_user_text("hello");

    let body = DummyProtocol.encode(&request).expect("body");

    assert_eq!(
        body,
        PreparedBody::Json(serde_json::json!({"model":"test-model","messages":1}))
    );
}

#[test]
fn stream_decoder_maps_raw_frames_to_ordered_events() {
    let mut decoder = DummyProtocol.stream_decoder();

    let events = decoder
        .decode_frame(RawStreamFrame::new(b"hello".to_vec()))
        .expect("events");

    assert_eq!(events, vec![LlmEvent::TextDelta { text: "hello".to_string() }]);
}

#[test]
fn unsupported_capabilities_fail_before_transport() {
    let request = LlmRequest::new("text-only")
        .with_user_text("describe")
        .with_image("image/png", vec![1, 2, 3]);
    let capabilities = Capabilities {
        streaming: true,
        tools: true,
        vision: false,
        documents: false,
        reasoning: false,
        structured_output: false,
    };

    let error = validate_capabilities(&request, capabilities).expect_err("vision should fail");

    assert!(matches!(
        error,
        llm_client::LlmError::UnsupportedCapability { capability } if capability == "vision"
    ));
}
