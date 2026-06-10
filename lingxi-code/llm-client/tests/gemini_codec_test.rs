use llm_client::{ContentBlock, LlmEvent, LlmRequest, ProviderResponse, RawStreamFrame, ToolDeclaration, WireCodec};
use llm_client::providers::GeminiCodec;

#[test]
fn encode_request_shape_is_gemini_generate_content() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.system = Some("sys".to_string());
    request.tools = vec![ToolDeclaration {
        name: "Read".to_string(),
        description: "d".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    }];

    let provider_request = codec.encode_request(&request).unwrap();

    assert!(provider_request.url.contains(":generateContent"));
    assert_eq!(provider_request.body_json["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(provider_request.body_json["tools"][0]["functionDeclarations"][0]["name"], "Read");
}

#[test]
fn decode_text_and_function_call() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let text = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"text":"hi there"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3}
    }));
    let decoded = codec.decode_response(text).unwrap();
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
    assert!(matches!(decoded.content[0], ContentBlock::Text { .. }));

    let tool = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]
    }));
    let decoded = codec.decode_response(tool).unwrap();
    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref name, .. } if name == "Bash"));
}

#[test]
fn encode_tool_result_uses_prior_tool_call_name() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.messages.push(llm_client::Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolCall {
            id: "call_1".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({"command":"ls"}),
        }],
    });
    request.messages.push(llm_client::Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_call_id: "call_1".to_string(),
            output: serde_json::json!("done"),
        }],
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["contents"][1]["parts"][0]["functionResponse"]["name"], "Bash");
}

#[test]
fn decode_cached_content_token_count_maps_to_cache_read_usage() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let response = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3,"cachedContentTokenCount":7}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert_eq!(decoded.usage.billable_tokens.cache_read, 7);
}

#[test]
fn sse_stream_reassembles_text_then_thought_then_function_call() {
    let frames = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"On it."}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"thinking","thought":true}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]} ,"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
    ];
    let mut decoder = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    assert!(matches!(events.first(), Some(LlmEvent::MessageStart { .. })));
    assert!(matches!(events[1], LlmEvent::ContentBlockStart { content_block: ContentBlock::Text { .. }, .. }));
    assert!(matches!(events[2], LlmEvent::ContentBlockDelta { delta: llm_client::ContentDelta::TextDelta { ref text }, .. } if text == "On it."));
    assert!(matches!(events[3], LlmEvent::ContentBlockStart { content_block: ContentBlock::Reasoning { .. }, .. }));
    assert!(matches!(events[4], LlmEvent::ContentBlockDelta { delta: llm_client::ContentDelta::ThinkingDelta { ref thinking }, .. } if thinking == "thinking"));
    assert!(matches!(events[5], LlmEvent::ContentBlockStart { content_block: ContentBlock::ToolCall { ref name, .. }, .. } if name == "Bash"));
    assert!(matches!(events[6], LlmEvent::ContentBlockDelta { delta: llm_client::ContentDelta::InputJsonDelta { ref partial_json }, .. } if partial_json == "{\"command\":\"ls\"}"));
    assert!(matches!(events[7], LlmEvent::ContentBlockStop { .. }));
    assert!(matches!(events[8], LlmEvent::ContentBlockStop { .. }));
    assert!(matches!(events[9], LlmEvent::ContentBlockStop { .. }));
    assert!(matches!(events[10], LlmEvent::MessageDelta { delta: llm_client::MessageDeltaPayload { stop_reason: Some(ref reason) }, .. } if reason == "tool_use"));
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}

#[test]
fn decode_usage_excludes_cached_tokens_from_input_and_maps_thoughts() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let response = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":10,"cachedContentTokenCount":40,"thoughtsTokenCount":5,"totalTokenCount":115}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert_eq!(decoded.usage.billable_tokens.input, 60);
    assert_eq!(decoded.usage.billable_tokens.cache_read, 40);
    assert_eq!(decoded.usage.billable_tokens.output, 10);
    assert_eq!(decoded.usage.billable_tokens.reasoning_output, 5);
    assert_eq!(decoded.usage.provider_reported_total_tokens, Some(115));
}

#[test]
fn stream_usage_only_final_frame_is_not_lost() {
    let frames = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]},"finishReason":"STOP"}]}"#,
        r#"{"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2,"totalTokenCount":7}}"#,
    ];
    let mut decoder = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta").stream_decoder();
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
        .expect("terminal usage should survive a usage-only final frame");

    assert_eq!(usage.billable_tokens.input, 5);
    assert_eq!(usage.billable_tokens.output, 2);
}

#[test]
fn decode_synthesizes_unique_tool_call_ids() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let response = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[
            {"functionCall":{"name":"Bash","args":{"command":"ls"}}},
            {"functionCall":{"name":"Read","args":{"path":"a.txt"}}}
        ]},"finishReason":"STOP"}]
    }));

    let decoded = codec.decode_response(response).unwrap();

    let ids: Vec<&str> = decoded
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| !id.is_empty()));
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn stream_synthesizes_unique_tool_call_ids() {
    let frame = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}},{"functionCall":{"name":"Read","args":{"path":"a.txt"}}}]},"finishReason":"STOP"}]}"#;
    let mut decoder = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta").stream_decoder();
    let mut events = decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap();
    events.extend(decoder.finish().unwrap());

    let ids: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ContentBlockStart { content_block: ContentBlock::ToolCall { id, .. }, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| !id.is_empty()));
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn encode_tool_result_with_unknown_call_id_is_rejected() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.messages.push(llm_client::Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_call_id: "call_unseen".to_string(),
            output: serde_json::json!("done"),
        }],
    });

    let err = codec.encode_request(&request).unwrap_err();

    assert!(matches!(err, llm_client::LlmError::InvalidRequest { message } if message.contains("call_unseen")));
}

#[test]
fn decode_blocked_prompt_reports_block_reason() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let response = ProviderResponse::json(200, serde_json::json!({
        "promptFeedback":{"blockReason":"SAFETY"}
    }));

    let err = codec.decode_response(response).unwrap_err();

    assert!(matches!(err, llm_client::LlmError::InvalidRequest { message } if message.contains("SAFETY")));
}
