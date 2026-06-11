//!
//! Exercises the core Stop continuation contract (TS `query.ts:1262-1308`):
//! - a Stop hook `Block` (keep working) continues ONE extra turn with the
//!   blocking message appended + `stop_hook_active=true`; a SECOND block does
//!   NOT loop forever (re-entry guard);
//! - `preventContinuation` (`continue:false`) terminates as `StopHookPrevented`;
//! - a `UserPromptSubmit` `Block` aborts the turn BEFORE any API call.
use llm_client::ContentBlock as LlmContentBlock;

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
use protocol::{HookId, HttpRequest, HttpResponse};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
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

// ---- Builtin Stop / UserPromptSubmit handlers ----

/// Stop hook that ALWAYS blocks (asks the agent to keep working) and carries a
/// system message.
struct StopBlockHandler;
#[async_trait]
impl BuiltinHookHandler for StopBlockHandler {
    fn id(&self) -> &str {
        "stop-block"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("keep going".to_string()),
            system_message: Some("[stop-hook] please continue".to_string()),
            ..Default::default()
        });
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response,
        }
    }
}

/// Stop hook that requests preventContinuation (`continue:false`).
struct StopPreventHandler;
#[async_trait]
impl BuiltinHookHandler for StopPreventHandler {
    fn id(&self) -> &str {
        "stop-prevent"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
            prevent_continuation: true,
            ..Default::default()
        });
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response,
        }
    }
}

/// `UserPromptSubmit` hook that blocks the prompt.
struct PromptBlockHandler;
#[async_trait]
impl BuiltinHookHandler for PromptBlockHandler {
    fn id(&self) -> &str {
        "prompt-block"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        let response = matches!(event, HookEvent::UserPromptSubmit { .. }).then(|| HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("prompt denied".to_string()),
            ..Default::default()
        });
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response,
        }
    }
}

fn builtin_hook(handler_id: &str, event_type: HookEventType) -> HookDefinition {
    HookDefinition {
        id: HookId::new(),
        name: handler_id.into(),
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

async fn exec_with(
    handler: Arc<dyn BuiltinHookHandler>,
    hook: HookDefinition,
) -> Arc<HookExecutorImpl> {
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    registry.write().await.register(hook);
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    exec.register_builtin(handler);
    Arc::new(exec)
}

fn orch(
    api: Arc<MockApiClient>,
    hooks: Arc<HookExecutorImpl>,
) -> Arc<ConversationOrchestrator> {
    Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        Arc::new(tool_api::registry::ToolRegistry::new()),
        hooks,
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ))
}

fn end_turn(text: &str) -> llm_client::LlmResponse {
    mock_message_response(
        vec![LlmContentBlock::Text { text: text.into(), cache_control: None }],
        Some("end_turn"),
    )
}

#[tokio::test]
async fn stop_block_continues_once_then_guard_stops() {
    // Two end_turn responses. The Stop hook blocks every time. First end_turn:
    // Block + !stop_hook_active → continue (one extra turn). Second end_turn:
    // Block + stop_hook_active → guard → pass → end. Exactly 2 API calls.
    let api = Arc::new(MockApiClient::new(vec![end_turn("one"), end_turn("two")]));
    let hooks = exec_with(
        Arc::new(StopBlockHandler),
        builtin_hook("stop-block", HookEventType::Stop),
    )
    .await;
    let o = orch(api.clone(), hooks);

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { .. }),
        "guard must end the turn, not loop forever: {outcome:?}"
    );
    assert_eq!(
        api.captured_msgs().await.len(),
        2,
        "Stop-block must drive exactly ONE continuation (2 API calls)"
    );
    // The blocking system message was appended as a meta user message.
    let history = o.session().lock().await.history.clone();
    assert!(
        history
            .iter()
            .any(|m| m.text_content().contains("[stop-hook] please continue")),
        "the Stop hook's system message must be appended to history"
    );
}

#[tokio::test]
async fn stop_prevent_continuation_terminates() {
    let api = Arc::new(MockApiClient::new(vec![end_turn("done")]));
    let hooks = exec_with(
        Arc::new(StopPreventHandler),
        builtin_hook("stop-prevent", HookEventType::Stop),
    )
    .await;
    let o = orch(api.clone(), hooks);

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(
        matches!(outcome, ConversationOutcome::StopHookPrevented { .. }),
        "preventContinuation must terminate as StopHookPrevented: {outcome:?}"
    );
    assert_eq!(api.captured_msgs().await.len(), 1, "exactly one API call");
}

#[tokio::test]
async fn user_prompt_submit_block_aborts_before_api() {
    // No API responses needed — a UserPromptSubmit block must abort before any
    // messages_create call.
    let api = Arc::new(MockApiClient::new(vec![]));
    let hooks = exec_with(
        Arc::new(PromptBlockHandler),
        builtin_hook("prompt-block", HookEventType::UserPromptSubmit),
    )
    .await;
    let o = orch(api.clone(), hooks);

    let outcome = o.run_turn("blocked prompt").await.expect("turn ok");
    assert!(
        matches!(outcome, ConversationOutcome::StopHookPrevented { .. }),
        "a UserPromptSubmit block aborts the turn: {outcome:?}"
    );
    assert_eq!(
        api.captured_msgs().await.len(),
        0,
        "no API call may happen when UserPromptSubmit blocks"
    );
}

#[tokio::test]
async fn stop_hook_pass_ends_normally() {
    // A Stop hook that returns no decision (the handler only acts on a
    // DIFFERENT event type) must NOT interfere: normal single-turn end.
    let api = Arc::new(MockApiClient::new(vec![end_turn("fin")]));
    let hooks = exec_with(
        // PromptBlockHandler only responds to UserPromptSubmit, so as a Stop
        // hook it returns no decision → Pass.
        Arc::new(PromptBlockHandler),
        builtin_hook("prompt-block", HookEventType::Stop),
    )
    .await;
    let o = orch(api.clone(), hooks);

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    assert_eq!(api.captured_msgs().await.len(), 1);
}
