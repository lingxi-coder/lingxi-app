//! Same conversational outcome whether the turn runs via batched or
//! streaming (M5-04 Task 17).
//!
//! Asserts:
//!   - both paths produce `ConversationOutcome::EndTurn { turn_count: 1, .. }`
//!   - both paths append identical assistant message bodies to the session.

use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
use orchestrator::test_support::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use protocol::{ContentBlock, ConversationMessage};
use std::path::PathBuf;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

fn batched_response(text: &str) -> MessageResponse {
    MessageResponse {
        id: "msg_eq".into(),
        model: "claude-opus-4-7".into(),
        content: vec![ContentBlockApi::Text { text: text.into() }],
        stop_reason: Some("end_turn".into()),
        usage: UsageApi::default(),
    }
}

#[tokio::test]
async fn batched_and_streaming_produce_same_assistant_text() {
    // ── Batched path
    let batched_mock = Arc::new(MockApiClient::new(vec![batched_response("hello world")]));
    let streaming_stub = Arc::new(MockStreamingApiClient::empty());
    let output_b = Arc::new(MockOutputStream::new());
    let orch_b = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched_mock,
        streaming_stub,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output_b.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    );
    let outcome_b = orch_b.run_turn("ping").await.expect("batched");

    // ── Streaming path
    let stream_script = scripted![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "hello world"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let streaming_mock = Arc::new(MockStreamingApiClient::with_turns(vec![stream_script]));
    let batched_stub = Arc::new(MockApiClient::new(Vec::new()));
    let output_s = Arc::new(MockOutputStream::new());
    let orch_s = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched_stub,
        streaming_mock,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output_s.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    );
    let outcome_s = orch_s.run_turn_streaming("ping").await.expect("stream");

    // Same outcome shape.
    match (outcome_b, outcome_s) {
        (
            ConversationOutcome::EndTurn {
                turn_count: tc_b, ..
            },
            ConversationOutcome::EndTurn {
                turn_count: tc_s, ..
            },
        ) => {
            assert_eq!(tc_b, 1);
            assert_eq!(tc_s, 1);
        }
        _ => panic!("unexpected outcome variants"),
    }

    // Same assistant text in the session history.
    let session_b = orch_b.session();
    let session_s = orch_s.session();
    let s_b = session_b.lock().await;
    let s_s = session_s.lock().await;
    let extract_text = |hist: &[ConversationMessage]| -> Option<String> {
        for m in hist {
            if let ConversationMessage::Assistant { content, .. } = m {
                for blk in content {
                    if let ContentBlock::Text { text } = blk {
                        return Some(text.clone());
                    }
                }
            }
        }
        None
    };
    assert_eq!(extract_text(&s_b.history), Some("hello world".into()));
    assert_eq!(extract_text(&s_s.history), Some("hello world".into()));
}
