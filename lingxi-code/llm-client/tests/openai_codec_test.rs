use llm_client::{
    ContentBlock, ContentDelta, LlmEvent, LlmRequest, OpenAiChatCodec, ProviderResponse,
    RawStreamFrame, ToolDeclaration, WireCodec,
};

#[test]
fn encode_request_shape_is_openai_chat_completions() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.system = vec![llm_client::SystemBlock::text("sys")];
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
    assert_eq!(decoded.stop_reason.as_deref(), Some("end_turn"));

    let tool = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-y",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]
    }));
    let decoded = codec.decode_response(tool).unwrap();
    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref name, .. } if name == "Bash"));
    assert_eq!(decoded.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn encode_sampling_controls() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.max_tokens = Some(1024);
    request.temperature = Some(0.5);
    request.top_p = Some(0.9);
    request.stop_sequences = vec!["END".to_string()];

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["max_tokens"], 1024);
    assert_eq!(provider_request.body_json["temperature"], 0.5);
    assert_eq!(provider_request.body_json["top_p"], 0.9);
    assert_eq!(provider_request.body_json["stop"], serde_json::json!(["END"]));

    let bare = codec.encode_request(&LlmRequest::new("gpt-4o")).unwrap();
    assert!(bare.body_json.get("max_tokens").is_none());
    assert!(bare.body_json.get("temperature").is_none());
    assert!(bare.body_json.get("top_p").is_none());
    assert!(bare.body_json.get("stop").is_none());
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
            is_error: false,
            cache_control: None,
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
                is_error: false,
                cache_control: None,
            },
            ContentBlock::ToolResult {
                tool_call_id: "call_2".to_string(),
                output: serde_json::json!("second"),
                is_error: false,
                cache_control: None,
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

#[test]
fn stream_usage_keeps_reasoning_and_cached_buckets_independent() {
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens_details":{"reasoning_tokens":30}}}"#,
        "[DONE]",
    ];
    let mut decoder = OpenAiChatCodec::new("https://api.openai.com/v1").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    let usage = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::MessageDelta { usage: Some(usage), .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("terminal usage");

    assert_eq!(usage.billable_tokens.input, 60);
    assert_eq!(usage.billable_tokens.cache_read, 40);
    assert_eq!(usage.billable_tokens.output, 20);
    assert_eq!(usage.billable_tokens.reasoning_output, 30);
}

#[test]
fn decode_response_usage_normalization_matches_stream_path() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-u",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens_details":{"reasoning_tokens":30},"total_tokens":150}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert_eq!(decoded.usage.billable_tokens.input, 60);
    assert_eq!(decoded.usage.billable_tokens.cache_read, 40);
    assert_eq!(decoded.usage.billable_tokens.output, 20);
    assert_eq!(decoded.usage.billable_tokens.reasoning_output, 30);
    assert_eq!(decoded.usage.provider_reported_total_tokens, Some(150));
}

#[test]
fn stream_tool_fragment_without_index_defaults_to_slot_zero() {
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"id":"call_n","type":"function","function":{"name":"Bash","arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
    let mut decoder = OpenAiChatCodec::new("https://api.openai.com/v1").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    let starts = events
        .iter()
        .filter(|event| matches!(
            event,
            LlmEvent::ContentBlockStart { content_block: ContentBlock::ToolCall { .. }, .. }
        ))
        .count();
    assert_eq!(starts, 1);

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
}

#[test]
fn reasoning_config_is_rejected_until_responses_api_exists() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let err = codec.encode_request(&request).unwrap_err();

    assert!(matches!(err, llm_client::LlmError::InvalidRequest { message } if message.contains("reasoning")));
}

#[test]
fn system_blocks_join_into_one_system_message() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.system = vec![
        llm_client::SystemBlock { text: "a".to_string(), cache_control: Some(llm_client::CacheControl::Ephemeral) },
        llm_client::SystemBlock { text: "b".to_string(), cache_control: None },
    ];

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["messages"][0]["role"], "system");
    assert_eq!(provider_request.body_json["messages"][0]["content"], "a\n\nb");

    let bare = codec
        .encode_request(&LlmRequest::new("gpt-4o").with_user_text("hi"))
        .unwrap();
    assert_eq!(bare.body_json["messages"][0]["role"], "user");
}
