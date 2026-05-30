//! `OrchestratorConfig::system_prompt_override = Some(_)` skips the
//! prompt assembler and forwards the literal byte-for-byte.

use lingxi_api_client::types::ContentBlockApi;
use lingxi_orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_tools::registry::ToolRegistry;
use std::sync::Arc;

fn end_turn() -> lingxi_api_client::types::MessageResponse {
    mock_message_response(
        vec![ContentBlockApi::Text { text: "ok".into() }],
        Some("end_turn"),
    )
}

#[tokio::test]
async fn override_is_forwarded_verbatim_bypassing_assembler() {
    let api = Arc::new(MockApiClient::new(vec![end_turn()]));
    let cfg = OrchestratorConfig {
        system_prompt_override: Some("CUSTOM PROMPT — no assembler".into()),
        ..OrchestratorConfig::default()
    };
    let orch = ConversationOrchestrator::new(
        cfg,
        api.clone(),
        Arc::new(ToolRegistry::new()),
        lingxi_orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );
    orch.run_turn("hi").await.expect("turn");

    let systems = api.captured_systems().await;
    assert_eq!(systems.len(), 1);
    assert_eq!(
        systems[0].as_deref(),
        Some("CUSTOM PROMPT — no assembler"),
        "override MUST be forwarded byte-for-byte"
    );
}

#[tokio::test]
async fn default_config_uses_assembler() {
    let api = Arc::new(MockApiClient::new(vec![end_turn()]));
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api.clone(),
        Arc::new(ToolRegistry::new()),
        lingxi_orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );
    orch.run_turn("hi").await.expect("turn");
    let s = api
        .captured_systems()
        .await
        .into_iter()
        .next()
        .flatten()
        .expect("system prompt");
    // Assembler always opens with HEADER.
    assert!(s.starts_with("You are Claude Code"));
    // Override sentinel must NOT appear.
    assert!(!s.contains("CUSTOM PROMPT"));
}
