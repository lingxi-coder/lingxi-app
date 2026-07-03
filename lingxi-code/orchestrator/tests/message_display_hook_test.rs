//! `MessageDisplay` hook firing (#39, R-O1).
//!
//! claude-code fires `MessageDisplay` at the BEGIN of each assistant-message
//! stream — its display state machine `begin(d)` (BIN off 208862320) checks the
//! `MessageDisplay` gate and initializes the per-message flush state
//! `o={apiMessageId:d, messageId:randomUUID(), turnId:r, index:0, …}`, and the
//! payload builder `aAt` (BIN off 205705090) yields
//! `{hook_event_name:"MessageDisplay", turn_id:e.turnId, message_id:e.messageId,
//! index:e.index, final:e.final, delta:e.delta}`.
//!
//! LingXi fires it ONCE per assistant message at its stream begin (the
//! orchestrator-owned point that holds the hook executor + the pre-allocated
//! assistant id), with `index:0`, `final:false`, an empty `delta` (no delta has
//! streamed yet at begin), a fresh per-turn `turn_id`, and the assistant
//! message's bare UUID as `message_id`.
//!
//! Scenarios:
//! 1. A registered `MessageDisplay` hook FIRES exactly once for a single
//!    assistant-message turn, carrying a non-empty `turn_id`, a valid-UUID
//!    `message_id`, `index == 0`, `final == false`, and an empty `delta`.
//! 2. No `MessageDisplay` hook registered ⇒ firing is a strict no-op (and the
//!    turn still completes normally).

use async_trait::async_trait;
use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
use hooks::events::{HookEvent, HookEventType};
use hooks::executor::BuiltinHookHandler;
use hooks::registry::{HookContext, HookRegistry};
use hooks::response::{HookOutcome, HookResult};
use hooks::HookExecutorImpl;
use orchestrator::test_support::{
    MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::test_support_stream::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    text_delta, MockStreamingApiClient,
};
use orchestrator::{scripted, ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use protocol::{HookId, HttpRequest, HttpResponse};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use tool_api::registry::ToolRegistry;
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

/// One captured `MessageDisplay` payload (the wire-relevant fields).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fired {
    turn_id: String,
    message_id: String,
    index: u64,
    is_final: bool,
    delta: String,
}

type FiredLog = Arc<Mutex<Vec<Fired>>>;

/// Records the payload of every `MessageDisplay` event it sees.
struct RecordingHandler {
    log: FiredLog,
}
#[async_trait]
impl BuiltinHookHandler for RecordingHandler {
    fn id(&self) -> &str {
        "record-message-display"
    }
    async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
        if let HookEvent::MessageDisplay {
            turn_id,
            message_id,
            index,
            is_final,
            delta,
        } = event
        {
            self.log.lock().unwrap().push(Fired {
                turn_id: turn_id.clone(),
                message_id: message_id.clone(),
                index: *index,
                is_final: *is_final,
                delta: delta.clone(),
            });
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

fn single_text_turn() -> Vec<llm_client::LlmEvent> {
    scripted![
        message_start("msg_01", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "hi"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ]
}

fn orch_with(
    hooks: Arc<HookExecutorImpl>,
    api: Arc<MockStreamingApiClient>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(Vec::new())),
        api,
        Arc::new(ToolRegistry::new()),
        hooks,
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    )
}

/// (1) A registered `MessageDisplay` hook fires exactly once at the assistant
/// message's stream begin with the byte-faithful at-begin payload.
#[tokio::test]
async fn message_display_fires_once_at_assistant_stream_begin() {
    let log: FiredLog = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    registry.write().await.register(builtin_hook(
        "record-message-display",
        HookEventType::MessageDisplay,
    ));
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    exec.register_builtin(Arc::new(RecordingHandler { log: log.clone() }));

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![single_text_turn()]));
    let orch = orch_with(Arc::new(exec), api);

    let outcome = orch
        .run_turn_streaming("say hi")
        .await
        .expect("turn must succeed");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { turn_count, .. } if turn_count == 1),
        "expected a single-turn EndTurn"
    );

    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        1,
        "MessageDisplay must fire exactly once per assistant message: {seen:?}"
    );
    let f = &seen[0];
    assert!(
        !f.turn_id.is_empty() && uuid::Uuid::parse_str(&f.turn_id).is_ok(),
        "turn_id must be a fresh UUID, got {:?}",
        f.turn_id
    );
    assert!(
        uuid::Uuid::parse_str(&f.message_id).is_ok(),
        "message_id must be the assistant message's BARE uuid (no `msg:` prefix), got {:?}",
        f.message_id
    );
    assert_eq!(f.index, 0, "at-begin index is 0");
    assert!(!f.is_final, "at-begin final is false");
    assert_eq!(f.delta, "", "no delta text has streamed yet at begin");
}

/// (2) No `MessageDisplay` hook registered ⇒ firing is a strict no-op and the
/// turn still completes.
#[tokio::test]
async fn message_display_noop_without_a_registered_hook() {
    let log: FiredLog = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(RwLock::new(HookRegistry::new()));
    // No MessageDisplay hook registered.
    let mut exec = HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
    exec.register_builtin(Arc::new(RecordingHandler { log: log.clone() }));

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![single_text_turn()]));
    let orch = orch_with(Arc::new(exec), api);

    orch.run_turn_streaming("say hi")
        .await
        .expect("turn must succeed");

    assert!(
        log.lock().unwrap().is_empty(),
        "no MessageDisplay hook registered ⇒ firing must be a strict no-op"
    );
}
