use llm_client::{AnthropicMessagesCodec, ContentBlock, LlmError, LlmRequest, ResponseFormat, ToolChoice, ToolDeclaration, WireCodec};
use llm_client::providers::{GeminiCodec, OpenAiChatCodec};

fn openai_codec() -> OpenAiChatCodec {
    OpenAiChatCodec::new("https://api.openai.com/v1")
}

fn gemini_codec() -> GeminiCodec {
    GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta")
}

fn anthropic_codec() -> AnthropicMessagesCodec {
    AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01")
}

fn request_with_block(model: &str, block: ContentBlock) -> LlmRequest {
    let mut request = LlmRequest::new(model);
    request.messages.push(llm_client::Message {
        role: "user".to_string(),
        content: vec![block],
    });
    request
}

#[test]
fn anthropic_encodes_stream_true() {
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.stream = true;

    let provider_request = anthropic_codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["stream"], true);
}

#[test]
fn openai_encodes_stream_true() {
    let mut request = LlmRequest::new("gpt-4o");
    request.stream = true;

    let provider_request = openai_codec().encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["stream"], true);
}

#[test]
fn openai_encodes_response_format_variants() {
    let cases = [
        (ResponseFormat::JsonObject, serde_json::json!({"type": "json_object"})),
        (
            ResponseFormat::JsonSchema { schema: serde_json::json!({"type": "object", "properties": {"answer": {"type": "string"}}}) },
            serde_json::json!({"type": "json_schema", "json_schema": {"name": "response", "strict": true, "schema": {"type": "object", "properties": {"answer": {"type": "string"}}}}}),
        ),
    ];

    for (response_format, expected) in cases {
        let mut request = LlmRequest::new("gpt-4o");
        request.response_format = Some(response_format);

        let provider_request = openai_codec().encode_request(&request).unwrap();

        assert_eq!(provider_request.body_json["response_format"], expected);
    }
}

#[test]
fn gemini_rejects_response_format_requests() {
    let cases = [
        ResponseFormat::JsonObject,
        ResponseFormat::JsonSchema { schema: serde_json::json!({"type": "object"}) },
    ];

    for response_format in cases {
        let mut request = LlmRequest::new("gemini-2.0-flash");
        request.response_format = Some(response_format);

        let err = gemini_codec().encode_request(&request).unwrap_err();

        assert!(matches!(err, LlmError::InvalidRequest { .. } | LlmError::UnsupportedCapability { .. }));
    }
}

#[test]
fn openai_encodes_tool_choice_variants() {
    let cases = [
        (ToolChoice::Auto, serde_json::json!("auto")),
        (ToolChoice::None, serde_json::json!("none")),
        (ToolChoice::Required, serde_json::json!("required")),
        (
            ToolChoice::Tool { name: "Read".to_string() },
            serde_json::json!({"type": "function", "function": {"name": "Read"}}),
        ),
    ];

    for (tool_choice, expected) in cases {
        let mut request = LlmRequest::new("gpt-4o");
        request.tool_choice = Some(tool_choice);
        request.tools = vec![ToolDeclaration {
            name: "Read".to_string(),
            description: "d".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            ..Default::default()
        }];

        let provider_request = openai_codec().encode_request(&request).unwrap();

        assert_eq!(provider_request.body_json["tool_choice"], expected);
    }
}

#[test]
fn gemini_encodes_tool_choice_variants() {
    // tool_choice is now supported for Gemini — verify it succeeds and emits toolConfig.
    let cases = [
        ToolChoice::Auto,
        ToolChoice::None,
        ToolChoice::Required,
        ToolChoice::Tool { name: "Read".to_string() },
    ];

    for tool_choice in cases {
        let mut request = LlmRequest::new("gemini-2.0-flash");
        request.tool_choice = Some(tool_choice);
        request.tools = vec![ToolDeclaration {
            name: "Read".to_string(),
            description: "d".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            ..Default::default()
        }];

        let provider_request = gemini_codec().encode_request(&request).unwrap();
        assert!(provider_request.body_json.get("toolConfig").is_some());
    }
}

#[test]
fn openai_rejects_unsupported_content_blocks() {
    // Image, ImageUrl, and Document are now supported. Remaining reject: Reasoning.
    let block = ContentBlock::Reasoning { text: "thought".to_string(), signature: None };
    let request = request_with_block("gpt-4o", block);
    let err = openai_codec().encode_request(&request).unwrap_err();
    assert!(matches!(err, LlmError::InvalidRequest { .. } | LlmError::UnsupportedCapability { .. }));
}

#[test]
fn openai_now_accepts_image_image_url_and_document_blocks() {
    // Image, ImageUrl, and Document are all supported.
    for block in [
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            bytes: vec![1, 2, 3],
        },
        ContentBlock::ImageUrl { url: "https://example.com/img.png".to_string() },
        ContentBlock::Document {
            media_type: "application/pdf".to_string(),
            bytes: vec![0x25, 0x50, 0x44, 0x46],
        },
    ] {
        let request = request_with_block("gpt-4o", block);
        openai_codec().encode_request(&request).expect("image/imageurl/document should be accepted");
    }
}

#[test]
fn gemini_rejects_unsupported_content_blocks() {
    // Image, Document, and ImageUrl are now supported. Remaining reject: Reasoning.
    let block = ContentBlock::Reasoning { text: "thought".to_string(), signature: None };
    let request = request_with_block("gemini-2.0-flash", block);
    let err = gemini_codec().encode_request(&request).unwrap_err();
    assert!(matches!(err, LlmError::InvalidRequest { .. } | LlmError::UnsupportedCapability { .. }));
}

#[test]
fn gemini_now_accepts_image_document_and_image_url_blocks() {
    // Image, Document, and ImageUrl are all supported.
    for block in [
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            bytes: vec![1, 2, 3],
        },
        ContentBlock::Document {
            media_type: "application/pdf".to_string(),
            bytes: vec![0x25, 0x50, 0x44, 0x46],
        },
        ContentBlock::ImageUrl { url: "https://example.com/img.png".to_string() },
    ] {
        let request = request_with_block("gemini-2.0-flash", block);
        gemini_codec().encode_request(&request).expect("image/document/imageurl should be accepted");
    }
}
