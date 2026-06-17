//! E2E: a forced `StructuredOutput` tool call captures the model's result into
//! the shared slot, and the 1-turn cap (which `build()` sets for `--json-schema`)
//! keeps the forced tool from looping — the orchestrator consumes exactly ONE
//! model response. This verifies the runtime half of the structured-output
//! mechanism that the print-path `run_structured_output` loop drives.
#![allow(clippy::field_reassign_with_default)]

use llm_client::ContentBlock as LlmContentBlock;
use orchestrator::structured_output::{StructuredOutputSlot, StructuredOutputTool};
use orchestrator::test_support::{
    mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::ToolUseId;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tool_api::registry::ToolRegistry;

#[tokio::test]
async fn forced_structured_output_call_captures_and_does_not_loop() {
    let slot: StructuredOutputSlot = Arc::new(Mutex::new(None));
    // The model (forced via tool_choice in production) calls StructuredOutput with
    // its final result. ONLY ONE response is scripted: if the 1-turn cap failed
    // and the loop continued, run_turn would demand a 2nd response from the now
    // empty MockApiClient queue — so a clean single-call capture proves the cap.
    let r1 = mock_message_response(
        vec![LlmContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input: json!({ "answer": 4 }),
        }],
        Some("tool_use"),
    );
    let api = Arc::new(MockApiClient::new(vec![r1]));
    let mut registry = ToolRegistry::new();
    registry.register_builtin(Arc::new(StructuredOutputTool::new(
        json!({ "type": "object", "required": ["answer"] }),
        slot.clone(),
    )));

    let mut cfg = OrchestratorConfig::default();
    cfg.max_turns = 1; // structured output caps the forced loop at one model call

    let orch = ConversationOrchestrator::new(
        cfg,
        api,
        Arc::new(registry),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    // The 1-turn cap ends the turn after the forced call; run_turn may return the
    // MaxTurns stop, but the tool already captured — that IS the contract.
    let _ = orch.run_turn("answer the question").await;

    assert_eq!(
        slot.lock().unwrap().as_ref(),
        Some(&json!({ "answer": 4 })),
        "the forced StructuredOutput call must capture the model's result into the slot"
    );
}
