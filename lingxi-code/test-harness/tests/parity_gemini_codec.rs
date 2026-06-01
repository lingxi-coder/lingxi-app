//! Parity fixtures for the Gemini codec — exercise encode/decode/SSE against
//! representative wire samples so the shapes stay locked.

use providers::gemini::decode::decode_generate_response;
use providers::gemini::stream::GeminiSseDecoder;
use providers::{Auth, CanonicalRequest, GeminiCodec, SseDecoder, WireCodec};

#[test]
fn encode_request_shape_is_gemini_generate_content() {
    let codec = GeminiCodec::new(None);
    let mut req = CanonicalRequest::new("gemini-2.0-flash");
    req.system = Some("sys".to_string());
    req.tools =
        vec![serde_json::json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
    let http = codec
        .encode_request(
            &req,
            &Auth::Header {
                name: "x-goog-api-key".to_string(),
                value: "k".to_string(),
            },
        )
        .unwrap();
    assert!(http.url.contains(":generateContent"));
    let body: serde_json::Value = serde_json::from_str(http.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "Read");
}

#[test]
fn decode_real_world_text_and_function_call() {
    let text = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"text":"hi there"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3}}"#;
    let r = decode_generate_response(200, text).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));

    let tool = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]}"#;
    let r = decode_generate_response(200, tool).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn sse_stream_reassembles_text_then_function_call() {
    use api_client::types::{ContentBlockApi, StreamEvent};
    let frames = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"On it."}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
    ];
    let mut d = GeminiSseDecoder::new();
    let mut events = Vec::new();
    for f in frames {
        events.extend(d.push(f));
    }
    events.extend(d.finish());

    assert!(matches!(
        events.first(),
        Some(StreamEvent::MessageStart { .. })
    ));
    assert!(events.iter().any(|e| matches!(
        e, StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::MessageStop))
            .count(),
        1
    );
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
}
