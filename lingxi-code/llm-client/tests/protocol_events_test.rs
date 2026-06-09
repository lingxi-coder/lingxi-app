use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDelta};

#[test]
fn stream_events_support_block_lifecycle_and_terminal_delta() {
    let start = LlmEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::Text { text: String::new() },
    };
    let delta = LlmEvent::ContentBlockDelta {
        index: 0,
        delta: ContentDelta::TextDelta { text: "hi".to_string() },
    };
    let stop = LlmEvent::ContentBlockStop { index: 0 };
    let terminal = LlmEvent::MessageDelta {
        delta: MessageDelta { stop_reason: Some("end_turn".to_string()) },
        usage: None,
    };

    assert!(matches!(start, LlmEvent::ContentBlockStart { .. }));
    assert!(matches!(delta, LlmEvent::ContentBlockDelta { .. }));
    assert!(matches!(stop, LlmEvent::ContentBlockStop { .. }));
    assert!(matches!(terminal, LlmEvent::MessageDelta { .. }));
}
