//! Integration: `ConversationOrchestrator::run_turn` assembles a
//! system prompt via `assemble_system_prompt` and passes it to the
//! API client.

use lingxi_api_client::types::ContentBlockApi;
use lingxi_orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_tools::registry::ToolRegistry;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn run_turn_passes_assembled_system_prompt_to_api_client() {
    let tmp = TempDir::new().unwrap();
    let api = Arc::new(MockApiClient::new(vec![mock_message_response(
        vec![ContentBlockApi::Text { text: "ok".into() }],
        Some("end_turn"),
    )]));
    let tools = Arc::new(ToolRegistry::new());
    let hooks = lingxi_orchestrator::test_support::noop_hook_executor();
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
    // Assembler always opens with HEADER and ends with FOOTER's last bullet.
    assert!(s.starts_with("You are Claude Code"));
    assert!(s.contains("<env>"));
    assert!(s.ends_with("with a period.\n"));
}
