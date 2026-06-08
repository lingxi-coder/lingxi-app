//! hooks (permission lifecycle) — `PermissionRequest` and `PermissionDenied`
//! fired from the turn loop's tool-dispatch chokepoint (`turn_loop.rs`) around
//! the permission gate.
//!
//! Parity with claude-code:
//! - `executePermissionRequestHooks` (`utils/hooks.ts:4157-4192`) fires when a
//!   tool call needs its permission RESOLVED (`permissions.ts:409`). The `LingXi`
//!   permission seam ([`PermissionGate::check`]) IS that resolution step (a single
//!   allow/deny gate, no separate interactive "ask" branch), so `PermissionRequest`
//!   fires immediately BEFORE consulting the gate.
//! - `executePermissionDeniedHooks` (`utils/hooks.ts:3529-3559`) fires when a
//!   permission decision denies a tool call (`toolExecution.ts:1081`). It fires
//!   at the gate's deny chokepoint, before the error `tool_result` is pushed.
//!
//! Scenarios:
//! 1. A gate that ALLOWS fires `PermissionRequest` (with tool name + input) and
//!    NOT `PermissionDenied`; the tool then runs.
//! 2. A gate that DENIES fires both `PermissionRequest` AND `PermissionDenied`
//!    (the request precedes the denial), with the gate's reason on the
//!    `PermissionDenied` payload.
//! 3. A `PreToolUse` hook 'allow' (which bypasses the gate) fires NEITHER event
//!    — the gate is never consulted.
//! 4. No permission hooks registered → both fires are strict no-ops; the turn is
//!    byte-identical (allow → tool runs; deny → error result, turn completes).

use api_client::types::ContentBlockApi;
use async_trait::async_trait;
use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
use hooks::events::{HookEvent, HookEventType};
use hooks::executor::BuiltinHookHandler;
use hooks::registry::{HookContext, HookRegistry};
use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
use hooks::HookExecutorImpl;
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use protocol::{HookId, HttpRequest, HttpResponse, ToolUseId};
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use tool_api::progress::ToolProgressSender;
use tool_api::registry::ToolRegistry;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use traits::permission_gate::{PermissionDecision, PermissionGate};
use traits::{HttpError, HttpTransport, RuntimeError, RuntimeSpawner};

// ---- unused HTTP / Runtime stubs (Builtin hooks never touch them) ----
struct UnusedHttp;
#[async_trait]
impl HttpTransport for UnusedHttp {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::InvalidRequest("unused".into()))
    }
    async fn stream_sse(&self, _req: HttpRequest) -> Result<traits::http::SseStream, HttpError> {
        Err(HttpError::InvalidRequest("unused".into()))
    }
}
struct UnusedRuntime;
#[async_trait]
impl RuntimeSpawner for UnusedRuntime {
    async fn spawn(
        &self,
        _name: &str,
        _task: Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
    ) -> Result<traits::BackgroundTaskHandle, RuntimeError> {
        Err(RuntimeError::Internal("unused".into()))
    }
    async fn sleep(&self, _d: Duration) {}
    async fn cancel(&self, _h: &traits::BackgroundTaskHandle) -> Result<(), RuntimeError> {
        Ok(())
    }
}

// ---- Permission gate that always DENIES with a fixed reason ----
struct DenyGate {
    reason: &'static str,
}
#[async_trait]
impl PermissionGate for DenyGate {
    async fn check(&self, _name: &str, _input: &serde_json::Value) -> PermissionDecision {
        PermissionDecision::Deny {
            reason: self.reason.into(),
        }
    }
}

// ---- A simple tool that always succeeds ----
struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "Echo"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
            once_cell::sync::Lazy::new(|| json!({"type": "object"}));
        &SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "test".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
        "echo".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        String::new()
    }
    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: tool_api::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        Ok(ToolCallResult {
            data: json!({ "content": format!("ran with {input}") }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ---- Recording hooks ----

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenRequest {
    tool_name: String,
    tool_input: serde_json::Value,
    reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenDenied {
    tool_name: String,
    tool_input: serde_json::Value,
    reason: String,
}

#[derive(Default)]
struct PermLog {
    requests: Vec<SeenRequest>,
    denials: Vec<SeenDenied>,
}

/// A single handler subscribed to BOTH events; the orchestrator routes each
/// event to it via the registry (one builtin definition per event type).
struct PermRecorder {
    log: Arc<Mutex<PermLog>>,
}
#[async_trait]
impl BuiltinHookHandler for PermRecorder {
    fn id(&self) -> &str {
        "record-permission"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        match event {
            HookEvent::PermissionRequest {
                tool_name,
                tool_input,
                reason,
            } => self.log.lock().unwrap().requests.push(SeenRequest {
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                reason: reason.clone(),
            }),
            HookEvent::PermissionDenied {
                tool_name,
                tool_input,
                reason,
                ..
            } => self.log.lock().unwrap().denials.push(SeenDenied {
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                reason: reason.clone(),
            }),
            _ => {}
        }
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: None,
        }
    }
}

/// A `PreToolUse` hook that 'allow's, bypassing the permission gate entirely.
struct PreAllowHook;
#[async_trait]
impl BuiltinHookHandler for PreAllowHook {
    fn id(&self) -> &str {
        "pre-allow"
    }
    async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: Some(HookResponse {
                decision: Some(HookDecision::Approve),
                ..Default::default()
            }),
        }
    }
}

fn builtin_hook(name: &str, handler_id: &str, event_type: HookEventType) -> HookDefinition {
    HookDefinition {
        id: HookId::new(),
        name: name.into(),
        events: vec![event_type],
        if_condition: None,
        executor: DefHookExecutor::Builtin {
            handler_id: handler_id.into(),
        },
        source: HookSource::User,
        blocking: true,
        timeout: None,
        priority: 0,
        once: false,
        status_message: None,
    }
}

/// Registry + executor with the permission recorder subscribed to BOTH
/// `PermissionRequest` and `PermissionDenied`.
async fn exec_permission_recorder(log: Arc<Mutex<PermLog>>) -> Arc<HookExecutorImpl> {
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    {
        let mut r = registry.write().await;
        r.register(builtin_hook(
            "req",
            "record-permission",
            HookEventType::PermissionRequest,
        ));
        r.register(builtin_hook(
            "denied",
            "record-permission",
            HookEventType::PermissionDenied,
        ));
    }
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    exec.register_builtin(Arc::new(PermRecorder { log }));
    Arc::new(exec)
}

fn orch_with_gate(
    api: Arc<MockApiClient>,
    hooks: Arc<HookExecutorImpl>,
    tools: ToolRegistry,
    gate: Arc<dyn PermissionGate>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        Arc::new(tools),
        hooks,
        gate,
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
}

fn two_turn_api(
    tool_use_id: ToolUseId,
    tool_name: &str,
    input: serde_json::Value,
) -> Arc<MockApiClient> {
    Arc::new(MockApiClient::new(vec![
        mock_message_response(
            vec![ContentBlockApi::ToolUse {
                id: tool_use_id,
                name: tool_name.into(),
                input,
            }],
            Some("tool_use"),
        ),
        mock_message_response(
            vec![ContentBlockApi::Text {
                text: "done".into(),
            }],
            Some("end_turn"),
        ),
    ]))
}

#[tokio::test]
async fn allow_gate_fires_permission_request_only() {
    let tool_use_id = ToolUseId::new();
    let api = two_turn_api(tool_use_id, "Echo", json!({ "x": 1 }));
    let log = Arc::new(Mutex::new(PermLog::default()));
    let hooks = exec_permission_recorder(log.clone()).await;
    let mut tools = ToolRegistry::new();
    tools.register_builtin(Arc::new(EchoTool));
    // The default NoOpPermissionGate always allows.
    let orch = orch_with_gate(api, hooks, tools, Arc::new(NoOpPermissionGate));

    let outcome = orch.run_turn("run echo").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));

    let log = log.lock().unwrap();
    assert_eq!(
        log.requests.len(),
        1,
        "PermissionRequest fires once before the gate: {:?}",
        log.requests
    );
    assert_eq!(log.requests[0].tool_name, "Echo");
    assert_eq!(log.requests[0].tool_input, json!({ "x": 1 }));
    assert!(
        log.requests[0].reason.contains("Echo"),
        "reason carries the tool name: {:?}",
        log.requests[0].reason
    );
    assert!(
        log.denials.is_empty(),
        "an allowed tool must NOT fire PermissionDenied: {:?}",
        log.denials
    );
}

#[tokio::test]
async fn deny_gate_fires_request_then_denied() {
    let tool_use_id = ToolUseId::new();
    let api = two_turn_api(tool_use_id, "Echo", json!({ "cmd": "rm -rf /" }));
    let log = Arc::new(Mutex::new(PermLog::default()));
    let hooks = exec_permission_recorder(log.clone()).await;
    let mut tools = ToolRegistry::new();
    tools.register_builtin(Arc::new(EchoTool));
    let gate = Arc::new(DenyGate {
        reason: "policy forbids it",
    });
    let orch = orch_with_gate(api, hooks, tools, gate);

    let outcome = orch.run_turn("run echo").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));

    let log = log.lock().unwrap();
    assert_eq!(
        log.requests.len(),
        1,
        "PermissionRequest fires before the deny: {:?}",
        log.requests
    );
    assert_eq!(
        log.denials.len(),
        1,
        "PermissionDenied fires once on the deny: {:?}",
        log.denials
    );
    assert_eq!(log.denials[0].tool_name, "Echo");
    assert_eq!(log.denials[0].tool_input, json!({ "cmd": "rm -rf /" }));
    assert_eq!(log.denials[0].reason, "policy forbids it");
}

#[tokio::test]
async fn pre_hook_allow_bypasses_gate_and_fires_neither() {
    // A PreToolUse hook 'allow' skips the permission gate entirely, so NEITHER
    // PermissionRequest nor PermissionDenied fires — even against a deny-gate.
    let tool_use_id = ToolUseId::new();
    let api = two_turn_api(tool_use_id, "Echo", json!({}));
    let log = Arc::new(Mutex::new(PermLog::default()));

    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    {
        let mut r = registry.write().await;
        r.register(builtin_hook("pre", "pre-allow", HookEventType::PreToolUse));
        r.register(builtin_hook(
            "req",
            "record-permission",
            HookEventType::PermissionRequest,
        ));
        r.register(builtin_hook(
            "denied",
            "record-permission",
            HookEventType::PermissionDenied,
        ));
    }
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    exec.register_builtin(Arc::new(PreAllowHook));
    exec.register_builtin(Arc::new(PermRecorder { log: log.clone() }));
    let hooks = Arc::new(exec);

    let mut tools = ToolRegistry::new();
    tools.register_builtin(Arc::new(EchoTool));
    // A deny-gate that would deny IF consulted — it must NOT be consulted.
    let gate = Arc::new(DenyGate {
        reason: "would deny",
    });
    let orch = orch_with_gate(api, hooks, tools, gate);

    let outcome = orch.run_turn("run echo").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));

    let log = log.lock().unwrap();
    assert!(
        log.requests.is_empty() && log.denials.is_empty(),
        "a PreToolUse 'allow' bypasses the gate → no permission events: req={:?} denied={:?}",
        log.requests,
        log.denials
    );
}

#[tokio::test]
async fn no_permission_hooks_registered_is_noop() {
    // No PermissionRequest/Denied hooks: a deny-gate still produces the error
    // result and the turn completes (byte-identical to before the fires).
    let tool_use_id = ToolUseId::new();
    let api = two_turn_api(tool_use_id, "Echo", json!({}));
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    let hooks = Arc::new(HookExecutorImpl::new(
        registry,
        Arc::new(UnusedHttp),
        Arc::new(UnusedRuntime),
    ));
    let mut tools = ToolRegistry::new();
    tools.register_builtin(Arc::new(EchoTool));
    let gate = Arc::new(DenyGate { reason: "nope" });
    let orch = orch_with_gate(api.clone(), hooks, tools, gate);

    let outcome = orch.run_turn("run echo").await.expect("turn ok");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { .. }),
        "no-hook deny case must still complete the turn: {outcome:?}"
    );
    assert_eq!(
        api.captured_msgs().await.len(),
        2,
        "the loop still reaches the terminating turn (2 API calls)"
    );
}
