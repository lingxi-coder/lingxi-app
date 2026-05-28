//! M6-06 — verify the new `cost_tracker` field + builder are present
//! and wired correctly.

use lingxi_cost::pricing::PricingCatalog;
use lingxi_cost::CostTracker;
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_protocol::SessionId;
use lingxi_tools::registry::ToolRegistry;
use std::sync::Arc;
use tokio::sync::mpsc;

fn make_tracker() -> Arc<CostTracker> {
    let (tx, _rx) = mpsc::channel(8);
    Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ))
}

#[tokio::test]
async fn with_cost_tracker_builder_stores_tracker() {
    let api = Arc::new(MockApiClient::new(Vec::new()));
    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        std::env::temp_dir(),
    )
    .with_cost_tracker(make_tracker());

    assert!(
        orch.has_cost_tracker(),
        "with_cost_tracker did not store the tracker"
    );
}

#[tokio::test]
async fn fresh_orchestrator_has_no_cost_tracker() {
    let api = Arc::new(MockApiClient::new(Vec::new()));
    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        std::env::temp_dir(),
    );

    assert!(
        !orch.has_cost_tracker(),
        "default orchestrator must not carry a tracker"
    );
}
