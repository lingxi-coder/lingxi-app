use base64::Engine as _;
use llm_client::providers::OpenAiResponsesCodec;
use llm_client::{ContentBlock, LlmRequest, Message, ToolChoice, ToolDeclaration, WireCodec};

const BASE_URL: &str = "https://api.openai.com/v1";

fn codec() -> OpenAiResponsesCodec {
    OpenAiResponsesCodec::new(BASE_URL)
}

// ── URL + headers ─────────────────────────────────────────────────────────────

#[test]
fn encode_request_posts_to_responses_url() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert_eq!(provider_request.method, "POST");
    assert_eq!(provider_request.url, "https://api.openai.com/v1/responses");
}

#[test]
fn encode_request_trims_trailing_slash_on_base_url() {
    let codec = OpenAiResponsesCodec::new("https://api.openai.com/v1/");
    let provider_request = codec
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert_eq!(provider_request.url, "https://api.openai.com/v1/responses");
}

#[test]
fn encode_request_sets_content_type_header_only() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert_eq!(
        provider_request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    // Auth is added later by DefaultLlmClient::authenticate, never by the codec.
    assert_eq!(provider_request.headers.len(), 1);
}

// ── instructions ──────────────────────────────────────────────────────────────

#[test]
fn system_blocks_join_into_instructions() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.system = vec![
        llm_client::SystemBlock::text("a"),
        llm_client::SystemBlock::text("b"),
    ];

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["instructions"], "a\n\nb");
}

#[test]
fn empty_system_omits_instructions() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert!(provider_request.body_json.get("instructions").is_none());
}

// ── input items: messages ─────────────────────────────────────────────────────

#[test]
fn user_text_message_encodes_input_text_message_item() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hello"))
        .unwrap();

    assert_eq!(
        provider_request.body_json["input"],
        serde_json::json!([{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}],
        }])
    );
}

#[test]
fn assistant_text_message_encodes_output_text_parts() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "answer".to_string(),
            cache_control: None,
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["input"],
        serde_json::json!([{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "answer"}],
        }])
    );
}

#[test]
fn tool_call_block_encodes_top_level_function_call_item() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "running it".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolCall {
                id: "call_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            },
        ],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    let input = provider_request.body_json["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    assert_eq!(
        input[0],
        serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "running it"}],
        })
    );
    // arguments is a JSON *string*, not an object (Responses API wire shape).
    assert_eq!(
        input[1],
        serde_json::json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "Bash",
            "arguments": "{\"command\":\"ls\"}",
        })
    );
}

#[test]
fn tool_result_block_encodes_function_call_output_item() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_call_id: "call_1".to_string(),
            output: serde_json::json!("done"),
            is_error: false,
            cache_control: None,
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["input"],
        serde_json::json!([{
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "done",
        }])
    );
}

#[test]
fn tool_result_non_string_output_is_stringified() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_call_id: "call_2".to_string(),
            output: serde_json::json!({"exit_code": 0}),
            is_error: false,
            cache_control: None,
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["input"][0]["output"],
        "{\"exit_code\":0}"
    );
}

// ── input items: images / documents ───────────────────────────────────────────

#[test]
fn image_bytes_encode_input_image_data_uri() {
    let bytes = vec![1u8, 2, 3];
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            bytes: bytes.clone(),
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert_eq!(
        provider_request.body_json["input"],
        serde_json::json!([{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": format!("data:image/png;base64,{b64}"),
            }],
        }])
    );
}

#[test]
fn image_url_encodes_input_image_url() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ImageUrl {
            url: "https://example.com/img.png".to_string(),
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["input"][0]["content"][0],
        serde_json::json!({
            "type": "input_image",
            "image_url": "https://example.com/img.png",
        })
    );
}

#[test]
fn document_encodes_input_file_data_uri() {
    let pdf_bytes = vec![0x25u8, 0x50, 0x44, 0x46]; // %PDF
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Document {
            media_type: "application/pdf".to_string(),
            bytes: pdf_bytes.clone(),
        }],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    let b64 = base64::engine::general_purpose::STANDARD.encode(&pdf_bytes);
    assert_eq!(
        provider_request.body_json["input"][0]["content"][0],
        serde_json::json!({
            "type": "input_file",
            "filename": "document",
            "file_data": format!("data:application/pdf;base64,{b64}"),
        })
    );
}

#[test]
fn mixed_text_and_image_stay_one_message_item_in_order() {
    let mut request = LlmRequest::new("gpt-5");
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "look".to_string(),
                cache_control: None,
            },
            ContentBlock::ImageUrl {
                url: "https://example.com/a.png".to_string(),
            },
        ],
    });

    let provider_request = codec().encode_request(&request).unwrap();

    let input = provider_request.body_json["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    let content = input[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[1]["type"], "input_image");
}

// ── tools ─────────────────────────────────────────────────────────────────────

#[test]
fn tools_encode_flattened_responses_shape() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.tools = vec![ToolDeclaration {
        name: "Read".to_string(),
        description: "read a file".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
    }];

    let provider_request = codec().encode_request(&request).unwrap();

    // Flattened Responses shape — NOT nested under "function" (that is Chat).
    assert_eq!(
        provider_request.body_json["tools"],
        serde_json::json!([{
            "type": "function",
            "name": "Read",
            "description": "read a file",
            "parameters": {"type": "object"},
            "strict": false,
        }])
    );
}

#[test]
fn no_tools_omits_tools_key() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert!(provider_request.body_json.get("tools").is_none());
}

#[test]
fn tool_choice_variants_encode() {
    let cases = [
        (ToolChoice::Auto, serde_json::json!("auto")),
        (ToolChoice::None, serde_json::json!("none")),
        (ToolChoice::Required, serde_json::json!("required")),
        (
            ToolChoice::Tool {
                name: "Read".to_string(),
            },
            serde_json::json!({"type": "function", "name": "Read"}),
        ),
    ];
    for (tool_choice, expected) in cases {
        let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
        request.tool_choice = Some(tool_choice);
        let provider_request = codec().encode_request(&request).unwrap();
        assert_eq!(provider_request.body_json["tool_choice"], expected);
    }
}

// ── sampling controls ─────────────────────────────────────────────────────────

#[test]
fn max_tokens_maps_to_max_output_tokens() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.max_tokens = Some(1024);

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["max_output_tokens"], 1024);
    assert!(provider_request.body_json.get("max_tokens").is_none());
}

#[test]
fn temperature_and_top_p_pass_through() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.temperature = Some(0.5);
    request.top_p = Some(0.9);

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["temperature"], 0.5);
    assert_eq!(provider_request.body_json["top_p"], 0.9);

    let bare = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();
    assert!(bare.body_json.get("max_output_tokens").is_none());
    assert!(bare.body_json.get("temperature").is_none());
    assert!(bare.body_json.get("top_p").is_none());
}

#[test]
fn stop_sequences_are_rejected() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.stop_sequences = vec!["END".to_string()];

    let err = codec().encode_request(&request).unwrap_err();

    assert!(
        matches!(err, llm_client::LlmError::InvalidRequest { ref message } if message.contains("stop")),
        "got: {err:?}"
    );
}

// ── reasoning effort mapping ──────────────────────────────────────────────────

#[test]
fn reasoning_budget_maps_to_effort_buckets() {
    let cases = [
        (1u32, "low"),
        (1024, "low"),
        (1025, "medium"),
        (8192, "medium"),
        (8193, "high"),
        (100_000, "high"),
    ];
    for (budget_tokens, effort) in cases {
        let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
        request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens });
        let provider_request = codec().encode_request(&request).unwrap();
        assert_eq!(
            provider_request.body_json["reasoning"],
            serde_json::json!({"effort": effort}),
            "budget {budget_tokens}"
        );
    }
}

#[test]
fn no_reasoning_config_omits_reasoning_key() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert!(provider_request.body_json.get("reasoning").is_none());
}

// ── response_format → text.format ─────────────────────────────────────────────

#[test]
fn response_format_json_object_encodes_text_format() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.response_format = Some(llm_client::ResponseFormat::JsonObject);

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["text"],
        serde_json::json!({"format": {"type": "json_object"}})
    );
}

#[test]
fn response_format_json_schema_encodes_text_format() {
    let schema = serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}});
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.response_format = Some(llm_client::ResponseFormat::JsonSchema {
        schema: schema.clone(),
    });

    let provider_request = codec().encode_request(&request).unwrap();

    // Shape pinned to codex codex-api common.rs TextFormat {type, strict, schema, name}.
    assert_eq!(
        provider_request.body_json["text"],
        serde_json::json!({
            "format": {
                "type": "json_schema",
                "strict": true,
                "schema": schema,
                "name": "response",
            }
        })
    );
}

#[test]
fn no_response_format_omits_text_key() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert!(provider_request.body_json.get("text").is_none());
}

// ── stream / store ────────────────────────────────────────────────────────────

#[test]
fn stream_true_sets_stream_in_body() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hi");
    request.stream = true;

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["stream"], true);
    assert_eq!(
        provider_request.stream_framing,
        llm_client::StreamFraming::Sse
    );
}

#[test]
fn stream_false_omits_stream_key() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert!(provider_request.body_json.get("stream").is_none());
}

#[test]
fn store_is_always_false() {
    let provider_request = codec()
        .encode_request(&LlmRequest::new("gpt-5").with_user_text("hi"))
        .unwrap();

    assert_eq!(provider_request.body_json["store"], false);
}

// ── unsupported history blocks ────────────────────────────────────────────────

#[test]
fn reasoning_history_blocks_are_rejected() {
    for block in [
        ContentBlock::Reasoning {
            text: "thought".to_string(),
            signature: None,
        },
        ContentBlock::RedactedThinking {
            data: "opaque".to_string(),
        },
    ] {
        let mut request = LlmRequest::new("gpt-5");
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![block],
        });
        let err = codec().encode_request(&request).unwrap_err();
        assert!(
            matches!(err, llm_client::LlmError::InvalidRequest { ref message } if message.contains("reasoning")),
            "got: {err:?}"
        );
    }
}

#[test]
fn server_generated_blocks_are_rejected() {
    for block in [
        ContentBlock::ServerToolUse {
            id: "srvtool_1".to_string(),
            name: "web_search".to_string(),
            input: serde_json::json!({}),
        },
        ContentBlock::ConnectorText {
            connector_text: "ct".to_string(),
            signature: None,
        },
        ContentBlock::AdvisorToolResult {
            tool_use_id: "srvtool_1".to_string(),
            content: serde_json::json!({}),
            is_error: false,
        },
    ] {
        let mut request = LlmRequest::new("gpt-5");
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![block],
        });
        let err = codec().encode_request(&request).unwrap_err();
        assert!(
            matches!(err, llm_client::LlmError::InvalidRequest { ref message } if message.contains("server-generated")),
            "got: {err:?}"
        );
    }
}

// ── full-body golden ──────────────────────────────────────────────────────────

#[test]
fn encode_request_full_body_golden() {
    let mut request = LlmRequest::new("gpt-5").with_user_text("hello");
    request.system = vec![llm_client::SystemBlock::text("sys")];
    request.stream = true;
    request.max_tokens = Some(4096);
    request.tools = vec![ToolDeclaration {
        name: "Bash".to_string(),
        description: "run a command".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
    }];
    request.tool_choice = Some(ToolChoice::Auto);
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let provider_request = codec().encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json,
        serde_json::json!({
            "model": "gpt-5",
            "instructions": "sys",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}],
            }],
            "tools": [{
                "type": "function",
                "name": "Bash",
                "description": "run a command",
                "parameters": {"type": "object"},
                "strict": false,
            }],
            "tool_choice": "auto",
            "max_output_tokens": 4096,
            "reasoning": {"effort": "medium"},
            "stream": true,
            "store": false,
        })
    );
}

// ── stubs (decode lands in batch-3 T2/T3) ─────────────────────────────────────

#[test]
fn decode_response_stub_errors_until_t2() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(200, serde_json::json!({})))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn stream_decoder_stub_errors_until_t3() {
    let mut decoder = codec().stream_decoder();
    let err = decoder
        .decode_frame(llm_client::RawStreamFrame::new(b"{}".to_vec()))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}
