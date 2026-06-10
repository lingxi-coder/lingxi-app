use llm_client::{AnthropicMessagesCodec, ContentBlock, ContentDelta, LlmEvent, LlmRequest, Message, ProviderResponse, ResponseFormat, ToolChoice, ToolDeclaration, WireCodec};

#[test]
fn encode_request_shape_is_anthropic_messages() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.system = vec![llm_client::SystemBlock::text("sys")];
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text { text: "hello".to_string(), cache_control: None }],
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
    assert_eq!(provider_request.body_json["system"][0]["text"], "sys");
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
                is_error: false,
                cache_control: None,
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
fn encode_omits_empty_tools_array() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");

    let bare = codec.encode_request(&LlmRequest::new("claude-sonnet-4-20250514")).unwrap();
    assert!(bare.body_json.get("tools").is_none());

    let mut with_tools = LlmRequest::new("claude-sonnet-4-20250514");
    with_tools.tools.push(ToolDeclaration {
        name: "Read".to_string(),
        description: "d".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    });
    let encoded = codec.encode_request(&with_tools).unwrap();
    assert_eq!(encoded.body_json["tools"][0]["name"], "Read");
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
    assert_eq!(decoded.stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn encode_thinking_round_trip_requires_signature() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");

    let mut signed = LlmRequest::new("claude-sonnet-4-20250514");
    signed.messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Reasoning {
            text: "pondering".to_string(),
            signature: Some("sig_abc".to_string()),
        }],
    });
    let provider_request = codec.encode_request(&signed).unwrap();
    let block = &provider_request.body_json["messages"][0]["content"][0];
    assert_eq!(block["type"], "thinking");
    assert_eq!(block["thinking"], "pondering");
    assert_eq!(block["signature"], "sig_abc");

    let mut unsigned = LlmRequest::new("claude-sonnet-4-20250514");
    unsigned.messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Reasoning {
            text: "pondering".to_string(),
            signature: None,
        }],
    });
    assert!(matches!(
        codec.encode_request(&unsigned).unwrap_err(),
        llm_client::LlmError::InvalidRequest { .. }
    ));
}

#[test]
fn redacted_thinking_round_trips() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id": "msg_5",
        "model": "claude-sonnet-4-20250514",
        "content": [{"type":"redacted_thinking","data":"opaque-bytes"}],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }));

    let decoded = codec.decode_response(response).unwrap();
    assert!(matches!(
        decoded.content.as_slice(),
        [ContentBlock::RedactedThinking { data }] if data == "opaque-bytes"
    ));

    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.messages.push(Message {
        role: "assistant".to_string(),
        content: decoded.content,
    });
    let provider_request = codec.encode_request(&request).unwrap();
    let block = &provider_request.body_json["messages"][0]["content"][0];
    assert_eq!(block["type"], "redacted_thinking");
    assert_eq!(block["data"], "opaque-bytes");
}

#[test]
fn stream_signature_delta_maps_to_signature_delta() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    let events = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"sig_abc"}}"#.to_vec(),
    )).unwrap();

    assert!(matches!(
        &events[0],
        LlmEvent::ContentBlockDelta {
            index: 1,
            delta: ContentDelta::SignatureDelta { signature },
        } if signature == "sig_abc"
    ));
}

#[test]
fn encode_tool_result_error_flag() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::ToolResult {
                tool_call_id: "tool-1".to_string(),
                output: serde_json::json!("boom"),
                is_error: true,
                cache_control: None,
            },
            ContentBlock::ToolResult {
                tool_call_id: "tool-2".to_string(),
                output: serde_json::json!("fine"),
                is_error: false,
                cache_control: None,
            },
        ],
    });

    let provider_request = codec.encode_request(&request).unwrap();
    let content = &provider_request.body_json["messages"][0]["content"];

    assert_eq!(content[0]["is_error"], true);
    assert!(content[1].get("is_error").is_none());
}

#[test]
fn encode_sampling_controls() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.max_tokens = Some(1024);
    request.temperature = Some(0.5);
    request.top_p = Some(0.9);
    request.stop_sequences = vec!["END".to_string()];

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["max_tokens"], 1024);
    assert_eq!(provider_request.body_json["temperature"], 0.5);
    assert_eq!(provider_request.body_json["top_p"], 0.9);
    assert_eq!(provider_request.body_json["stop_sequences"], serde_json::json!(["END"]));

    let bare = codec.encode_request(&LlmRequest::new("claude-sonnet-4-20250514")).unwrap();
    assert!(bare.body_json.get("temperature").is_none());
    assert!(bare.body_json.get("top_p").is_none());
    assert!(bare.body_json.get("stop_sequences").is_none());
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

#[test]
fn stream_decoder_covers_required_event_paths_and_reasoning_blocks() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    let message_start = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"message_start","message":{"id":"msg_3","model":"claude-sonnet-4-20250514","content":[],"usage":{"input_tokens":2,"output_tokens":0}}}"#.to_vec(),
    )).unwrap();
    assert!(matches!(&message_start[0], LlmEvent::MessageStart { .. }));

    let text_start = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_vec(),
    )).unwrap();
    assert!(matches!(&text_start[0], LlmEvent::ContentBlockStart { .. }));

    let reasoning_start = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":""}}"#.to_vec(),
    ));
    assert!(reasoning_start.is_ok());
    let reasoning_start = reasoning_start.unwrap();
    assert!(matches!(
        &reasoning_start[0],
        LlmEvent::ContentBlockStart { content_block: ContentBlock::Reasoning { .. }, .. }
    ));

    let reasoning_delta = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"ponder"}}"#.to_vec(),
    )).unwrap();
    assert!(matches!(
        &reasoning_delta[0],
        LlmEvent::ContentBlockDelta { delta: ContentDelta::ThinkingDelta { thinking }, .. } if thinking == "ponder"
    ));

    let stop = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_stop","index":1}"#.to_vec(),
    )).unwrap();
    assert!(matches!(&stop[0], LlmEvent::ContentBlockStop { index: 1 }));

    let terminal = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":2,"output_tokens":1}}"#.to_vec(),
    )).unwrap();
    assert!(matches!(&terminal[0], LlmEvent::MessageDelta { .. }));

    let stop_event = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"message_stop"}"#.to_vec(),
    )).unwrap();
    assert!(matches!(&stop_event[0], LlmEvent::MessageStop));

    assert!(decoder.decode_frame(llm_client::RawStreamFrame::new(br#"{"type":"ping"}"#.to_vec())).unwrap().is_empty());
    assert!(matches!(
        decoder.decode_frame(llm_client::RawStreamFrame::new(br#"{"type":"error"}"#.to_vec())),
        Err(llm_client::LlmError::ProviderInternal)
    ));
}

#[test]
fn decode_skips_unknown_content_block_types() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id": "msg_4",
        "model": "claude-sonnet-4-20250514",
        "content": [
            {"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"weather"}},
            {"type":"text","text":"hi"}
        ],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert!(matches!(decoded.content.as_slice(), [ContentBlock::Text { text, .. }] if text == "hi"));
    assert_eq!(decoded.provider_metadata["content"][0]["type"], "server_tool_use");
}

#[test]
fn stream_decoder_ignores_unknown_event_and_delta_types() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    let unknown_event = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"some_future_event","payload":{}}"#.to_vec(),
    )).unwrap();
    assert!(unknown_event.is_empty());

    let unknown_block_start = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_start","index":0,"content_block":{"type":"web_search_tool_result","content":[]}}"#.to_vec(),
    )).unwrap();
    assert!(unknown_block_start.is_empty());

    let unknown_delta = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{}}}"#.to_vec(),
    )).unwrap();
    assert!(unknown_delta.is_empty());
}

#[test]
fn stream_error_events_map_to_error_taxonomy() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    assert!(matches!(
        decoder.decode_frame(llm_client::RawStreamFrame::new(
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#.to_vec(),
        )),
        Err(llm_client::LlmError::ProviderInternal)
    ));
    assert!(matches!(
        decoder.decode_frame(llm_client::RawStreamFrame::new(
            br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#.to_vec(),
        )),
        Err(llm_client::LlmError::RateLimited { .. })
    ));
    assert!(matches!(
        decoder.decode_frame(llm_client::RawStreamFrame::new(
            br#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#.to_vec(),
        )),
        Err(llm_client::LlmError::Authentication)
    ));
    assert!(matches!(
        decoder.decode_frame(llm_client::RawStreamFrame::new(
            br#"{"type":"error","error":{"type":"invalid_request_error","message":"bad request"}}"#.to_vec(),
        )),
        Err(llm_client::LlmError::InvalidRequest { message }) if message.contains("bad request")
    ));
}

#[test]
fn encode_reasoning_budget_as_thinking() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["thinking"]["type"], "enabled");
    assert_eq!(provider_request.body_json["thinking"]["budget_tokens"], 2048);

    let bare = codec.encode_request(&LlmRequest::new("claude-sonnet-4-20250514")).unwrap();
    assert!(bare.body_json.get("thinking").is_none());
}

#[test]
fn encode_system_blocks_and_cache_control() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.system = vec![
        llm_client::SystemBlock { text: "stable prefix".to_string(), cache_control: Some(llm_client::CacheControl::Ephemeral) },
        llm_client::SystemBlock { text: "tail".to_string(), cache_control: None },
    ];
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: Some(llm_client::CacheControl::Ephemeral),
        }],
    });

    let provider_request = codec.encode_request(&request).unwrap();
    let body = &provider_request.body_json;

    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "stable prefix");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(body["system"][1].get("cache_control").is_none());
    assert_eq!(body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");

    let bare = codec
        .encode_request(&LlmRequest::new("claude-sonnet-4-20250514").with_user_text("hi"))
        .unwrap();
    assert!(bare.body_json.get("system").is_none());
    assert!(bare.body_json["messages"][0]["content"][0].get("cache_control").is_none());
}
