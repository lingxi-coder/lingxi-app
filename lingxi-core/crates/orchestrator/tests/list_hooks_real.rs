//! M6-07 — `list_hooks` reads the wired `Arc<RwLock<HookRegistry>>`.

use lingxi_hooks::definition::{HookCondition, HookExecutor, HookSource};
use lingxi_hooks::events::HookEventType;
use lingxi_hooks::{HookDefinition, HookRegistry};
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_protocol::HookId;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

fn hk(
    name: &str,
    event: HookEventType,
    matcher: Option<&str>,
    timeout: Option<Duration>,
) -> HookDefinition {
    HookDefinition {
        id: HookId::new(),
        name: name.into(),
        events: vec![event],
        if_condition: matcher.map(|m| HookCondition {
            pattern: m.into(),
            match_tool_name: true,
            match_input: false,
        }),
        executor: HookExecutor::Builtin {
            handler_id: "noop".into(),
        },
        source: HookSource::User,
        blocking: true,
        timeout,
        priority: 0,
    }
}

fn build_orch() -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(lingxi_tools::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
}

#[tokio::test]
async fn list_hooks_returns_empty_when_no_registry() {
    let orch = Arc::new(build_orch());
    assert!(orch.list_hooks().await.is_empty());
}

#[tokio::test]
async fn list_hooks_returns_one_pretooluse_entry() {
    let mut reg = HookRegistry::new();
    reg.register(hk(
        "./fmt.sh",
        HookEventType::PreToolUse,
        Some("Write|Edit"),
        Some(Duration::from_secs(30)),
    ));
    let reg = Arc::new(RwLock::new(reg));

    let orch = Arc::new(build_orch().with_hook_registry(reg));
    let v = orch.list_hooks().await;
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "./fmt.sh");
    assert_eq!(v[0].event, "PreToolUse");
    assert_eq!(v[0].matcher.as_deref(), Some("Write|Edit"));
    assert_eq!(v[0].timeout_ms, 30_000);
}

#[tokio::test]
async fn list_hooks_default_timeout_is_60000ms() {
    let mut reg = HookRegistry::new();
    reg.register(hk("./x.sh", HookEventType::Stop, None, None));
    let reg = Arc::new(RwLock::new(reg));

    let orch = Arc::new(build_orch().with_hook_registry(reg));
    let v = orch.list_hooks().await;
    assert_eq!(v[0].timeout_ms, 60_000);
    assert_eq!(v[0].event, "Stop");
    assert!(v[0].matcher.is_none());
}
