//! Verify hook integration: a `PreToolUse` hook can block a tool call.
use lingxi_hooks::{
    BuiltinHookHandler, HookContext, HookDecision, HookDefinition, HookEvent, HookEventType,
    HookExecutor, HookExecutorImpl, HookOutcome, HookRegistry, HookResponse, HookResult,
    HookSource,
};
use lingxi_protocol::{HookId, SessionId, ToolUseId};
use lingxi_test_harness::mocks::{MockHttpTransport, MockRuntimeSpawner};
use std::sync::Arc;
use tokio::sync::RwLock;

struct BlockingBuiltin;

#[async_trait::async_trait]
impl BuiltinHookHandler for BlockingBuiltin {
    async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            response: Some(HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("test policy".into()),
                updated_input: None,
                system_message: None,
                attachments: vec![],
                suppress_output: false,
                structured_content: None,
            }),
        }
    }
    fn id(&self) -> &str {
        "blocking-test"
    }
}

#[tokio::test]
async fn pretooluse_block_short_circuits() {
    let reg = Arc::new(RwLock::new(HookRegistry::new()));
    reg.write().await.register(HookDefinition {
        id: HookId::new(),
        name: "block-everything".into(),
        events: vec![HookEventType::PreToolUse],
        if_condition: None,
        executor: HookExecutor::Builtin {
            handler_id: "blocking-test".into(),
        },
        source: HookSource::User,
        blocking: true,
        timeout: None,
        priority: 100,
    });

    let http = Arc::new(MockHttpTransport::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let mut exec = HookExecutorImpl::new(reg.clone(), http, runtime);
    exec.register_builtin(Arc::new(BlockingBuiltin));

    let event = HookEvent::PreToolUse {
        tool_name: "Bash".into(),
        tool_input: serde_json::json!({"command": "rm -rf /"}),
        tool_use_id: ToolUseId::new(),
    };
    let ctx = HookContext {
        session_id: SessionId::nil(),
        agent_id: None,
        cwd: std::path::PathBuf::from("/tmp"),
        ..Default::default()
    };
    let result = exec.execute(event, ctx).await;
    assert_eq!(result.decision, Some(HookDecision::Block));
    assert_eq!(result.reason.as_deref(), Some("test policy"));
}
