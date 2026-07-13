//! Verify hook integration: a `PreToolUse` hook can block a tool call.
use hooks::{
    BuiltinHookHandler, HookContext, HookDecision, HookDefinition, HookEvent, HookEventType,
    HookExecutor, HookExecutorImpl, HookOutcome, HookRegistry, HookResponse, HookResult,
    HookSource,
};
use protocol::{HookId, SessionId, ToolUseId};
use std::sync::Arc;
use test_harness::mocks::{MockHttpTransport, MockRuntimeSpawner};
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
                // hooks B4: additive `continue:false` (preventContinuation) wire
                // field; `false` here preserves this mock's prior Block behavior.
                prevent_continuation: false,
                // Hook-payload parity batch (956c136b): additive output fields —
                // None/false for this Block mock.
                display_content: None,
                session_title: None,
                suppress_original_prompt: false,
                // Elicitation batch: additive `elicitation_response` (None here —
                // this mock is a Block policy, not an elicitation responder).
                elicitation_response: None,
                // Hook-firing batch: additive `updated_mcp_tool_output` (None
                // here — this PreToolUse Block mock returns no PostToolUse
                // output replacement).
                updated_mcp_tool_output: None,
                // additionalContext split (082d8283): None — this Block mock
                // emits no model-facing additionalContext.
                additional_context: None,
                // PermissionDenied retry (cb796fad): None — not a retry responder.
                retry: None,
                // #38 all-tools updatedToolOutput: None — Block mock replaces no
                // tool output.
                updated_tool_output: None,
                // #40 terminalSequence: None — Block mock emits no terminal
                // escape sequence.
                terminal_sequence: None,
                // P2-10 watchPaths: None — this PreToolUse Block mock is not a
                // FileChanged/CwdChanged hook, so it adds no watch paths.
                watch_paths: None,
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
        once: false,
        status_message: None,
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
