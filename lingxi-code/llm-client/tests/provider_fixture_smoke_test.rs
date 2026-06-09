use llm_client::{
    providers::GeminiCodec, AnthropicMessagesCodec, LlmEvent, OpenAiChatCodec, ProviderResponse,
    RawStreamFrame, WireCodec,
};

fn decode_jsonl_stream<C: WireCodec>(codec: &C, fixture: &str) -> Vec<LlmEvent> {
    let mut decoder = codec.stream_decoder();
    let mut events = Vec::new();

    for frame in fixture.lines().map(str::trim).filter(|line| !line.is_empty()) {
        events.extend(
            decoder
                .decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec()))
                .unwrap(),
        );
    }

    events.extend(decoder.finish().unwrap());
    events
}

fn assert_single_message_stop(events: &[LlmEvent]) {
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LlmEvent::MessageStop))
            .count(),
        1
    );
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}

#[test]
fn provider_fixtures_smoke_test() {
    let openai_fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/openai_stream_text_tool.jsonl"
    ));
    let gemini_fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gemini_stream_text_tool.jsonl"
    ));
    let anthropic_fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/anthropic_text_response.json"
    ));

    let openai_events = decode_jsonl_stream(
        &OpenAiChatCodec::new("https://api.openai.com/v1"),
        openai_fixture,
    );
    assert_single_message_stop(&openai_events);

    let gemini_events = decode_jsonl_stream(
        &GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta"),
        gemini_fixture,
    );
    assert_single_message_stop(&gemini_events);

    let anthropic = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let decoded = anthropic
        .decode_response(ProviderResponse::json(
            200,
            serde_json::from_str(anthropic_fixture).unwrap(),
        ))
        .unwrap();

    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
}
