use llm_client::{AnthropicMessagesCodec, ContentBlock, LlmRequest, Message, ToolDeclaration, WireCodec};

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
