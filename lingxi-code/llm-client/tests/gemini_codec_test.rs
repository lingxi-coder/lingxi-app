use llm_client::{ContentBlock, LlmRequest, ProviderResponse, ToolDeclaration, WireCodec};
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
