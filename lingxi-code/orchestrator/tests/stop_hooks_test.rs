//!
//! Exercises the core Stop continuation contract (TS `query.ts:1262-1308`):
//! - a Stop hook `Block` (keep working) continues with the blocking message
//!   appended + `stop_hook_active=true`; consecutive blocks loop up to
//!   `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP` (default 8), then the cap surfaces an
//!   override warning and ends the turn (no infinite loop);
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
            reason: Some("do X first".to_string()),
            // A transcript-only systemMessage that must NOT reach the model — the
            // continuation must come from `reason` (blockingError), never this.
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
async fn stop_block_continues_until_cap_then_overrides() {
    // A perpetually-blocking Stop hook drives consecutive continuations up to
    // the default CLAUDE_CODE_STOP_HOOK_BLOCK_CAP of 8 (binary v2.1.191:
    // `bo=Number.isNaN(jr)?8:jr; if(bo>0&&ar>bo) …yield warning…{reason:"completed"}`),
    // NOT the old boolean guard that stopped after ONE continuation. Blocks 1..8
    // loop (counts 1..8 ≤ 8); the 9th block has next-count 9 > 8 ⇒ the cap fires,
    // surfaces the override warning, and ends the turn. So 9 API calls.
    let api = Arc::new(MockApiClient::new(vec![
        end_turn("1"),
        end_turn("2"),
        end_turn("3"),
        end_turn("4"),
        end_turn("5"),
        end_turn("6"),
        end_turn("7"),
        end_turn("8"),
        end_turn("9"),
    ]));
    let hooks = exec_with(
        Arc::new(StopBlockHandler),
        builtin_hook("stop-block", HookEventType::Stop),
    )
    .await;
    // Build with a CAPTURED output stream so we can assert the override warning.
    let output = Arc::new(MockOutputStream::new());
    let o = Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api.clone(),
        Arc::new(tool_api::registry::ToolRegistry::new()),
        hooks,
        Arc::new(NoOpPermissionGate),
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ));

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { .. }),
        "the cap must end the turn, not loop forever: {outcome:?}"
    );
    assert_eq!(
        api.captured_msgs().await.len(),
        9,
        "a perpetually-blocking Stop hook drives 8 continuations (cap=8), ending on the 9th call"
    );
    // The byte-exact override warning is surfaced once the cap is exceeded
    // (em-dash U+2014; count 9 == the would-be next consecutive block).
    let texts = output.text_events().await;
    assert!(
        texts.iter().any(|t| t == "A hook blocked the turn from ending 9 consecutive times — overriding and ending turn. For Stop/SubagentStop hooks, check stop_hook_active in the input and return success while it's true. Set CLAUDE_CODE_STOP_HOOK_BLOCK_CAP to raise this limit."),
        "the byte-exact override warning must be surfaced when the cap is exceeded: {texts:?}"
    );
    // The blocking REASON (blockingError) is appended as a meta user message,
    // wrapped exactly as claude-code's `getStopHookMessage`
    // (`utils/hooks.ts:1895`): "Stop hook feedback:\n<reason>". The transcript-
    // only systemMessage must NOT appear (it must never reach the model).
    let history = o.session().lock().await.history.clone();
    let appended = history
        .iter()
        .find(|m| m.text_content() == "Stop hook feedback:\ndo X first")
        .expect("the Stop hook feedback (from `reason`) must be appended to history verbatim");
    assert!(
        appended.is_meta(),
        "the appended Stop-hook-feedback user message must be marked meta (TS isMeta:true)"
    );
    assert!(
        !history
            .iter()
            .any(|m| m.text_content().contains("[stop-hook] please continue")),
        "the transcript-only systemMessage must NOT be appended (it must not reach the model)"
    );
}

#[tokio::test]
async fn stop_block_coinciding_with_max_turns_ends_without_feedback() {
    // Binary blocking-branch order: max-turns is checked BEFORE the block cap
    // (`let dt=ie+1,nn=te+1; if(c&&dt>c) return G("tengu_stop_hook_block_count",
    // {count:nn,hit_max_turns:!0,hit_cap:!1}),…,{reason:"max_turns",turnCount:dt}`).
    // So when a blocking Stop hook fires on the turn that hits `max_turns`, the
    // turn ends on the MAX-TURNS terminal and — unlike the `LoopAgain` path — does
    // NOT append the stop-hook feedback message (the binary returns before the
    // append). With `max_turns = 1`: turn 1 runs, the hook blocks, `turn_count(1)
    // >= max_turns(1)` ⇒ end as MaxTurnsReached, exactly ONE API call, and the
    // "Stop hook feedback:…" message must be absent from history.
    let api = Arc::new(MockApiClient::new(vec![end_turn("1"), end_turn("2")]));
    let hooks = exec_with(
        Arc::new(StopBlockHandler),
        builtin_hook("stop-block", HookEventType::Stop),
    )
    .await;
    let mut cfg = OrchestratorConfig::default();
    cfg.max_turns = 1;
    let o = Arc::new(ConversationOrchestrator::new(
        cfg,
        api.clone(),
        Arc::new(tool_api::registry::ToolRegistry::new()),
        hooks,
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ));

    let result = o.run_turn("hi").await;
    assert!(
        matches!(
            result,
            Err(orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 1 })
        ),
        "a blocking Stop hook coinciding with max_turns must end on the max-turns \
         terminal, not the block cap: {result:?}"
    );
    assert_eq!(
        api.captured_msgs().await.len(),
        1,
        "exactly one API call — the blocking branch ends immediately at max_turns"
    );
    let history = o.session().lock().await.history.clone();
    assert!(
        !history
            .iter()
            .any(|m| m.text_content() == "Stop hook feedback:\ndo X first"),
        "the stop-hook feedback must NOT be appended when the block coincides with \
         max_turns (the binary returns before appending): {:?}",
        history.iter().map(|m| m.text_content()).collect::<Vec<_>>()
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
