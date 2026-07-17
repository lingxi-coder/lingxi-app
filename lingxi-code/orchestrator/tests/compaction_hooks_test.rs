//! hooks compaction lifecycle — `PreCompact` / `PostCompact` fired around the
//! orchestrator's compaction path.
//!
//! Mirrors the TS firing semantics (`services/compact/compact.ts`):
//! `executePreCompactHooks` runs BEFORE the summary request carrying
//! `trigger = auto|manual` + `custom_instructions`; `executePostCompactHooks`
//! runs AFTER, carrying the produced `compact_summary`.
//!
//! Exercises the proactive auto-compact seam (`maybe_compact_before_call`),
//! which is the always-reachable automatic path: with a compactor wired at a
//! low threshold and an over-threshold history, running a turn fires
//! `PreCompact` (trigger=auto) before the pass and `PostCompact` after it.
//!
//! Scenarios:
//! 1. A registered `PreCompact` hook fires with `reason == "auto"` when the
//!    proactive trigger compacts.
//! 2. A registered `PostCompact` hook fires after, carrying a summary.
//! 3. A `PreCompact` hook that FAILS (and one that returns `Block`) does NOT
//!    break the compaction or the turn — best-effort, like `PostToolUse`.
use llm_client::ContentBlock as LlmContentBlock;

use async_trait::async_trait;
use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
use hooks::events::{HookEvent, HookEventType};
use hooks::executor::BuiltinHookHandler;
use hooks::registry::{HookContext, HookRegistry};
use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
use hooks::HookExecutorImpl;
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::{ConversationMessage, HookId, HttpRequest, HttpResponse, MessageId};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use traits::{HttpError, HttpTransport, OutputEvent, RuntimeError, RuntimeSpawner};

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

// ---- recording builtin handlers ----

/// Shared probe the test reads after the turn.
#[derive(Default)]
struct Probe {
    pre_fired: AtomicBool,
    post_fired: AtomicBool,
    pre_count: AtomicU32,
    /// The `reason` (= wire `trigger`) carried by the last `PreCompact` event.
    pre_trigger: Mutex<Option<String>>,
    /// The `summary` carried by the last `PostCompact` event.
    post_summary: Mutex<Option<String>>,
    /// The `trigger` (`manual`/`auto`) carried by the last `PostCompact` event.
    post_trigger: Mutex<Option<String>>,
}

/// `PreCompact` hook that records the trigger. `fail` makes it return a non-zero
/// exit (best-effort failure); `block` makes it return a `Block` decision —
/// neither may abort compaction.
struct RecordPreCompact {
    probe: Arc<Probe>,
    fail: bool,
    block: bool,
}
#[async_trait]
impl BuiltinHookHandler for RecordPreCompact {
    fn id(&self) -> &str {
        "record-pre-compact"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        if let HookEvent::PreCompact { reason, .. } = event {
            self.probe.pre_fired.store(true, Ordering::SeqCst);
            self.probe.pre_count.fetch_add(1, Ordering::SeqCst);
            *self.probe.pre_trigger.lock().unwrap() = Some(reason.clone());
        }
        HookResult {
            outcome: if self.fail {
                HookOutcome::Error
            } else {
                HookOutcome::Success
            },
            stdout: String::new(),
            stderr: if self.fail {
                "boom".into()
            } else {
                String::new()
            },
            exit_code: if self.fail { Some(1) } else { None },
            response: if self.block {
                Some(HookResponse {
                    decision: Some(HookDecision::Block),
                    reason: Some("nope".into()),
                    ..Default::default()
                })
            } else {
                None
            },
        }
    }
}

/// `PostCompact` hook that records the summary.
struct RecordPostCompact {
    probe: Arc<Probe>,
}
#[async_trait]
impl BuiltinHookHandler for RecordPostCompact {
    fn id(&self) -> &str {
        "record-post-compact"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        if let HookEvent::PostCompact {
            summary, trigger, ..
        } = event
        {
            self.probe.post_fired.store(true, Ordering::SeqCst);
            *self.probe.post_summary.lock().unwrap() = Some(summary.clone());
            *self.probe.post_trigger.lock().unwrap() = Some(trigger.clone());
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

/// Build a hook executor with the supplied (handler, definition) pairs.
async fn hook_executor_with(
    entries: Vec<(Arc<dyn BuiltinHookHandler>, HookDefinition)>,
) -> Arc<HookExecutorImpl> {
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    {
        let mut w = registry.write().await;
        for (_, def) in &entries {
            w.register(def.clone());
        }
    }
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    for (handler, _) in entries {
        exec.register_builtin(handler);
    }
    Arc::new(exec)
}

/// Build an orchestrator with a single `end_turn` mock response, a compactor at
/// `threshold`, and the provided hook executor.
fn make_orch(
    hooks: Arc<HookExecutorImpl>,
    threshold: u64,
) -> (Arc<ConversationOrchestrator>, Arc<MockOutputStream>) {
    let api = Arc::new(MockApiClient::new(vec![mock_message_response(
        vec![LlmContentBlock::Text {
            text: "done".to_string(),
            cache_control: None,
        }],
        Some("end_turn"),
    )]));
    let tools = Arc::new(tool_api::registry::ToolRegistry::new());
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output.clone(),
        memory,
        std::env::temp_dir(),
    )
    .with_compaction(Arc::new(compaction::CompactionOrchestrator::new(threshold)));
    (Arc::new(orch), output)
}

/// Seed `n` filler user messages so the token estimate clears a small threshold
/// (matches `proactive_compaction_test.rs`).
async fn seed_history(orch: &ConversationOrchestrator, n: usize) {
    let session = orch.session();
    let mut s = session.lock().await;
    for i in 0..n {
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            format!("turn-{i} body padded with filler text to push token count up beyond autocompact threshold"),
        ));
    }
}

#[tokio::test]
async fn pre_and_post_compact_hooks_fire_on_proactive_autocompact() {
    let probe = Arc::new(Probe::default());
    let hooks = hook_executor_with(vec![
        (
            Arc::new(RecordPreCompact {
                probe: probe.clone(),
                fail: false,
                block: false,
            }) as Arc<dyn BuiltinHookHandler>,
            builtin_hook("record-pre-compact", HookEventType::PreCompact),
        ),
        (
            Arc::new(RecordPostCompact {
                probe: probe.clone(),
            }) as Arc<dyn BuiltinHookHandler>,
            builtin_hook("record-post-compact", HookEventType::PostCompact),
        ),
    ])
    .await;

    let (orch, output) = make_orch(hooks, 100);
    seed_history(&orch, 60).await;

    orch.run_turn("hello").await.expect("turn ok");

    // Sanity: the proactive trigger actually compacted (boundary marker +
    // CompactionCompleted), so the hooks had something to fire around.
    let events = output.snapshot().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::CompactionCompleted { .. })),
        "compaction must have run for this test to be meaningful"
    );

    // PreCompact fired with trigger=auto, BEFORE the summary (TS firing order).
    assert!(
        probe.pre_fired.load(Ordering::SeqCst),
        "PreCompact hook must fire when the proactive trigger compacts"
    );
    assert_eq!(
        probe.pre_trigger.lock().unwrap().as_deref(),
        Some("auto"),
        "the proactive/automatic compaction path must carry trigger=auto"
    );

    // PostCompact fired AFTER, carrying the produced summary.
    assert!(
        probe.post_fired.load(Ordering::SeqCst),
        "PostCompact hook must fire after compaction is applied"
    );
    assert!(
        probe.post_summary.lock().unwrap().is_some(),
        "PostCompact must carry the compaction summary payload"
    );
    // P2-04: the proactive/automatic path carries trigger=auto, threaded onto
    // the PostCompact event (so a matcher of "auto" would filter to it).
    assert_eq!(
        probe.post_trigger.lock().unwrap().as_deref(),
        Some("auto"),
        "PostCompact from the proactive autocompact path must carry trigger=auto"
    );
}

#[tokio::test]
async fn failing_or_blocking_pre_compact_hook_does_not_break_compaction() {
    // A PreCompact hook that both FAILS (non-zero exit) and returns Block must
    // not abort compaction or the turn — the fire is best-effort, mirroring how
    // PostToolUse hooks are best-effort in the turn loop.
    let probe = Arc::new(Probe::default());
    let hooks = hook_executor_with(vec![(
        Arc::new(RecordPreCompact {
            probe: probe.clone(),
            fail: true,
            block: true,
        }) as Arc<dyn BuiltinHookHandler>,
        builtin_hook("record-pre-compact", HookEventType::PreCompact),
    )])
    .await;

    let (orch, output) = make_orch(hooks, 100);
    seed_history(&orch, 60).await;

    // Turn must still succeed despite the failing/blocking PreCompact hook.
    orch.run_turn("hello").await.expect("turn must not fail");

    // The hook fired ...
    assert!(
        probe.pre_fired.load(Ordering::SeqCst),
        "the (failing) PreCompact hook must still fire"
    );
    // ... and compaction STILL happened (the Block did not short-circuit it).
    let events = output.snapshot().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::CompactionCompleted { .. })),
        "a failing/blocking PreCompact hook must NOT abort compaction"
    );
}
