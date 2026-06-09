use llm_client::{AnthropicMessagesCodec, ContentBlock, ContentDelta, LlmEvent, LlmRequest, Message, ProviderResponse, ResponseFormat, ToolChoice, ToolDeclaration, WireCodec};

#[test]
fn encode_request_shape_is_anthropic_messages() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.system = Some("sys".to_string());
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text { text: "hello".to_string() }],
    });
    request.tools.push(ToolDeclaration {
        name: "Read".to_string(),
        description: "Read a file".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.method, "POST");
    assert!(provider_request.url.ends_with("/v1/messages"));
    assert_eq!(provider_request.headers["anthropic-version"], "2023-06-01");
    assert_eq!(provider_request.body_json["model"], "claude-sonnet-4-20250514");
    assert_eq!(provider_request.body_json["system"], "sys");
    assert_eq!(provider_request.body_json["messages"][0]["role"], "user");
    assert_eq!(provider_request.body_json["tools"][0]["input_schema"]["type"], "object");
}

#[test]
fn encode_request_maps_image_and_tool_result_blocks() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3],
            },
            ContentBlock::ToolResult {
                tool_call_id: "tool-1".to_string(),
                output: serde_json::json!({"ok": true}),
            },
        ],
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["messages"][0]["content"][0]["type"], "image");
    assert_eq!(provider_request.body_json["messages"][0]["content"][0]["source"]["type"], "base64");
    assert_eq!(provider_request.body_json["messages"][0]["content"][0]["source"]["media_type"], "image/png");
    assert_eq!(provider_request.body_json["messages"][0]["content"][0]["source"]["data"], "AQID");
    assert_eq!(provider_request.body_json["messages"][0]["content"][1]["type"], "tool_result");
    assert_eq!(provider_request.body_json["messages"][0]["content"][1]["tool_use_id"], "tool-1");
    assert_eq!(provider_request.body_json["messages"][0]["content"][1]["content"], "{\"ok\":true}");
}

#[test]
fn encode_request_maps_supported_tool_choice_variants() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");

    let cases = [
        (ToolChoice::Auto, "auto", None::<&str>),
        (ToolChoice::None, "none", None::<&str>),
        (ToolChoice::Required, "any", None::<&str>),
        (ToolChoice::Tool { name: "Read".to_string() }, "tool", Some("Read")),
    ];

    for (choice, expected_type, expected_name) in cases {
        let mut request = LlmRequest::new("claude-sonnet-4-20250514");
        request.tool_choice = Some(choice);

        let provider_request = codec.encode_request(&request).unwrap();

        assert_eq!(provider_request.body_json["tool_choice"]["type"], expected_type);
        if let Some(name) = expected_name {
            assert_eq!(provider_request.body_json["tool_choice"]["name"], name);
        }
    }
}

#[test]
fn encode_request_rejects_response_format_explicitly() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");

    let mut response_format_request = LlmRequest::new("claude-sonnet-4-20250514");
    response_format_request.response_format = Some(ResponseFormat::JsonObject);

    let response_format_err = codec.encode_request(&response_format_request).unwrap_err();
    assert!(matches!(response_format_err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn encode_request_pins_default_max_tokens_to_4096() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let request = LlmRequest::new("claude-sonnet-4-20250514");

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["max_tokens"], 4096);
}

#[test]
fn decode_text_response_maps_usage_and_stop_reason() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id": "msg_1",
        "model": "claude-sonnet-4-20250514",
        "content": [{"type":"text","text":"hi"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 9, "output_tokens": 3}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert_eq!(decoded.id, "msg_1");
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
}

#[test]
fn decode_tool_use_response_maps_tool_call_block() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id": "msg_2",
        "model": "claude-sonnet-4-20250514",
        "content": [{"type":"tool_use","id":"tool_1","name":"Read","input":{"path":"foo.txt"}}],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref id, ref name, .. } if id == "tool_1" && name == "Read"));
}

#[test]
fn stream_decoder_maps_text_delta_and_rejects_garbage() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    let events = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#.to_vec(),
    )).unwrap();

    assert!(matches!(
        &events[0],
        LlmEvent::ContentBlockDelta { index: 0, delta: ContentDelta::TextDelta { text } } if text == "hi"
    ));
    assert!(decoder.decode_frame(llm_client::RawStreamFrame::new(b"not json".to_vec())).is_err());
}
