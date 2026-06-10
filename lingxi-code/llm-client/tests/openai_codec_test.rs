use llm_client::{
    ContentBlock, ContentDelta, LlmEvent, LlmRequest, OpenAiChatCodec, ProviderResponse,
    RawStreamFrame, ToolDeclaration, WireCodec,
};

#[test]
fn encode_request_shape_is_openai_chat_completions() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.system = Some("sys".to_string());
    request.tools = vec![ToolDeclaration {
        name: "Read".to_string(),
        description: "d".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    }];

    let provider_request = codec.encode_request(&request).unwrap();

    assert!(provider_request.url.ends_with("/chat/completions"));
    assert_eq!(provider_request.body_json["messages"][0]["role"], "system");
    assert_eq!(provider_request.body_json["tools"][0]["type"], "function");
}

#[test]
fn decode_text_and_tool_responses() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let text = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-x",
        "model":"gpt-4o-2024-08-06",
        "choices":[{"index":0,"message":{"role":"assistant","content":"hi there"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":9,"completion_tokens":3}
    }));
    let decoded = codec.decode_response(text).unwrap();
    assert_eq!(decoded.model, "gpt-4o-2024-08-06");
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
    assert!(matches!(decoded.content[0], ContentBlock::Text { .. }));

    let tool = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-y",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]
    }));
    let decoded = codec.decode_response(tool).unwrap();
    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref name, .. } if name == "Bash"));
}

#[test]
fn encode_tool_result_as_tool_message_not_tool_call() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.messages.push(llm_client::Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_call_id: "call_1".to_string(),
            output: serde_json::json!("done"),
        }],
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["messages"][0]["role"], "tool");
    assert_eq!(provider_request.body_json["messages"][0]["tool_call_id"], "call_1");
    assert_eq!(provider_request.body_json["messages"][0]["content"], "done");
    assert!(provider_request.body_json["messages"][0].get("tool_calls").is_none());
}

#[test]
fn encode_multiple_tool_results_preserves_each_as_tool_message() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.messages.push(llm_client::Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::ToolResult {
                tool_call_id: "call_1".to_string(),
                output: serde_json::json!("first"),
            },
            ContentBlock::ToolResult {
                tool_call_id: "call_2".to_string(),
                output: serde_json::json!("second"),
            },
        ],
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["messages"][0]["role"], "tool");
    assert_eq!(provider_request.body_json["messages"][0]["tool_call_id"], "call_1");
    assert_eq!(provider_request.body_json["messages"][0]["content"], "first");
    assert_eq!(provider_request.body_json["messages"][1]["role"], "tool");
    assert_eq!(provider_request.body_json["messages"][1]["tool_call_id"], "call_2");
    assert_eq!(provider_request.body_json["messages"][1]["content"], "second");
    assert_eq!(provider_request.body_json["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn decode_rejects_invalid_tool_call_arguments() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-z",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"not-json"}}]},"finish_reason":"tool_calls"}]
    }));

    let err = codec.decode_response(response).unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn decode_rejects_missing_tool_call_arguments() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-w",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash"}}]},"finish_reason":"tool_calls"}]
    }));

    let err = codec.decode_response(response).unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn sse_stream_reassembles_text_then_tool_call() {
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"On it. "}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_z","type":"function","function":{"name":"Bash","arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
    let mut decoder = OpenAiChatCodec::new("https://api.openai.com/v1").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    assert!(matches!(events.first(), Some(LlmEvent::MessageStart { .. })));
    let args: String = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ContentBlockDelta {
                delta: ContentDelta::InputJsonDelta { partial_json },
                ..
            } => Some(partial_json.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(args, "{\"command\":\"ls\"}");
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}
