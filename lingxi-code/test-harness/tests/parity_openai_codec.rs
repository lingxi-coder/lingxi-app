//! Parity fixtures for the OpenAI codec — exercise the pure encode/decode/SSE
//! functions against representative wire samples so the shapes stay locked.

use providers::openai::decode::decode_chat_response;
use providers::openai::stream::OpenAiSseDecoder;
use providers::{CanonicalRequest, OpenAiCodec, SseDecoder, WireCodec};

#[test]
fn encode_request_shape_is_openai_chat_completions() {
    let codec = OpenAiCodec::new(None);
    let mut req = CanonicalRequest::new("gpt-4o");
    req.system = Some("sys".to_string());
    req.tools =
        vec![serde_json::json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
    let http = codec.encode_request(&req).unwrap();
    assert!(http.url.ends_with("/chat/completions"));
    let body: serde_json::Value = serde_json::from_str(http.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["tools"][0]["type"], "function");
}

#[test]
fn decode_real_world_text_and_tool_responses() {
    let text = r#"{"id":"chatcmpl-x","model":"gpt-4o-2024-08-06","choices":[{"index":0,"message":{"role":"assistant","content":"hi there"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#;
    let r = decode_chat_response(200, text).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));

    let tool = r#"{"id":"chatcmpl-y","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    let r = decode_chat_response(200, tool).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn sse_stream_reassembles_text_then_tool_call() {
    use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"On it. "}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_z","type":"function","function":{"name":"Bash","arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
    let mut d = OpenAiSseDecoder::new();
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
        e,
        StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
    )));
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::InputJsonDelta { partial_json },
                ..
            } => Some(partial_json.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(args, "{\"command\":\"ls\"}");
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
}
