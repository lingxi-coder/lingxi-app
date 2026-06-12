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

// ── decode_response ───────────────────────────────────────────────────────────
//
// Wire shapes pinned to the vendored Codex CLI reference:
// output items per codex-rs/protocol/src/models.rs ResponseItem
// (message{role,content:[{type:"output_text",text}]},
//  function_call{call_id,name,arguments-as-JSON-string},
//  reasoning{summary:[{type:"summary_text",text}]}),
// usage per codex-rs/codex-api/src/sse/responses.rs ResponseCompletedUsage.

fn decode(body: serde_json::Value) -> llm_client::LlmResponse {
    codec()
        .decode_response(llm_client::ProviderResponse::json(200, body))
        .unwrap()
}

fn completed_body(output: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "completed",
        "output": output,
    })
}

#[test]
fn decode_response_parses_id_and_model() {
    let decoded = decode(completed_body(&serde_json::json!([])));
    assert_eq!(decoded.id, "resp_1");
    assert_eq!(decoded.model, "gpt-5");
}

#[test]
fn decode_response_message_output_text_parts_become_text_blocks() {
    let decoded = decode(completed_body(&serde_json::json!([{
        "type": "message",
        "role": "assistant",
        "content": [
            {"type": "output_text", "text": "hello"},
            {"type": "refusal", "refusal": "nope"},
            {"type": "output_text", "text": "world"},
        ],
    }])));
    assert_eq!(
        decoded.content,
        vec![
            ContentBlock::Text { text: "hello".to_string(), cache_control: None },
            ContentBlock::Text { text: "world".to_string(), cache_control: None },
        ]
    );
}

#[test]
fn decode_response_function_call_becomes_tool_call_with_parsed_arguments() {
    let decoded = decode(completed_body(&serde_json::json!([{
        "type": "function_call",
        "call_id": "call_7",
        "name": "Bash",
        "arguments": "{\"command\":\"ls\"}",
    }])));
    assert_eq!(
        decoded.content,
        vec![ContentBlock::ToolCall {
            id: "call_7".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
        }]
    );
}

#[test]
fn decode_response_unparseable_arguments_fall_back_to_raw_string() {
    let decoded = decode(completed_body(&serde_json::json!([{
        "type": "function_call",
        "call_id": "call_7",
        "name": "Bash",
        "arguments": "{not json",
    }])));
    assert_eq!(
        decoded.content,
        vec![ContentBlock::ToolCall {
            id: "call_7".to_string(),
            name: "Bash".to_string(),
            input: serde_json::Value::String("{not json".to_string()),
        }]
    );
}

#[test]
fn decode_response_function_call_missing_call_id_errors() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            200,
            completed_body(&serde_json::json!([{
                "type": "function_call",
                "name": "Bash",
                "arguments": "{}",
            }])),
        ))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn decode_response_reasoning_summary_parts_become_reasoning_block() {
    let decoded = decode(completed_body(&serde_json::json!([{
        "type": "reasoning",
        "id": "rs_1",
        "summary": [
            {"type": "summary_text", "text": "first thought"},
            {"type": "summary_text", "text": "second thought"},
        ],
    }])));
    assert_eq!(
        decoded.content,
        vec![ContentBlock::Reasoning {
            text: "first thought\n\nsecond thought".to_string(),
            signature: None,
        }]
    );
}

#[test]
fn decode_response_reasoning_without_summary_is_skipped() {
    let decoded = decode(completed_body(&serde_json::json!([
        {"type": "reasoning", "id": "rs_1", "summary": []},
        {"type": "reasoning", "id": "rs_2"},
        {"type": "message", "role": "assistant",
         "content": [{"type": "output_text", "text": "hi"}]},
    ])));
    assert_eq!(
        decoded.content,
        vec![ContentBlock::Text { text: "hi".to_string(), cache_control: None }]
    );
}

#[test]
fn decode_response_unknown_output_items_are_skipped() {
    let decoded = decode(completed_body(&serde_json::json!([
        {"type": "web_search_call", "id": "ws_1", "status": "completed"},
        {"type": "message", "role": "assistant",
         "content": [{"type": "output_text", "text": "hi"}]},
    ])));
    assert_eq!(
        decoded.content,
        vec![ContentBlock::Text { text: "hi".to_string(), cache_control: None }]
    );
}

#[test]
fn decode_response_preserves_output_item_order() {
    let decoded = decode(completed_body(&serde_json::json!([
        {"type": "reasoning", "id": "rs_1",
         "summary": [{"type": "summary_text", "text": "thinking"}]},
        {"type": "message", "role": "assistant",
         "content": [{"type": "output_text", "text": "I'll run it"}]},
        {"type": "function_call", "call_id": "call_1", "name": "Bash",
         "arguments": "{}"},
    ])));
    assert_eq!(
        decoded.content,
        vec![
            ContentBlock::Reasoning { text: "thinking".to_string(), signature: None },
            ContentBlock::Text { text: "I'll run it".to_string(), cache_control: None },
            ContentBlock::ToolCall {
                id: "call_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({}),
            },
        ]
    );
}

#[test]
fn decode_response_completed_maps_end_turn() {
    let decoded = decode(completed_body(&serde_json::json!([])));
    assert_eq!(decoded.stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn decode_response_completed_with_function_call_maps_tool_use() {
    let decoded = decode(completed_body(&serde_json::json!([{
        "type": "function_call", "call_id": "call_1", "name": "Bash", "arguments": "{}",
    }])));
    assert_eq!(decoded.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn decode_response_incomplete_max_output_tokens_maps_max_tokens() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "incomplete",
        "incomplete_details": {"reason": "max_output_tokens"},
        "output": [],
    }));
    assert_eq!(decoded.stop_reason.as_deref(), Some("max_tokens"));
}

#[test]
fn decode_response_incomplete_other_reason_passes_through_verbatim() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "incomplete",
        "incomplete_details": {"reason": "content_filter"},
        "output": [],
    }));
    assert_eq!(decoded.stop_reason.as_deref(), Some("content_filter"));
}

#[test]
fn decode_response_incomplete_without_reason_passes_status_verbatim() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "incomplete",
        "output": [],
    }));
    assert_eq!(decoded.stop_reason.as_deref(), Some("incomplete"));
}

#[test]
fn decode_response_usage_subset_normalization() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "completed",
        "output": [],
        "usage": {
            "input_tokens": 100,
            "input_tokens_details": {"cached_tokens": 30},
            "output_tokens": 50,
            "output_tokens_details": {"reasoning_tokens": 20},
            "total_tokens": 150,
        },
    }));
    assert_eq!(decoded.usage.billable_tokens.input, 70);
    assert_eq!(decoded.usage.billable_tokens.cache_read, 30);
    assert_eq!(decoded.usage.billable_tokens.output, 30);
    assert_eq!(decoded.usage.billable_tokens.reasoning_output, 20);
    assert_eq!(decoded.usage.billable_tokens.cache_write, 0);
    assert_eq!(decoded.usage.provider_reported_total_tokens, Some(150));
    assert_eq!(decoded.usage.context_tokens, Some(150));
}

#[test]
fn decode_response_usage_subtraction_saturates() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "completed",
        "output": [],
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": {"cached_tokens": 30},
            "output_tokens": 5,
            "output_tokens_details": {"reasoning_tokens": 20},
            "total_tokens": 15,
        },
    }));
    assert_eq!(decoded.usage.billable_tokens.input, 0);
    assert_eq!(decoded.usage.billable_tokens.output, 0);
    assert_eq!(decoded.usage.billable_tokens.cache_read, 30);
    assert_eq!(decoded.usage.billable_tokens.reasoning_output, 20);
}

#[test]
fn decode_response_usage_missing_details_objects_zero_tolerantly() {
    let decoded = decode(serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "completed",
        "output": [],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "total_tokens": 150,
        },
    }));
    assert_eq!(decoded.usage.billable_tokens.input, 100);
    assert_eq!(decoded.usage.billable_tokens.cache_read, 0);
    assert_eq!(decoded.usage.billable_tokens.output, 50);
    assert_eq!(decoded.usage.billable_tokens.reasoning_output, 0);
}

#[test]
fn decode_response_missing_usage_defaults_to_zero() {
    let decoded = decode(completed_body(&serde_json::json!([])));
    assert_eq!(decoded.usage, llm_client::Usage::default());
}

#[test]
fn decode_response_cost_is_none_and_metadata_is_body() {
    let body = serde_json::json!({
        "id": "resp_1",
        "model": "gpt-5",
        "status": "completed",
        "output": [],
        "vendor_extra": {"k": "v"},
    });
    let decoded = decode(body.clone());
    assert!(decoded.cost.is_none());
    assert_eq!(decoded.provider_metadata, body);
}

#[test]
fn decode_response_missing_id_errors() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            200,
            serde_json::json!({"model": "gpt-5", "status": "completed", "output": []}),
        ))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::InvalidRequest { .. }));
}

#[test]
fn decode_response_http_429_maps_rate_limited() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            429,
            serde_json::json!({"error": {"message": "slow down", "type": "rate_limit_error"}}),
        ))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::RateLimited { .. }));
}

#[test]
fn decode_response_http_401_maps_authentication() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            401,
            serde_json::json!({"error": {"message": "bad key", "code": "invalid_api_key"}}),
        ))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::Authentication));
}

#[test]
fn decode_response_insufficient_quota_maps_quota_exceeded() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            429,
            serde_json::json!({"error": {"message": "quota", "code": "insufficient_quota"}}),
        ))
        .unwrap_err();
    assert!(matches!(err, llm_client::LlmError::QuotaExceeded));
}

#[test]
fn decode_response_http_400_maps_invalid_request_with_message() {
    let err = codec()
        .decode_response(llm_client::ProviderResponse::json(
            400,
            serde_json::json!({"error": {"message": "bad input", "type": "invalid_request_error"}}),
        ))
        .unwrap_err();
    assert!(
        matches!(err, llm_client::LlmError::InvalidRequest { ref message } if message == "bad input")
    );
}

// ── stream decoder ────────────────────────────────────────────────────────────

use llm_client::{
    ContentDelta, LlmEvent, LlmResponse, MessageDeltaPayload, RawStreamFrame, TokenUsage, Usage,
};

/// Feed SSE data payloads (already de-framed) through a fresh stream decoder.
fn decode_stream(frames: &[serde_json::Value]) -> Vec<LlmEvent> {
    let mut decoder = codec().stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(
            decoder
                .decode_frame(RawStreamFrame::new(frame.to_string().into_bytes()))
                .unwrap(),
        );
    }
    events
}

fn created_frame() -> serde_json::Value {
    serde_json::json!({
        "type": "response.created",
        "response": {"id": "resp_1", "model": "gpt-5", "status": "in_progress"},
    })
}

#[test]
fn stream_created_emits_message_start_snapshot() {
    let events = decode_stream(&[created_frame()]);

    assert_eq!(
        events,
        vec![LlmEvent::MessageStart {
            response: Box::new(LlmResponse {
                id: "resp_1".to_string(),
                model: "gpt-5".to_string(),
                content: Vec::new(),
                stop_reason: None,
                usage: Usage::default(),
                cost: None,
                provider_metadata: serde_json::Value::Null,
            }),
        }]
    );
}

#[test]
fn stream_text_delta_opens_block_once_then_deltas() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "Hel",
        }),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "lo",
        }),
    ]);

    assert_eq!(events.len(), 4);
    assert_eq!(
        events[1],
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
        }
    );
    assert_eq!(
        events[2],
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta { text: "Hel".to_string() },
        }
    );
    assert_eq!(
        events[3],
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta { text: "lo".to_string() },
        }
    );
}

#[test]
fn stream_function_call_added_starts_tool_block_with_empty_input() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": ""},
        }),
    ]);

    assert_eq!(
        events[1],
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::ToolCall {
                id: "call_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::Value::Object(serde_json::Map::new()),
            },
        }
    );
}

#[test]
fn stream_function_call_arguments_delta_emits_input_json_delta() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": ""},
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1", "output_index": 0,
            "delta": "{\"command\":",
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1", "output_index": 0,
            "delta": "\"ls\"}",
        }),
    ]);

    assert_eq!(
        &events[2..],
        &[
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{\"command\":".to_string(),
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "\"ls\"}".to_string(),
                },
            },
        ]
    );
}

#[test]
fn stream_reasoning_text_delta_opens_reasoning_block_once() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.reasoning_text.delta",
            "item_id": "rs_1", "output_index": 0, "content_index": 0,
            "delta": "thinking ",
        }),
        serde_json::json!({
            "type": "response.reasoning_text.delta",
            "item_id": "rs_1", "output_index": 0, "content_index": 0,
            "delta": "hard",
        }),
    ]);

    assert_eq!(events.len(), 4);
    assert_eq!(
        events[1],
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Reasoning {
                text: String::new(),
                signature: None,
            },
        }
    );
    assert_eq!(
        events[2],
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::ThinkingDelta {
                thinking: "thinking ".to_string(),
            },
        }
    );
}

#[test]
fn stream_reasoning_summary_delta_shares_reasoning_block() {
    // Both reasoning_text.delta and reasoning_summary_text.delta target the
    // same item's single Reasoning block: one start, two thinking deltas.
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1", "output_index": 0, "summary_index": 0,
            "delta": "summary ",
        }),
        serde_json::json!({
            "type": "response.reasoning_text.delta",
            "item_id": "rs_1", "output_index": 0, "content_index": 0,
            "delta": "raw",
        }),
    ]);

    let starts = events
        .iter()
        .filter(|event| matches!(event, LlmEvent::ContentBlockStart { .. }))
        .count();
    assert_eq!(starts, 1);
    let thinking: String = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ContentBlockDelta {
                delta: ContentDelta::ThinkingDelta { thinking },
                ..
            } => Some(thinking.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, "summary raw");
}

#[test]
fn stream_output_item_done_emits_content_block_stop() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "hi",
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "message", "id": "msg_1", "role": "assistant"},
        }),
    ]);

    assert_eq!(events.last(), Some(&LlmEvent::ContentBlockStop { index: 0 }));
}

#[test]
fn stream_output_index_keys_block_mapping() {
    // Interleaved deltas for two wire items route to two sequential block
    // indices keyed by output_index.
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.reasoning_text.delta",
            "item_id": "rs_1", "output_index": 0, "content_index": 0,
            "delta": "think",
        }),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 1, "content_index": 0,
            "delta": "answer",
        }),
        serde_json::json!({
            "type": "response.reasoning_text.delta",
            "item_id": "rs_1", "output_index": 0, "content_index": 0,
            "delta": " more",
        }),
    ]);

    assert_eq!(
        events[1],
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Reasoning {
                text: String::new(),
                signature: None,
            },
        }
    );
    assert_eq!(
        events[3],
        LlmEvent::ContentBlockStart {
            index: 1,
            content_block: ContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
        }
    );
    assert_eq!(
        events[5],
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::ThinkingDelta {
                thinking: " more".to_string(),
            },
        }
    );
}

#[test]
fn stream_completed_closes_open_blocks_then_message_delta_and_stop() {
    let usage_json = serde_json::json!({
        "input_tokens": 100,
        "input_tokens_details": {"cached_tokens": 40},
        "output_tokens": 50,
        "output_tokens_details": {"reasoning_tokens": 30},
        "total_tokens": 150,
    });
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "hi",
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1", "model": "gpt-5", "status": "completed",
                "usage": usage_json,
            },
        }),
    ]);

    // The still-open text block is closed before the terminal events.
    assert_eq!(
        &events[3..],
        &[
            LlmEvent::ContentBlockStop { index: 0 },
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("end_turn".to_string()),
                },
                usage: Some(Usage {
                    billable_tokens: TokenUsage {
                        input: 60,
                        output: 20,
                        cache_read: 40,
                        reasoning_output: 30,
                        ..Default::default()
                    },
                    context_tokens: Some(150),
                    provider_reported_total_tokens: Some(150),
                    provider_metadata: usage_json,
                    ..Default::default()
                }),
            },
            LlmEvent::MessageStop,
        ]
    );
}

#[test]
fn stream_completed_after_function_call_maps_tool_use() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": ""},
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": "{}"},
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {"id": "resp_1", "model": "gpt-5", "status": "completed"},
        }),
    ]);

    let stop_reason = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::MessageDelta { delta, .. } => delta.stop_reason.clone(),
            _ => None,
        })
        .expect("terminal stop reason");
    assert_eq!(stop_reason, "tool_use");
}

#[test]
fn stream_incomplete_max_output_tokens_maps_max_tokens() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1", "model": "gpt-5", "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"},
            },
        }),
    ]);

    assert_eq!(
        &events[1..],
        &[
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("max_tokens".to_string()),
                },
                usage: None,
            },
            LlmEvent::MessageStop,
        ]
    );
}

#[test]
fn stream_failed_maps_error_taxonomy() {
    let mut decoder = codec().stream_decoder();
    decoder
        .decode_frame(RawStreamFrame::new(created_frame().to_string().into_bytes()))
        .unwrap();
    let err = decoder
        .decode_frame(RawStreamFrame::new(
            serde_json::json!({
                "type": "response.failed",
                "response": {
                    "id": "resp_1", "status": "failed",
                    "error": {"code": "context_length_exceeded", "message": "too long"},
                },
            })
            .to_string()
            .into_bytes(),
        ))
        .unwrap_err();

    assert!(matches!(err, llm_client::LlmError::ContextOverflow { .. }));
}

#[test]
fn stream_failed_quota_maps_quota_exceeded() {
    let mut decoder = codec().stream_decoder();
    let err = decoder
        .decode_frame(RawStreamFrame::new(
            serde_json::json!({
                "type": "response.failed",
                "response": {
                    "id": "resp_1", "status": "failed",
                    "error": {"code": "insufficient_quota", "message": "quota gone"},
                },
            })
            .to_string()
            .into_bytes(),
        ))
        .unwrap_err();

    assert!(matches!(err, llm_client::LlmError::QuotaExceeded));
}

#[test]
fn stream_unknown_event_types_ignored() {
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({"type": "response.in_progress", "response": {"id": "resp_1"}}),
        serde_json::json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 0, "text": "hi"}),
        serde_json::json!({"type": "response.some_future_event"}),
    ]);

    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], LlmEvent::MessageStart { .. }));
}

#[test]
fn stream_finish_without_completed_closes_blocks_and_stops() {
    let mut decoder = codec().stream_decoder();
    let mut events = Vec::new();
    for frame in [
        created_frame(),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "hi",
        }),
    ] {
        events.extend(
            decoder
                .decode_frame(RawStreamFrame::new(frame.to_string().into_bytes()))
                .unwrap(),
        );
    }
    events.extend(decoder.finish().unwrap());

    assert_eq!(
        &events[3..],
        &[
            LlmEvent::ContentBlockStop { index: 0 },
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload { stop_reason: None },
                usage: None,
            },
            LlmEvent::MessageStop,
        ]
    );
    // finish() is idempotent: a second call emits nothing.
    assert_eq!(decoder.finish().unwrap(), Vec::new());
}

#[test]
fn stream_delta_without_output_index_defaults_to_slot_zero() {
    // Mirror the OpenAiChatCodec tolerance: a single-item stream that omits
    // output_index routes to slot zero instead of dropping the fragment.
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": "a"}),
        serde_json::json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": "b"}),
    ]);

    let starts = events
        .iter()
        .filter(|event| matches!(event, LlmEvent::ContentBlockStart { .. }))
        .count();
    assert_eq!(starts, 1);
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ContentBlockDelta {
                delta: ContentDelta::TextDelta { text },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "ab");
}

#[test]
#[allow(clippy::too_many_lines)] // full-transcript pin: every event in sequence
fn stream_happy_path_exact_event_sequence() {
    let usage_json = serde_json::json!({
        "input_tokens": 100,
        "input_tokens_details": {"cached_tokens": 40},
        "output_tokens": 50,
        "output_tokens_details": {"reasoning_tokens": 30},
        "total_tokens": 150,
    });
    let events = decode_stream(&[
        created_frame(),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "message", "id": "msg_1", "role": "assistant"},
        }),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "On ",
        }),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1", "output_index": 0, "content_index": 0,
            "delta": "it.",
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "message", "id": "msg_1", "role": "assistant"},
        }),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": ""},
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1", "output_index": 1,
            "delta": "{\"command\":",
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1", "output_index": 1,
            "delta": "\"ls\"}",
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output_index": 1,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "Bash", "arguments": "{\"command\":\"ls\"}"},
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1", "model": "gpt-5", "status": "completed",
                "usage": usage_json,
            },
        }),
    ]);

    assert_eq!(
        events,
        vec![
            LlmEvent::MessageStart {
                response: Box::new(LlmResponse {
                    id: "resp_1".to_string(),
                    model: "gpt-5".to_string(),
                    content: Vec::new(),
                    stop_reason: None,
                    usage: Usage::default(),
                    cost: None,
                    provider_metadata: serde_json::Value::Null,
                }),
            },
            LlmEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: "On ".to_string() },
            },
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: "it.".to_string() },
            },
            LlmEvent::ContentBlockStop { index: 0 },
            LlmEvent::ContentBlockStart {
                index: 1,
                content_block: ContentBlock::ToolCall {
                    id: "call_1".to_string(),
                    name: "Bash".to_string(),
                    input: serde_json::Value::Object(serde_json::Map::new()),
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 1,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{\"command\":".to_string(),
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 1,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "\"ls\"}".to_string(),
                },
            },
            LlmEvent::ContentBlockStop { index: 1 },
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("tool_use".to_string()),
                },
                usage: Some(Usage {
                    billable_tokens: TokenUsage {
                        input: 60,
                        output: 20,
                        cache_read: 40,
                        reasoning_output: 30,
                        ..Default::default()
                    },
                    context_tokens: Some(150),
                    provider_reported_total_tokens: Some(150),
                    provider_metadata: usage_json,
                    ..Default::default()
                }),
            },
            LlmEvent::MessageStop,
        ]
    );
}
