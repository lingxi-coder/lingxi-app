//!
//! Exercises the core Stop continuation contract (TS `query.ts:1262-1308`):
//! - a Stop hook `Block` (keep working) continues with the blocking message
//!   appended + `stop_hook_active=true`; consecutive blocks loop up to
//!   `LINGXI_STOP_HOOK_BLOCK_CAP` (default 8), then the cap surfaces an
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
use hooks::{HookExecutorImpl, HookPromptRunner, PromptHookError, PromptHookRequest};
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{
    ConversationOrchestrator, ConversationOutcome, OrchestratorConfig, TurnOutcome,
};
use protocol::{HookId, HttpRequest, HttpResponse, SessionId};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use platform_api::{HttpError, HttpTransport, OrchestratorHandle, RuntimeError, RuntimeSpawner};

static GOAL_CAP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---- unused HTTP / Runtime stubs (Builtin hooks never touch them) ----
struct UnusedHttp;
#[async_trait]
impl HttpTransport for UnusedHttp {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::InvalidRequest("unused".into()))
    }
    async fn stream_sse(&self, _req: HttpRequest) -> Result<platform_api::http::SseStream, HttpError> {
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
    ) -> Result<platform_api::BackgroundTaskHandle, RuntimeError> {
        Err(RuntimeError::Internal("unused".into()))
    }
    async fn sleep(&self, _d: Duration) {}
    async fn cancel(&self, _h: &platform_api::BackgroundTaskHandle) -> Result<(), RuntimeError> {
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
        async_rewake: false,
        async_timeout: None,
        rewake_message: None,
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

fn orch(api: Arc<MockApiClient>, hooks: Arc<HookExecutorImpl>) -> Arc<ConversationOrchestrator> {
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

fn orch_with_output(
    api: Arc<MockApiClient>,
    hooks: Arc<HookExecutorImpl>,
    output: Arc<MockOutputStream>,
) -> Arc<ConversationOrchestrator> {
    Arc::new(ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        Arc::new(tool_api::registry::ToolRegistry::new()),
        hooks,
        Arc::new(NoOpPermissionGate),
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ))
}

fn end_turn(text: &str) -> llm_client::LlmResponse {
    mock_message_response(
        vec![LlmContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        Some("end_turn"),
    )
}

fn end_turn_with_usage(text: &str, input: u64, output: u64) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        id: "msg_goal".to_string(),
        model: "claude-opus-4-6".to_string(),
        content: vec![LlmContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        stop_reason: Some("end_turn".to_string()),
        stop_details: None,
        usage: llm_client::Usage {
            billable_tokens: llm_client::TokenUsage {
                input,
                output,
                cache_write: 0,
                cache_read: 0,
                reasoning_output: 0,
            },
            ..llm_client::Usage::default()
        },
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

struct ScriptedPromptRunner {
    seen: StdMutex<Vec<PromptHookRequest>>,
    scripted: StdMutex<VecDeque<Result<String, PromptHookError>>>,
    on_call: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[async_trait]
impl HookPromptRunner for ScriptedPromptRunner {
    async fn run(&self, req: PromptHookRequest) -> Result<String, PromptHookError> {
        self.seen.lock().unwrap().push(req);
        if let Some(on_call) = &self.on_call {
            on_call();
        }
        self.scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(PromptHookError::Query("no scripted prompt response".into())))
    }
}

async fn exec_with_prompt_runner(runner: Arc<dyn HookPromptRunner>) -> Arc<HookExecutorImpl> {
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    Arc::new(
        HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime))
            .with_prompt_runner(runner),
    )
}

#[tokio::test]
async fn stop_block_continues_until_cap_then_overrides() {
    let _env = GOAL_CAP_ENV_LOCK.lock().unwrap();
    let prior_cap = std::env::var("LINGXI_STOP_HOOK_BLOCK_CAP").ok();
    std::env::remove_var("LINGXI_STOP_HOOK_BLOCK_CAP");
    // A perpetually-blocking Stop hook drives consecutive continuations up to
    // the default LINGXI_STOP_HOOK_BLOCK_CAP of 8 (binary v2.1.191:
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
        texts.iter().any(|t| t == "A hook blocked the turn from ending 9 consecutive times — overriding and ending turn. For Stop/SubagentStop hooks, check stop_hook_active in the input and return success while it's true. Set LINGXI_STOP_HOOK_BLOCK_CAP to raise this limit."),
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
    if let Some(prior_cap) = prior_cap {
        std::env::set_var("LINGXI_STOP_HOOK_BLOCK_CAP", prior_cap);
    }
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
    let o = orch(api.clone(), hooks.clone());

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
    let o = orch(api.clone(), hooks.clone());

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
    let o = orch(api.clone(), hooks.clone());

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    assert_eq!(api.captured_msgs().await.len(), 1);
}

#[tokio::test]
async fn stop_goal_registers_named_prompt_hook_and_clears_when_met() {
    let api = Arc::new(MockApiClient::new(vec![end_turn("1"), end_turn("2")]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Ok(r#"{"ok": false, "reason": "not yet"}"#.to_string()),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner.clone()).await;
    let o = orch(api.clone(), hooks.clone());

    o.set_active_goal("ship it").await;
    let current_session = o.current_session_id().await;
    assert!(
        hooks
            .get_session_named_hook(current_session, "__session_goal_stop")
            .await
            .is_some(),
        "setting /goal must register the named session Stop Prompt hook"
    );

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    assert_eq!(api.captured_msgs().await.len(), 2);
    assert!(
        o.get_active_goal().await.is_none(),
        "a completed /goal condition must auto-clear itself"
    );
    assert!(
        hooks
            .get_session_named_hook(current_session, "__session_goal_stop")
            .await
            .is_none(),
        "completed /goal must remove the temporary Stop hook"
    );
    let history = o.session().lock().await.history.clone();
    assert!(
        history
            .iter()
            .any(|m| m.text_content() == "Stop hook feedback:\nnot yet"),
        "an unmet goal should append blocking feedback from the /goal evaluator"
    );
    let seen = runner.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].prompt.contains(r#""hook_event_name":"Stop""#),
        "goal Stop hook must evaluate the Stop payload"
    );
}

#[tokio::test]
async fn goal_status_transcript_records_set_progress_and_one_terminal_achievement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let fs: Arc<dyn platform_api::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
        dir.path().to_path_buf(),
    ));
    let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path.clone(), fs));
    let api = Arc::new(MockApiClient::new(vec![end_turn("1"), end_turn("2")]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Ok(r#"{"ok": false, "reason": "not yet"}"#.to_string()),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner).await;
    let orchestrator = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tool_api::registry::ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer),
    );

    orchestrator.set_active_goal("ship it").await;
    orchestrator.run_turn("hi").await.expect("turn ok");

    let statuses = std::fs::read_to_string(path)
        .expect("goal transcript")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|line| {
            if line["type"] != "attachment" || line["attachment"]["type"] != "goal_status" {
                return None;
            }
            Some((
                line["attachment"]["status"].as_str()?.to_string(),
                line["attachment"]["iterations"].as_u64()?,
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [
            ("set".to_string(), 0),
            ("set".to_string(), 1),
            ("achieved".to_string(), 2)
        ],
        "a successful evaluation must not emit a redundant terminal set record"
    );
}

#[tokio::test]
async fn stop_goal_is_not_capped_by_stop_hook_block_limit() {
    let _env = GOAL_CAP_ENV_LOCK.lock().unwrap();
    let prior_cap = std::env::var("LINGXI_STOP_HOOK_BLOCK_CAP").ok();
    std::env::set_var("LINGXI_STOP_HOOK_BLOCK_CAP", "1");

    let api = Arc::new(MockApiClient::new(vec![
        end_turn("1"),
        end_turn("2"),
        end_turn("3"),
    ]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Ok(r#"{"ok": false, "reason": "still working"}"#.to_string()),
            Ok(r#"{"ok": false, "reason": "still working"}"#.to_string()),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner).await;
    let output = Arc::new(MockOutputStream::new());
    let o = orch_with_output(api.clone(), hooks, output.clone());
    o.set_active_goal("ship it").await;

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    assert_eq!(
        api.captured_msgs().await.len(),
        3,
        "the /goal continuation must survive beyond the generic stop-hook cap and end only once the goal passes"
    );
    assert!(o.get_active_goal().await.is_none());
    assert!(
        !output
            .text_events()
            .await
            .iter()
            .any(|text| text.contains("overriding and ending turn")),
        "goal continuations must not trip the generic stop-hook cap"
    );
    if let Some(prior_cap) = prior_cap {
        std::env::set_var("LINGXI_STOP_HOOK_BLOCK_CAP", prior_cap);
    } else {
        std::env::remove_var("LINGXI_STOP_HOOK_BLOCK_CAP");
    }
}

#[tokio::test]
async fn stop_goal_timeout_keeps_working_until_a_later_success() {
    let api = Arc::new(MockApiClient::new(vec![end_turn("1"), end_turn("2")]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Err(PromptHookError::Timeout(Duration::from_secs(30))),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner).await;
    let o = orch(api.clone(), hooks);
    o.set_active_goal("ship it").await;

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    assert_eq!(api.captured_msgs().await.len(), 2);
    let history = o.session().lock().await.history.clone();
    assert!(
        history
            .iter()
            .any(|m| { m.text_content().contains("timeout after 30000ms") }),
        "timeout feedback must be injected into the next turn"
    );
    assert!(o.get_active_goal().await.is_none());
}

#[tokio::test]
async fn stop_goal_query_error_keeps_working_until_a_later_success() {
    let api = Arc::new(MockApiClient::new(vec![end_turn("1"), end_turn("2")]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Err(PromptHookError::Query("provider 500".into())),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner).await;
    let o = orch(api.clone(), hooks);
    o.set_active_goal("ship it").await;

    let outcome = o.run_turn("hi").await.expect("turn ok");
    assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }));
    let history = o.session().lock().await.history.clone();
    assert!(
        history
            .iter()
            .any(|m| m.text_content().contains("provider 500")),
        "query failures must also feed a reason into the next turn"
    );
    assert!(o.get_active_goal().await.is_none());
}

#[tokio::test]
async fn stop_goal_explicit_cancel_escapes_before_second_goal_check() {
    let api = Arc::new(MockApiClient::new(vec![end_turn("1")]));
    let cancel = CancellationToken::new();
    let cancel_for_runner = cancel.clone();
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([Ok(
            r#"{"ok": false, "reason": "not yet"}"#.to_string(),
        )])),
        on_call: Some(Arc::new(move || cancel_for_runner.cancel())),
    });
    let hooks = exec_with_prompt_runner(runner.clone()).await;
    let o = orch(api.clone(), hooks);
    o.set_active_goal("ship it").await;

    let outcome = o
        .run_turn_with_cancel("hi", cancel)
        .await
        .expect("cancel path returns a turn outcome");
    assert_eq!(outcome, TurnOutcome::Cancelled);
    assert_eq!(api.captured_msgs().await.len(), 1);
    assert_eq!(runner.seen.lock().unwrap().len(), 1);
    assert!(
        o.get_active_goal().await.is_some(),
        "explicit cancel must not auto-clear an unmet goal"
    );
}

#[tokio::test]
async fn stop_goal_hard_budget_escape_stops_before_second_goal_check() {
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_with_usage("1", 1_000, 500),
        end_turn_with_usage("2", 1_000, 500),
    ]));
    let runner = Arc::new(ScriptedPromptRunner {
        seen: StdMutex::new(Vec::new()),
        scripted: StdMutex::new(VecDeque::from([
            Ok(r#"{"ok": false, "reason": "not yet"}"#.to_string()),
            Ok(r#"{"ok": true, "reason": "done"}"#.to_string()),
        ])),
        on_call: None,
    });
    let hooks = exec_with_prompt_runner(runner.clone()).await;
    let (tx, _rx) = mpsc::channel(8);
    let tracker = Arc::new(cost::CostTracker::new(
        SessionId::new(),
        Arc::new(cost::PricingCatalog::builtin_reference()),
        tx,
    ));
    let mut cfg = OrchestratorConfig::default();
    cfg.model = "claude-opus-4-6".into();
    cfg.max_budget_nano_usd = Some(1_000_000);
    let o = Arc::new(
        ConversationOrchestrator::new(
            cfg,
            api.clone(),
            Arc::new(tool_api::registry::ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_cost_tracker(tracker),
    );
    o.set_active_goal("ship it").await;

    let err = o
        .run_turn("hi")
        .await
        .expect_err("budget must stop the loop");
    assert!(
        matches!(
            err,
            orchestrator::OrchestratorError::MaxBudgetReached { .. }
        ),
        "hard budget remains an escape hatch, got {err:?}"
    );
    assert_eq!(api.captured_msgs().await.len(), 1);
    assert_eq!(
        runner.seen.lock().unwrap().len(),
        1,
        "the second goal check must not run once the hard budget is exceeded"
    );
    assert!(o.get_active_goal().await.is_some());
}
