//! system prompt via `assemble_system_prompt` and passes it to the
//! API client.
use llm_client::ContentBlock as LlmContentBlock;

use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use std::sync::Arc;
use tempfile::TempDir;
use tool_api::registry::ToolRegistry;

#[tokio::test]
async fn run_turn_passes_assembled_system_prompt_to_api_client() {
    let tmp = TempDir::new().unwrap();
    let api = Arc::new(MockApiClient::new(vec![mock_message_response(
        vec![LlmContentBlock::Text {
            text: "ok".into(),
            cache_control: None,
        }],
        Some("end_turn"),
    )]));
    let tools = Arc::new(ToolRegistry::new());
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api.clone(),
        tools,
        hooks,
        perms,
        output,
        memory,
        tmp.path().to_path_buf(),
    );
    orch.run_turn("hi").await.expect("turn");

    let systems = api.captured_systems().await;
    assert_eq!(systems.len(), 1, "exactly one API call");
    let s = systems[0].as_deref().expect("system prompt threaded");
    // Assembler always opens with HEADER and carries the `# Environment` block.
    // There is NO `Notes:` FOOTER on the MAIN prompt (R-P1b).
    // GAP-2: `# Context management` is now the last section (after env block).
    assert!(s.starts_with("You are LingXi"));
    assert!(s.contains("# Environment"));
    assert!(!s.contains("<env>"));
    assert!(!s.contains("Notes:"));
    // The env block is still present; context management follows it.
    assert!(s.contains("available on Opus 4.8/4.7."));
    assert!(s.contains("# Context management"));
    assert!(s.ends_with("you don't need to wrap up early or hand off mid-task."));
}
