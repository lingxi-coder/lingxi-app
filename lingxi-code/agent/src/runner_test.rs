//! Extracted tests for `agent::runner`.

use super::*;
use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use crate::display::{AgentColor, AgentDisplay};
use async_trait::async_trait;
use lingxi_core::token::Usage;
use protocol::{ContentBlock, ConversationMessage, MessageId, RequestId, ToolUseId};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;

// ---- Scripted loop-mode fixtures -------------------------------------

/// `SubagentApiClient` that hands back a pre-scripted queue of responses,
/// one per `messages_create` call. Counts calls so tests can assert the
/// number of model round-trips (`max_turns` bound, multi-turn loop).
struct MockSubagentApiClient {
    responses: Mutex<VecDeque<Result<llm_client::LlmResponse, llm_client::LlmError>>>,
    calls: AtomicUsize,
    /// The messages the LAST call was given — lets a test assert what the
    /// runner actually seeded the conversation with.
    last_messages: Mutex<Vec<ConversationMessage>>,
}

impl MockSubagentApiClient {
    fn new(responses: Vec<Result<llm_client::LlmResponse, llm_client::LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
            last_messages: Mutex::new(Vec::new()),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn last_messages(&self) -> Vec<ConversationMessage> {
        self.last_messages.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for MockSubagentApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        *self.last_messages.lock().unwrap() = _messages.clone();
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                // Out of scripted responses: a non-terminal, no-tool turn keeps
                // the loop honest (it terminates on empty tool_uses).
                Ok(text_response("(exhausted)", Some("end_turn")))
            })
    }
}

/// `SubagentApiClient` that OVERRIDES the streaming seam with scripted
/// `LlmEvent` sequences (one `Vec` per turn) and makes the non-streaming
/// `messages_create` unreachable — proving the runner drives the loop
/// through `messages_create_stream` + `accumulate_stream`, not the
/// non-streaming fallback.
struct StreamingMockApiClient {
    turns: Mutex<VecDeque<Vec<llm_client::LlmEvent>>>,
    calls: AtomicUsize,
    /// Tools seen on the most recent `messages_create_stream` call — lets a
    /// test prove `ctx.tool_schemas` threads through the seam.
    last_tools: Mutex<Vec<serde_json::Value>>,
    /// The model each call was issued against, in order — lets a test prove a
    /// refusal hop actually re-issued against the fallback.
    models: Mutex<Vec<String>>,
}

impl StreamingMockApiClient {
    fn new(turns: Vec<Vec<llm_client::LlmEvent>>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into_iter().collect()),
            calls: AtomicUsize::new(0),
            last_tools: Mutex::new(Vec::new()),
            models: Mutex::new(Vec::new()),
        })
    }

    fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn last_tools(&self) -> Vec<serde_json::Value> {
        self.last_tools.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for StreamingMockApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        unreachable!("streaming mock must be driven through messages_create_stream")
    }

    async fn messages_create_stream(
        &self,
        model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _effort: Option<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        use futures::StreamExt;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.models.lock().unwrap().push(model.to_string());
        *self.last_tools.lock().unwrap() = tools;
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

static NEAR_LIMIT_WRAP_UP_FLAG_LOCK: RwLock<()> = RwLock::new(());

fn near_limit_wrap_up_write() -> std::sync::RwLockWriteGuard<'static, ()> {
    NEAR_LIMIT_WRAP_UP_FLAG_LOCK
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct NearLimitWrapUpFlagOn;

impl NearLimitWrapUpFlagOn {
    fn set() -> Self {
        ::telemetry::test_set_flag("tengu_vellum_anchor", true);
        Self
    }
}

impl Drop for NearLimitWrapUpFlagOn {
    fn drop(&mut self) {
        ::telemetry::test_clear_flag("tengu_vellum_anchor");
    }
}

struct NearLimitHintApiClient {
    response: llm_client::LlmResponse,
    last_messages: Mutex<Vec<ConversationMessage>>,
    pending_hint: AtomicBool,
    consume_calls: AtomicUsize,
    near_limit_observations: AtomicUsize,
    checkpoint_requests: Mutex<Vec<crate::api::NearLimitCheckpointRequest>>,
}

impl NearLimitHintApiClient {
    fn new(response: llm_client::LlmResponse, pending_hint: bool) -> Arc<Self> {
        Arc::new(Self {
            response,
            last_messages: Mutex::new(Vec::new()),
            pending_hint: AtomicBool::new(pending_hint),
            consume_calls: AtomicUsize::new(0),
            near_limit_observations: AtomicUsize::new(0),
            checkpoint_requests: Mutex::new(Vec::new()),
        })
    }

    fn last_messages(&self) -> Vec<ConversationMessage> {
        self.last_messages.lock().unwrap().clone()
    }

    fn checkpoint_requests(&self) -> Vec<crate::api::NearLimitCheckpointRequest> {
        self.checkpoint_requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for NearLimitHintApiClient {
    fn consume_pending_near_limit_wrap_up_hint(&self) -> bool {
        self.consume_calls.fetch_add(1, Ordering::SeqCst);
        self.pending_hint.swap(false, Ordering::SeqCst)
    }

    fn dispatch_near_limit_checkpoint(&self, request: crate::api::NearLimitCheckpointRequest) {
        self.checkpoint_requests.lock().unwrap().push(request);
    }

    fn record_usage_limit_near_wrap_up(&self) {
        self.near_limit_observations.fetch_add(1, Ordering::SeqCst);
    }

    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        *self.last_messages.lock().unwrap() = messages;
        Ok(self.response.clone())
    }
}

/// Build the `message_start` envelope shared by the streamed-turn builders.
fn ev_message_start() -> llm_client::LlmEvent {
    llm_client::LlmEvent::MessageStart {
        response: Box::new(llm_client::LlmResponse {
            id: "mock".into(),
            model: "mock".into(),
            content: vec![],
            stop_reason: None,
            stop_details: None,
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }),
    }
}

/// One streamed turn carrying a single text block + `stop` reason.
fn streamed_text_turn(text: &str, stop: &str) -> Vec<llm_client::LlmEvent> {
    use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDeltaPayload};
    vec![
        ev_message_start(),
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
        },
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta { text: text.into() },
        },
        LlmEvent::ContentBlockStop { index: 0 },
        LlmEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: Some(stop.into()),
                stop_details: None,
            },
            usage: None,
        },
        LlmEvent::MessageStop,
    ]
}

/// One streamed turn carrying a single `tool_call` block + `stop` reason.
fn streamed_tool_use_turn(name: &str, stop: &str) -> Vec<llm_client::LlmEvent> {
    use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDeltaPayload};
    vec![
        ev_message_start(),
        LlmEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::ToolCall {
                id: ToolUseId::new().to_string(),
                name: name.into(),
                input: serde_json::Value::Null,
            },
        },
        LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::InputJsonDelta {
                partial_json: "{}".into(),
            },
        },
        LlmEvent::ContentBlockStop { index: 0 },
        LlmEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: Some(stop.into()),
                stop_details: None,
            },
            usage: None,
        },
        LlmEvent::MessageStop,
    ]
}

/// `ToolInvoker` that counts invocations and returns a canned value.
struct CountingInvoker {
    calls: AtomicUsize,
}
impl CountingInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl platform_api::ToolInvoker for CountingInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!("tool-output"))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct AbortInvoker;

#[async_trait]
impl platform_api::ToolInvoker for AbortInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        Err(platform_api::tool_invoker::ToolInvokerError::Abort(
            "Agent aborted: too many classifier denials in headless mode".into(),
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct SessionModeRecordingInvoker {
    captured: Mutex<Option<bool>>,
}

impl SessionModeRecordingInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            captured: Mutex::new(None),
        })
    }
}

#[async_trait]
impl platform_api::ToolInvoker for SessionModeRecordingInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        ctx: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        *self.captured.lock().unwrap() = Some(ctx.is_non_interactive_session);
        Ok(serde_json::json!("tool-output"))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `BudgetEnforcerHandle` that reports the budget already exhausted when
/// `exceeded` is set. `check_and_charge` returns
/// `Err(BudgetError::Exceeded { current_nano_usd: 1_500_000_000 })`
/// (i.e. $1.50) when exhausted, else `Ok`. Mirrors the real enforcer's
/// charge-0 consult used by the per-turn budget gate.
struct MockBudget {
    exceeded: bool,
}
#[async_trait]
impl platform_api::budget::BudgetEnforcerHandle for MockBudget {
    async fn check_and_charge(&self, _: u64) -> Result<(), platform_api::budget::BudgetError> {
        if self.exceeded {
            Err(platform_api::budget::BudgetError::Exceeded {
                current_nano_usd: 1_500_000_000,
            })
        } else {
            Ok(())
        }
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        1_500_000_000
    }

    fn max_session_nano_usd(&self) -> Option<u64> {
        Some(1_000_000_000)
    }
}

/// Build an `LlmResponse` carrying a single text block.
fn text_response(text: &str, stop_reason: Option<&str>) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        id: "mock".into(),
        model: "mock".into(),
        content: vec![llm_client::ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        stop_reason: stop_reason.map(str::to_string),
        stop_details: None,
        usage: llm_client::Usage::default(),
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

/// Build an `LlmResponse` carrying one `tool_call` block (+ the given `stop_reason`).
fn tool_use_response(name: &str, stop_reason: Option<&str>) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        id: "mock".into(),
        model: "mock".into(),
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: name.into(),
            input: serde_json::json!({}),
        }],
        stop_reason: stop_reason.map(str::to_string),
        stop_details: None,
        usage: llm_client::Usage::default(),
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

/// Build an `LlmResponse` carrying a text block AND a `tool_call` block
/// (+ the given `stop_reason`) — for the G2 backward-scan test (a turn that
/// surfaces text then a later turn that is tool-only).
fn text_and_tool_response(
    text: &str,
    name: &str,
    stop_reason: Option<&str>,
) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        id: "mock".into(),
        model: "mock".into(),
        content: vec![
            llm_client::ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            },
            llm_client::ContentBlock::ToolCall {
                id: ToolUseId::new().to_string(),
                name: name.into(),
                input: serde_json::json!({}),
            },
        ],
        stop_reason: stop_reason.map(str::to_string),
        stop_details: None,
        usage: llm_client::Usage::default(),
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

/// Build an `LlmResponse` carrying one `tool_call` block AND a non-default
/// `usage` (for the G1 usage-threading test).
fn tool_use_response_with_usage(
    name: &str,
    stop_reason: Option<&str>,
    usage: llm_client::Usage,
) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        usage,
        ..tool_use_response(name, stop_reason)
    }
}

/// `fresh_subagent_ctx` plus a scripted `api_client` (and optional invoker),
/// raising `max_turns` so multi-turn loops are reachable.
fn loop_ctx(
    api_client: Arc<dyn crate::api::SubagentApiClient>,
    tool_invoker: Option<Arc<dyn platform_api::ToolInvoker>>,
    max_turns: u32,
) -> SubagentContext {
    let mut ctx = fresh_subagent_ctx();
    ctx.agent_definition.max_turns = max_turns;
    ctx.api_client = Some(api_client);
    ctx.tool_invoker = tool_invoker;
    ctx
}

/// Build a `SubagentContext` with the minimum fields the runner reads.
fn fresh_subagent_ctx() -> SubagentContext {
    SubagentContext {
        task_registry: None,
        agent_id: AgentId::new(),
        parent_agent_id: None,
        agent_name: None,
        team_name: None,
        agent_definition: AgentDefinition {
            agent_type: "test".into(),
            when_to_use: String::new(),
            tools: AgentToolPolicy::All {
                use_exact_tools: true,
            },
            max_turns: 1,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
            observer: None,
        },
        prompt_messages: vec![],
        fork_context_messages: None,
        allowed_tools: vec![],
        worktree_handle: None,
        cwd: None,
        is_async: false,
        persistent: false,
        can_show_permission_prompts: true,
        session_interactive: None,
        origin_session_id: None,
        mcp_clients: vec![],
        transcript_subdir: "/tmp".into(),
        transcript_fs: None,
        resumed_history: None,
        rendered_system_prompt: Some(Arc::from("")),
        mobile_runtime_environment_reminder: None,
        mobile_runtime_workspace_reminder: None,
        content_replacement_state: None,
        agent_memory: None,
        display: AgentDisplay {
            color: AgentColor::Cyan,
            icon: None,
        },
        model_profile: None,
        api_client: None,
        tool_invoker: None,
        new_diagnostics_source: None,
        tool_schemas: vec![],
        schema: None,
        structured_output_mode: platform_api::subagent_spawn::StructuredOutputMode::Forced,
        budget: None,
        hook_executor: None,
        strict_plugin_only_hooks: false,
        skill_loader: None,
        hook_session_id: protocol::SessionId::nil(),
        hook_cwd: std::path::PathBuf::new(),
        depth: 0,
        observer: None,
        permission_mode_override: None,
        frozen_command_denies: Vec::new(),
        max_output_tokens_per_turn: None,
        max_input_bytes_per_turn: None,
        query_source_label: None,
        correlation_id: None,
        refusal_fallback_chain: Vec::new(),
    }
}

struct OneShotDiagnostics {
    block: Mutex<Option<String>>,
    closed: Arc<AtomicBool>,
}

#[async_trait]
impl platform_api::NewDiagnosticsSource for OneShotDiagnostics {
    async fn take_new_diagnostics_block(&self) -> Option<String> {
        self.block.lock().unwrap().take()
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn write_result_injects_independent_lsp_diagnostics_before_next_round_trip() {
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response("Write", Some("tool_use"))),
        Ok(text_response("fixed", Some("end_turn"))),
    ]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 3);
    let diagnostics_closed = Arc::new(AtomicBool::new(false));
    ctx.new_diagnostics_source = Some(Arc::new(OneShotDiagnostics {
        block: Mutex::new(Some(
            "<new-diagnostics>\napp/app.jsx: [Line 2:1] unknownName\n</new-diagnostics>".into(),
        )),
        closed: diagnostics_closed.clone(),
    }));
    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);

    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert!(api.last_messages().iter().any(|message| {
        matches!(message, ConversationMessage::User { content, is_meta: true, .. }
        if content.iter().any(|block| matches!(block,
            ContentBlock::Text { text, .. }
                if text.contains("<system-reminder>\n<new-diagnostics>")
                    && text.contains("unknownName")
        )))
    }));
    assert!(
        diagnostics_closed.load(Ordering::SeqCst),
        "the agent terminal path must release its workspace diagnostics source"
    );
}

/// Drain the `SubagentEvent` receiver into a `Vec`.
async fn drain(mut rx: mpsc::Receiver<SubagentEvent>) -> Vec<SubagentEvent> {
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn subagent_near_limit_wrap_up_is_exact_and_dispatches_once_through_wrapper() {
    let _flag_lock = near_limit_wrap_up_write();
    let _flag = NearLimitWrapUpFlagOn::set();

    let tempdir = tempfile::tempdir().expect("tempdir");
    let api = NearLimitHintApiClient::new(text_response("done", Some("end_turn")), true);
    let wrapped = Arc::new(crate::api::WorkflowWatchdogApiClient::new(
        api.clone(),
        platform_api::WorkflowQueryWatchdog {
            stall_timeout_ms: 1_000,
            max_retries: 0,
        },
        Vec::new(),
    ));
    let mut ctx = loop_ctx(wrapped, None, 1);
    ctx.depth = 1;
    ctx.session_interactive = Some(true);
    ctx.is_async = true;
    ctx.hook_session_id = protocol::SessionId::new();
    ctx.hook_cwd = tempdir.path().to_path_buf();

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(16);
    run_subagent(ctx.clone(), event_rx, out_tx).await;
    let events = drain(out_rx).await;

    let messages = api.last_messages();
    let note_count = messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                ConversationMessage::User { content, is_meta: true, .. }
                    if matches!(content.as_slice(), [ContentBlock::Text { text, .. }]
                        if text == NEAR_LIMIT_WRAP_UP_NOTE)
            )
        })
        .count();
    assert_eq!(note_count, 1, "the model-visible note is one-shot");
    let last = messages.last().expect("near-limit note");
    assert!(matches!(
        last,
        ConversationMessage::User { content, is_meta: true, .. }
            if matches!(content.as_slice(), [ContentBlock::Text { text, .. }]
                if text == NEAR_LIMIT_WRAP_UP_NOTE)
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SubagentEvent::Message { message, .. }
                if serde_json::from_value::<ConversationMessage>(message.clone()).is_ok_and(|message|
                    matches!(message,
                        ConversationMessage::User { content, is_meta: true, .. }
                            if matches!(content.as_slice(), [ContentBlock::Text { text, .. }]
                                if text == NEAR_LIMIT_WRAP_UP_NOTE)))))
            .count(),
        1,
        "the note is yielded exactly once"
    );

    assert!(api.consume_calls.load(Ordering::SeqCst) >= 1);
    assert_eq!(api.near_limit_observations.load(Ordering::SeqCst), 1);
    let requests = api.checkpoint_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0],
        crate::api::NearLimitCheckpointRequest {
            session_id: ctx.hook_session_id,
            cwd: tempdir.path().to_path_buf(),
            non_interactive: false,
        },
        "an async child still belongs to its interactive parent session"
    );
}

#[tokio::test]
async fn disabled_near_limit_flag_consumes_and_drops_the_pending_hint() {
    let _flag_lock = near_limit_wrap_up_write();
    ::telemetry::test_clear_flag("tengu_vellum_anchor");
    let api = NearLimitHintApiClient::new(text_response("done", Some("end_turn")), true);
    let mut ctx = loop_ctx(api.clone(), None, 1);
    ctx.depth = 1;

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert!(api.consume_calls.load(Ordering::SeqCst) >= 1);
    assert_eq!(api.near_limit_observations.load(Ordering::SeqCst), 0);
    assert!(!api.pending_hint.load(Ordering::SeqCst));
    assert!(api.checkpoint_requests().is_empty());
    assert!(!api.last_messages().iter().any(|message| matches!(
        message,
        ConversationMessage::User { content, .. }
            if content.iter().any(|block| matches!(block,
                ContentBlock::Text { text, .. } if text == NEAR_LIMIT_WRAP_UP_NOTE
            ))
    )));
}

#[tokio::test]
async fn main_depth_does_not_consume_the_subagent_near_limit_hint() {
    let _flag_lock = near_limit_wrap_up_write();
    let _flag = NearLimitWrapUpFlagOn::set();
    let api = NearLimitHintApiClient::new(text_response("done", Some("end_turn")), true);
    let ctx = loop_ctx(api.clone(), None, 1);

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(api.consume_calls.load(Ordering::SeqCst), 0);
    assert_eq!(api.near_limit_observations.load(Ordering::SeqCst), 0);
    assert!(api.pending_hint.load(Ordering::SeqCst));
    assert!(api.checkpoint_requests().is_empty());
}

#[tokio::test]
async fn near_limit_noninteractive_subagent_gets_hint_but_does_not_dispatch_checkpoint() {
    let _flag_lock = near_limit_wrap_up_write();
    let _flag = NearLimitWrapUpFlagOn::set();
    let api = NearLimitHintApiClient::new(text_response("done", Some("end_turn")), true);
    let mut ctx = loop_ctx(api.clone(), None, 1);
    ctx.depth = 1;
    ctx.session_interactive = Some(false);

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert!(api.last_messages().iter().any(|message| matches!(
        message,
        ConversationMessage::User { content, is_meta: true, .. }
            if content.iter().any(|block| matches!(block,
                ContentBlock::Text { text, .. } if text == NEAR_LIMIT_WRAP_UP_NOTE
            ))
    )));
    assert_eq!(api.near_limit_observations.load(Ordering::SeqCst), 1);
    assert!(api.checkpoint_requests().is_empty());
}

#[tokio::test]
async fn workflow_watchdog_times_out_stream_open() {
    let policy = platform_api::WorkflowQueryWatchdog {
        stall_timeout_ms: 10,
        max_retries: 0,
    };
    let result = await_workflow_query_phase::<(), _>(
        async {
            std::future::pending::<()>().await;
            Ok(())
        },
        Some(policy),
        "opening the response stream",
    )
    .await;
    assert!(matches!(result, Err(ref error) if is_workflow_watchdog_timeout(error)));
}

#[tokio::test]
async fn workflow_watchdog_times_out_before_first_event() {
    use futures::StreamExt;
    let stream =
        futures::stream::pending::<Result<llm_client::LlmEvent, llm_client::LlmError>>().boxed();
    let mut watched = with_workflow_stream_watchdog(
        stream,
        Some(platform_api::WorkflowQueryWatchdog {
            stall_timeout_ms: 10,
            max_retries: 0,
        }),
    );
    let event = watched.next().await.expect("watchdog error event");
    assert!(matches!(event, Err(ref error) if is_workflow_watchdog_timeout(error)));
    assert!(
        watched.next().await.is_none(),
        "timeout terminates the stream"
    );
}

#[tokio::test]
async fn workflow_watchdog_resets_between_events_and_has_no_total_deadline() {
    use futures::StreamExt;
    let stream = futures::stream::unfold(0_u8, |index| async move {
        if index == 4 {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(8)).await;
        Some((Ok(ev_message_start()), index + 1))
    })
    .boxed();
    let watched = with_workflow_stream_watchdog(
        stream,
        Some(platform_api::WorkflowQueryWatchdog {
            stall_timeout_ms: 20,
            max_retries: 0,
        }),
    );
    let events = watched.collect::<Vec<_>>().await;
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(Result::is_ok));
    // Four 8ms waits exceed the 20ms idle threshold in aggregate. Success
    // proves the deadline resets after every event instead of wrapping the
    // whole accumulator/model round-trip.
}

struct SlowInvoker {
    delay: std::time::Duration,
}

#[async_trait]
impl platform_api::ToolInvoker for SlowInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        tokio::time::sleep(self.delay).await;
        Ok(serde_json::json!("slow-tool-finished"))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn workflow_watchdog_does_not_cover_tool_execution() {
    let inner = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Read", "tool_use"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let api: Arc<dyn crate::api::SubagentApiClient> =
        Arc::new(crate::api::WorkflowWatchdogApiClient::new(
            inner,
            platform_api::WorkflowQueryWatchdog {
                stall_timeout_ms: 10,
                max_retries: 0,
            },
            Vec::new(),
        ));
    let invoker: Arc<dyn platform_api::ToolInvoker> = Arc::new(SlowInvoker {
        delay: std::time::Duration::from_millis(35),
    });
    let ctx = loop_ctx(api, Some(invoker), 3);
    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;
    let events = drain(out_rx).await;
    assert!(events.iter().any(
        |event| matches!(event, SubagentEvent::Completed { result, .. } if result["text"] == "done")
    ));
    assert!(!events
        .iter()
        .any(|event| matches!(event, SubagentEvent::Failed { .. })));
}

struct TimeoutAfterToolApi {
    calls: AtomicUsize,
}

#[async_trait]
impl crate::api::SubagentApiClient for TimeoutAfterToolApi {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok(text_and_tool_response("partial", "Read", Some("tool_use")))
        } else {
            std::future::pending().await
        }
    }
}

#[derive(Default)]
struct RetryObserver {
    attempts: Mutex<Vec<u32>>,
    reasons: Mutex<Vec<String>>,
}

#[async_trait]
impl platform_api::SubagentSpawnObserver for RetryObserver {
    async fn on_event(&self, event: platform_api::SubagentObservation) {
        if let platform_api::SubagentObservation::Retry {
            attempt, reason, ..
        } = event
        {
            self.attempts.lock().unwrap().push(attempt);
            self.reasons.lock().unwrap().push(reason);
        }
    }
}

#[tokio::test]
async fn workflow_watchdog_retries_five_times_then_fails_without_partial_salvage() {
    let inner = Arc::new(TimeoutAfterToolApi {
        calls: AtomicUsize::new(0),
    });
    let observer = Arc::new(RetryObserver::default());
    let observer_dyn: Arc<dyn platform_api::SubagentSpawnObserver> = observer.clone();
    let api: Arc<dyn crate::api::SubagentApiClient> =
        Arc::new(crate::api::WorkflowWatchdogApiClient::new(
            inner.clone(),
            platform_api::WorkflowQueryWatchdog {
                stall_timeout_ms: 10,
                max_retries: 5,
            },
            vec![observer_dyn],
        ));
    let ctx = loop_ctx(api, Some(CountingInvoker::new()), 3);
    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;
    let events = drain(out_rx).await;

    assert_eq!(inner.calls.load(Ordering::SeqCst), 7);
    assert_eq!(*observer.attempts.lock().unwrap(), vec![2, 3, 4, 5, 6]);
    assert!(observer
        .reasons
        .lock()
        .unwrap()
        .iter()
        .all(|reason| reason.contains("opening the response stream")));
    assert!(events.iter().any(|event| matches!(
        event,
        SubagentEvent::Failed { error, .. }
            if error.contains("workflow query timeout")
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SubagentEvent::Completed { .. })),
        "watchdog exhaustion must not salvage prior partial text as success"
    );
}

/// Build a minimal assistant message with a single text block.
fn assistant_text(text: &str) -> ConversationMessage {
    ConversationMessage::Assistant {
        id: MessageId::new(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        stop_reason: Some("end_turn".into()),
    }
}

#[tokio::test]
async fn run_subagent_emits_message_on_api_stream_end_then_completed() {
    let ctx = fresh_subagent_ctx();
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    // Drive the runner from a request-response cycle the M1 reducer
    // accepts: UserMessage -> ApiStreamStart -> ApiStreamEnd -> EOF.
    // The runner is expected to surface the assistant Message on
    // ApiStreamEnd and finally Completed (Killed only on user_exit-prefixed
    // terminal — see Task 1 step 3 notes).
    let req = RequestId::new();
    let msg_id = MessageId::new();
    let final_msg = assistant_text("hello world");

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    event_tx
        .send(lingxi_core::Event::UserMessage {
            message_id: msg_id,
            request_id: req,
            content: "hi".into(),
        })
        .await
        .unwrap();
    event_tx
        .send(lingxi_core::Event::ApiStreamStart { request_id: req })
        .await
        .unwrap();
    event_tx
        .send(lingxi_core::Event::ApiStreamEnd {
            request_id: req,
            final_message: final_msg.clone(),
            usage: Usage::default(),
        })
        .await
        .unwrap();
    // Close the event channel — the runner should NOT treat clean close
    // as a failure when it has already produced a Message; instead it
    // emits a Completed terminal (graceful end on EOF).
    drop(event_tx);

    handle.await.unwrap();
    let evs = drain(out_rx).await;

    // At least one Message and exactly one terminal Completed.
    let message_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Message { .. }))
        .count();
    let completed_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
        .count();
    let failed_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Failed { .. }))
        .count();

    assert_eq!(
        message_count, 1,
        "exactly one Message emitted on ApiStreamEnd; got events: {evs:?}"
    );
    assert_eq!(
        completed_count, 1,
        "exactly one Completed terminal; got events: {evs:?}"
    );
    assert_eq!(
        failed_count, 0,
        "no Failed events expected; got events: {evs:?}"
    );

    // The Message payload must be JSON-equivalent to the serialized final_message.
    let msg_payload = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Message {
                agent_id: aid,
                message,
            } => Some((*aid, message.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        msg_payload.0, agent_id,
        "Message agent_id matches ctx.agent_id"
    );
    let expected = serde_json::to_value(&final_msg).unwrap();
    assert_eq!(
        msg_payload.1, expected,
        "Message payload byte-equals serialized final_message"
    );
}

#[tokio::test]
async fn run_subagent_emits_killed_on_user_exit_terminal() {
    let ctx = fresh_subagent_ctx();
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Drive directly to terminal via UserExit. The fast-path in the
    // runner short-circuits to Killed on this input event.
    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    drop(event_tx);

    handle.await.unwrap();
    let evs = drain(out_rx).await;

    let killed_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Killed { .. }))
        .count();
    let completed_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
        .count();

    // Exactly one Killed, no Completed (Killed is the terminal here).
    assert_eq!(
        killed_count, 1,
        "exactly one Killed expected on UserExit; got events: {evs:?}"
    );
    assert_eq!(
        completed_count, 0,
        "no Completed expected when terminal is UserExit; got events: {evs:?}"
    );

    // Killed carries the agent id.
    let killed_aid = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Killed { agent_id } => Some(*agent_id),
            _ => None,
        })
        .unwrap();
    assert_eq!(killed_aid, agent_id);
}

#[tokio::test]
async fn run_subagent_emits_killed_on_user_interrupt_terminal() {
    // UserInterrupt does NOT terminate via the M1 reducer (catch-all),
    // so without the fast-path this would emit Failed (channel close
    // before terminal). The fast-path makes it produce Killed instead.
    let ctx = fresh_subagent_ctx();
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    event_tx
        .send(lingxi_core::Event::UserInterrupt)
        .await
        .unwrap();
    drop(event_tx);

    handle.await.unwrap();
    let evs = drain(out_rx).await;

    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Killed { agent_id: aid } if *aid == agent_id)),
        "expected exactly one Killed on UserInterrupt; got events: {evs:?}"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "no Failed expected on UserInterrupt; got events: {evs:?}"
    );
}

#[tokio::test]
async fn run_subagent_emits_failed_on_eof_before_any_work() {
    let ctx = fresh_subagent_ctx();
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Drop the sender immediately — runner sees event_rx close on the
    // very first recv, with no prior Message or Terminated emission.
    drop(event_tx);

    handle.await.unwrap();
    let evs = drain(out_rx).await;

    let failed = evs.iter().find_map(|e| match e {
        SubagentEvent::Failed {
            agent_id: aid,
            error,
            ..
        } => Some((*aid, error.clone())),
        _ => None,
    });
    assert!(
        failed.is_some(),
        "exactly one Failed expected on premature EOF; got events: {evs:?}"
    );
    let (failed_aid, failed_err) = failed.unwrap();
    assert_eq!(failed_aid, agent_id);
    assert_eq!(
        failed_err, "run_subagent: event channel closed without terminal state",
        "byte-locked error message"
    );

    let completed_count = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
        .count();
    assert_eq!(
        completed_count, 0,
        "no Completed expected; got events: {evs:?}"
    );
}

// ---- Loop-mode tests (api_client = Some) -----------------------------

/// Pull the single `Completed.result` payload (panics if none / many).
fn one_completed(evs: &[SubagentEvent]) -> serde_json::Value {
    let mut found = evs.iter().filter_map(|e| match e {
        SubagentEvent::Completed { result, .. } => Some(result.clone()),
        _ => None,
    });
    let r = found.next().expect("exactly one Completed");
    assert!(found.next().is_none(), "more than one Completed: {evs:?}");
    r
}

#[tokio::test]
async fn loop_single_end_turn_completes_with_aggregated_text() {
    // One turn: end_turn, no tools. Asserts the terminal-text path and the
    // `{text, stop_reason}` result shape, and that the model was called once.
    let api = MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 1, "exactly one model round-trip");
    let result = one_completed(&evs);
    assert_eq!(result["text"], "final answer");
    assert_eq!(result["stop_reason"], "end_turn");
}

#[tokio::test]
async fn loop_completed_result_carries_claude_content_array() {
    // #3: the terminal result carries claude's `content` array of text
    // blocks (one per text block), not only the joined `text` string.
    let api = MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
    let ctx = loop_ctx(api.clone(), None, 4);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let result = one_completed(&evs);
    assert_eq!(
        result["content"],
        serde_json::json!([{ "type": "text", "text": "final answer" }]),
        "result carries claude content[] array"
    );
    assert_eq!(
        result["text"], "final answer",
        "legacy `text` still present"
    );
}

#[tokio::test]
async fn loop_g2_backward_scan_recovers_text_from_earlier_turn() {
    // G2 (agentToolUtils.ts:304-317): when the FINAL assistant turn is
    // tool-only (no text), the result content falls back to the most recent
    // assistant message that HAS text. Turn 1: text "partial" + a tool_use
    // (continues). Turn 2: tool-only with end_turn (terminates, no text in
    // the final block) → content must be "partial" from turn 1.
    let api = MockSubagentApiClient::new(vec![
        Ok(text_and_tool_response("partial", "Read", Some("tool_use"))),
        Ok(tool_use_response("Read", Some("end_turn"))),
    ]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let result = one_completed(&evs);
    // The final turn was tool-only; the backward scan recovered turn 1's text.
    assert_eq!(
        result["content"],
        serde_json::json!([{ "type": "text", "text": "partial" }]),
        "backward scan recovers the most recent assistant text; got {result:?}"
    );
    assert_eq!(result["text"], "partial");
}

#[tokio::test]
async fn schema_forces_structured_output_and_returns_the_tool_input() {
    // With `ctx.schema` set, the runner injects+forces a `StructuredOutput`
    // tool; the model's tool input IS the run's result (it is NOT dispatched).
    let structured = serde_json::json!({ "answer": 42, "ok": true });
    let resp = llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input: structured.clone(),
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    };
    let api = MockSubagentApiClient::new(vec![Ok(resp)]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api, Some(invoker.clone()), 4);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    // The Completed result IS the captured StructuredOutput tool input.
    assert_eq!(one_completed(&evs), structured);
}

/// A StructuredOutput call whose input FAILS the schema is fed back as an
/// `is_error` result (the model retries); a later VALID call is captured.
#[tokio::test]
async fn schema_invalid_output_retried_then_captured() {
    let so = |input: serde_json::Value| llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input,
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    };
    let valid = serde_json::json!({ "answer": 42 });
    let api = MockSubagentApiClient::new(vec![
        Ok(so(serde_json::json!({ "answer": "not-an-int" }))), // fails: wrong type
        Ok(so(valid.clone())),                                 // passes
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 6);
    ctx.schema = Some(
        r#"{"type":"object","required":["answer"],"properties":{"answer":{"type":"integer"}}}"#
            .to_string(),
    );
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(one_completed(&evs), valid, "the valid retry is captured");
    assert_eq!(
        api.call_count(),
        2,
        "the model retried once after the failure"
    );
}

/// cc 2.1.196 (M9): a schema-rejected StructuredOutput attempt does NOT
/// render beside its retry — no duplicate recap. The result surface a
/// workflow/Agent consumer renders is the terminal `Completed.result`
/// (`PoolSubagentSpawner::spawn` ignores Message events): after a rejected
/// attempt + a valid retry there is EXACTLY ONE Completed, its payload is the
/// retry's, and the rejected payload appears nowhere in it.
#[tokio::test]
async fn schema_rejected_attempt_is_not_surfaced_beside_its_retry() {
    let so = |input: serde_json::Value| llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input,
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    };
    let rejected = serde_json::json!({ "answer": "REJECTED-SENTINEL" });
    let valid = serde_json::json!({ "answer": 7 });
    let api = MockSubagentApiClient::new(vec![
        Ok(so(rejected.clone())), // schema-rejected attempt
        Ok(so(valid.clone())),    // its retry
    ]);
    let mut ctx = loop_ctx(api, Some(CountingInvoker::new()), 6);
    ctx.schema = Some(
        r#"{"type":"object","required":["answer"],"properties":{"answer":{"type":"integer"}}}"#
            .to_string(),
    );
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    // Exactly one terminal Completed (one_completed asserts uniqueness) whose
    // payload is the RETRY's — the rejected attempt is suppressed from the
    // surfaced result, not rendered beside it.
    let result = one_completed(&evs);
    assert_eq!(result, valid, "the surfaced recap is the valid retry only");
    assert!(
        !result.to_string().contains("REJECTED-SENTINEL"),
        "rejected payload must not leak into the surfaced result: {result}"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "a recovered retry is not a failure; got: {evs:?}"
    );
}

/// Repeated schema-invalid StructuredOutput calls exhaust the retry cap (5)
/// and abort with the byte-exact retry-cap-exceeded message.
#[tokio::test]
async fn schema_retry_cap_exceeded_aborts() {
    let bad_so = || llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input: serde_json::json!({ "answer": "still-wrong" }),
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    };
    // 5 failing calls → kn reaches the default cap (5) → abort.
    let api = MockSubagentApiClient::new((0..5).map(|_| Ok(bad_so())).collect());
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api, Some(invoker), 10);
    ctx.schema =
        Some(r#"{"type":"object","properties":{"answer":{"type":"integer"}}}"#.to_string());
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    // Five invalid schema turns each surface multiple non-terminal events
    // before the runner returns. Keep the fixture from back-pressuring the
    // runner while this synchronous test waits to drain after completion.
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(32);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let err = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("a Failed event");
    assert_eq!(
        err,
        "agent({schema}): StructuredOutput retry cap (5) exceeded \u{2014} 5 failed calls with no valid output"
    );
}

/// When the model ends turns WITHOUT calling StructuredOutput, the runner
/// nudges up to 2 times then aborts with the byte-exact "completed without
/// calling" message.
#[tokio::test]
async fn schema_no_call_nudges_twice_then_aborts() {
    // 3 end_turn text turns: turn 1 → nudge, turn 2 → nudge, turn 3 → abort.
    let api = MockSubagentApiClient::new(vec![
        Ok(text_response("no tool here", Some("end_turn"))),
        Ok(text_response("still none", Some("end_turn"))),
        Ok(text_response("nope", Some("end_turn"))),
    ]);
    let invoker = CountingInvoker::new();
    let api2 = api.clone();
    let mut ctx = loop_ctx(api, Some(invoker), 10);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let err = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("a Failed event");
    assert_eq!(
        err,
        "agent({schema}): subagent completed without calling StructuredOutput (after in-conversation nudge)"
    );
    // 3 round-trips: the original turn + 2 nudge re-runs.
    assert_eq!(api2.call_count(), 3);
}

/// `SubagentApiClient` used ONLY through the `_opts` streaming seam
/// (`messages_create_stream_in_opts` / `messages_create_stream_forced_in_opts`
/// — the two methods `run_subagent_loop` actually calls). Records, per
/// round-trip, whether the runner went through the FORCED variant (`true`) or
/// the plain/auto variant (`false`) — the only way to observe `tool_choice`
/// from outside the wire, since both variants funnel unrelated calls through
/// the same underlying `LlmResponse` script.
struct StructuredOutputModeCapturingApiClient {
    responses: Mutex<VecDeque<llm_client::LlmResponse>>,
    forced_calls: Mutex<Vec<bool>>,
    /// The `messages` argument of every round-trip, in call order — lets a
    /// test inspect the REQUEST shape the runner built for a given turn
    /// (e.g. whether its last message is user- or assistant-authored), not
    /// just whether `tool_choice` was forced.
    messages_seen: Mutex<Vec<Vec<ConversationMessage>>>,
}

impl StructuredOutputModeCapturingApiClient {
    fn new(responses: Vec<llm_client::LlmResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            forced_calls: Mutex::new(Vec::new()),
            messages_seen: Mutex::new(Vec::new()),
        })
    }

    fn forced_calls(&self) -> Vec<bool> {
        self.forced_calls.lock().unwrap().clone()
    }

    fn messages_seen(&self) -> Vec<Vec<ConversationMessage>> {
        self.messages_seen.lock().unwrap().clone()
    }

    fn next_stream(
        &self,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        use futures::StreamExt;
        let resp = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text_response("(exhausted)", Some("end_turn")));
        let events = crate::accumulator::response_to_stream_events(resp);
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for StructuredOutputModeCapturingApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        unreachable!("driven only through the _opts streaming seam")
    }

    async fn messages_create_stream_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _effort: Option<serde_json::Value>,
        _opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.forced_calls.lock().unwrap().push(false);
        self.messages_seen.lock().unwrap().push(messages);
        self.next_stream()
    }

    async fn messages_create_stream_forced_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _forced_tool: Option<&str>,
        _effort: Option<serde_json::Value>,
        _opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.forced_calls.lock().unwrap().push(true);
        self.messages_seen.lock().unwrap().push(messages);
        self.next_stream()
    }
}

/// Build an `LlmResponse` carrying a valid `StructuredOutput` tool call.
fn structured_output_call_response(input: serde_json::Value) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input,
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    }
}

/// WP2a item 1 / `StructuredOutputMode::WhenDone`: while tools remain and the
/// run is not on its last turn, the runner uses AUTO `tool_choice` (the model
/// is free to call its other tools); only the LAST turn forces
/// `StructuredOutput`. Turns 1-2 call the ordinary `Read` tool (dispatched,
/// keeping the loop going); turn 3 (the last, `max_turns == 3`) is forced and
/// returns a schema-valid `StructuredOutput` call.
#[tokio::test]
async fn when_done_uses_auto_tool_choice_until_the_last_turn() {
    let structured = serde_json::json!({ "answer": 1 });
    let api = StructuredOutputModeCapturingApiClient::new(vec![
        tool_use_response("Read", Some("tool_use")),
        tool_use_response("Read", Some("tool_use")),
        structured_output_call_response(structured.clone()),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 3);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    ctx.structured_output_mode = platform_api::subagent_spawn::StructuredOutputMode::WhenDone;
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(
        one_completed(&evs),
        structured,
        "the forced final turn's StructuredOutput call is the result"
    );
    assert_eq!(
        api.forced_calls(),
        vec![false, false, true],
        "WhenDone forces tool_choice only on the last turn"
    );
    assert_eq!(
        invoker.call_count(),
        2,
        "the two non-final turns actually dispatched Read (auto tool_choice let the model use it)"
    );
}

/// Round-3 review items 3/7 regression test. Every REAL Fusion panel
/// advertises more than one tool (`fusion_panel_definition`:
/// `Read`/`Grep`/`Glob`/`WebFetch` plus the injected `StructuredOutput`) —
/// a shape `when_done_uses_auto_tool_choice_until_the_last_turn` above never
/// exercises, since it builds on `tool_schemas: vec![]` and StructuredOutput
/// ends up the sole advertised tool. Before the round-3 fix, the runner's
/// wire gate ANDed `force_this_turn` with the P0-1 single-tool check
/// (`force_tool_choice_for_api.is_some()`, `true` only when
/// `tool_schemas.len() == 1`), so with `Read`/`Grep` also advertised that
/// second conjunct was permanently `false` and WhenDone's last-turn force
/// never reached the wire for any panel — turn 3 here would have gone out
/// with `tool_choice` unpinned. Reproduces the panel shape directly: two
/// tools stay advertised across all three turns; turns 1-2 dispatch `Read`
/// under AUTO tool_choice, and turn 3 (the last) MUST be forced.
#[tokio::test]
async fn when_done_forces_the_last_turn_even_with_other_tools_still_advertised() {
    let structured = serde_json::json!({ "answer": 9 });
    let api = StructuredOutputModeCapturingApiClient::new(vec![
        tool_use_response("Read", Some("tool_use")),
        tool_use_response("Read", Some("tool_use")),
        structured_output_call_response(structured.clone()),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 3);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    ctx.structured_output_mode = platform_api::subagent_spawn::StructuredOutputMode::WhenDone;
    // The exact shape every real Fusion panel spawns with: MORE than one
    // tool advertised alongside the injected `StructuredOutput`, so
    // `tool_schemas.len() != 1` and the P0-1 `force_tool_choice_for_api`
    // gate is `None` for the whole run.
    ctx.tool_schemas = vec![
        serde_json::json!({"name": "Read", "input_schema": {"type": "object"}}),
        serde_json::json!({"name": "Grep", "input_schema": {"type": "object"}}),
    ];
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(
        one_completed(&evs),
        structured,
        "the forced final turn's StructuredOutput call must be the result \
         even with Read/Grep still advertised — a real panel that answers \
         in prose on the unforced last turn ends Failed with no report"
    );
    assert_eq!(
        api.forced_calls(),
        vec![false, false, true],
        "WhenDone must pin tool_choice on the last turn regardless of how \
         many other tools are advertised — this is exactly the >1-tool \
         configuration every real Fusion panel runs in"
    );
    assert_eq!(
        invoker.call_count(),
        2,
        "the two non-final turns still dispatch Read under auto tool_choice"
    );
}

/// WP2a item 1 / `StructuredOutputMode::WhenDone`: once the model has
/// produced TWO consecutive turns with no tool call and no `StructuredOutput`
/// call, the runner forces `StructuredOutput` on the very next turn — even
/// though `max_turns` (5) is far from exhausted.
#[tokio::test]
async fn when_done_forces_after_two_consecutive_idle_turns() {
    let structured = serde_json::json!({ "answer": 2 });
    let api = StructuredOutputModeCapturingApiClient::new(vec![
        text_response("thinking...", Some("end_turn")),
        text_response("still thinking...", Some("end_turn")),
        structured_output_call_response(structured.clone()),
    ]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 5);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    ctx.structured_output_mode = platform_api::subagent_spawn::StructuredOutputMode::WhenDone;
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(
        one_completed(&evs),
        structured,
        "the idle-triggered forced turn's StructuredOutput call is the result"
    );
    assert_eq!(
        api.forced_calls(),
        vec![false, false, true],
        "two idle (no-tool-call) turns force the third turn, well short of max_turns (5)"
    );
}

/// `StructuredOutputMode::WhenDone`: an idle (text-only, non-forced) turn
/// that loops back must APPEND a user message to `history` before doing so.
/// Every other exit from the "no tool call" arm pushes a user message first
/// (the nudge under the forced-turn escalation, or a dispatched tool's
/// tool_results); this is the one path that used to `continue` with nothing
/// appended, which would send the NEXT round-trip a request whose last
/// message is assistant-authored — a prefill continuation rather than a
/// fresh turn, and something a real provider can reject outright. Turn 1 is
/// idle (`end_turn`, no tool call, not forced since it's neither the last
/// turn nor two-idle-turns-deep); turn 2 must see a freshly appended user
/// message as the LAST message of its request.
#[tokio::test]
async fn when_done_idle_turn_appends_a_user_message_before_looping_back() {
    let structured = serde_json::json!({ "answer": 5 });
    let api = StructuredOutputModeCapturingApiClient::new(vec![
        text_response("thinking...", Some("end_turn")),
        structured_output_call_response(structured.clone()),
    ]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 5);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    ctx.structured_output_mode = platform_api::subagent_spawn::StructuredOutputMode::WhenDone;
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(
        one_completed(&evs),
        structured,
        "the second (forced) turn's StructuredOutput call is the result"
    );
    let calls = api.messages_seen();
    assert_eq!(
        calls.len(),
        2,
        "one idle turn followed by one forced turn: forced_calls={:?}",
        api.forced_calls()
    );
    let second_call_last = calls[1]
        .last()
        .expect("the second call's request must carry at least one message");
    assert!(
        matches!(second_call_last, ConversationMessage::User { .. }),
        "the second call's LAST message must be user-authored, not the bare \
         first-turn assistant reply: {second_call_last:?}"
    );
}

/// `StructuredOutputMode::Forced` (the default) is byte-identical to the
/// pre-WP2a behavior: EVERY turn is forced, even the first.
#[tokio::test]
async fn forced_mode_forces_every_turn_including_the_first() {
    let structured = serde_json::json!({ "answer": 3 });
    let api = StructuredOutputModeCapturingApiClient::new(vec![structured_output_call_response(
        structured.clone(),
    )]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 4);
    ctx.schema = Some(r#"{"type":"object"}"#.to_string());
    // `Forced` is also `SubagentContext::structured_output_mode`'s default —
    // set explicitly here so the test documents the invariant rather than
    // relying on the struct's field order.
    ctx.structured_output_mode = platform_api::subagent_spawn::StructuredOutputMode::Forced;
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert_eq!(one_completed(&evs), structured);
    assert_eq!(api.forced_calls(), vec![true]);
}

// ---- P0-1 (2026-09-02): a schema subagent must still be able to call
// tools — the wire `tool_choice` must NOT be pinned to `StructuredOutput` on
// every round when other tools are advertised. See
// `<scratchpad>/WP3-oracle.txt` for the claude-code 2.1.258 oracle evidence
// this is verified against (the shared subagent turn loop hardcodes
// `toolChoice: void 0` and enforces StructuredOutput purely via a repeated
// in-conversation nudge, never via `tool_choice`). ------------------------

/// `SubagentApiClient` that drives the runner exclusively through the
/// `_opts` streaming seam (the one `run_subagent_loop` actually calls) and
/// records, per round-trip, whether it was called through the FORCED variant
/// (`Some(forced_tool)`) or the plain one (`None`) — a direct proxy for what
/// `tool_choice` the wire request would have carried. Also records the
/// message history each round was given, so a test can assert the nudge
/// text rides on a specific round-trip.
struct RecordingForceApiClient {
    responses: Mutex<VecDeque<Result<llm_client::LlmResponse, llm_client::LlmError>>>,
    forced_tools: Mutex<Vec<Option<String>>>,
    messages_per_call: Mutex<Vec<Vec<ConversationMessage>>>,
}

impl RecordingForceApiClient {
    fn new(responses: Vec<Result<llm_client::LlmResponse, llm_client::LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            forced_tools: Mutex::new(Vec::new()),
            messages_per_call: Mutex::new(Vec::new()),
        })
    }

    fn forced_tools(&self) -> Vec<Option<String>> {
        self.forced_tools.lock().unwrap().clone()
    }

    fn messages_per_call(&self) -> Vec<Vec<ConversationMessage>> {
        self.messages_per_call.lock().unwrap().clone()
    }

    fn next_response(&self) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(text_response("(exhausted)", Some("end_turn"))))
    }

    fn record_and_stream(
        &self,
        messages: Vec<ConversationMessage>,
        forced_tool: Option<&str>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        use futures::StreamExt;
        self.forced_tools
            .lock()
            .unwrap()
            .push(forced_tool.map(str::to_string));
        self.messages_per_call.lock().unwrap().push(messages);
        let resp = self.next_response()?;
        let events = crate::accumulator::response_to_stream_events(resp);
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for RecordingForceApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        unreachable!("run_subagent_loop drives every round through the _opts streaming seam only")
    }

    async fn messages_create_stream_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _effort: Option<serde_json::Value>,
        _opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.record_and_stream(messages, None)
    }

    async fn messages_create_stream_forced_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
        _effort: Option<serde_json::Value>,
        _opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.record_and_stream(messages, forced_tool)
    }
}

/// (a) + (b) + (d): a schema run with another tool ALSO advertised must
/// never pin `tool_choice` — not on round 1 (other tools available), and
/// NOT on the round immediately after a turn that ended without a valid
/// StructuredOutput call either (verified oracle behavior: enforcement is
/// the in-conversation nudge alone, never a wire-level restriction). The
/// structured result is still captured and schema-validated once produced.
#[tokio::test]
async fn schema_with_other_tools_never_pins_tool_choice_even_after_a_nudge() {
    let structured = serde_json::json!({ "answer": 42 });
    let api = RecordingForceApiClient::new(vec![
        // Round 1: the model uses an unrelated tool — only possible at all
        // if round 1 was NOT forced to StructuredOutput.
        Ok(tool_use_response("OtherTool", Some("tool_use"))),
        // Round 2: the model ends its turn without calling StructuredOutput
        // at all — triggers the in-conversation nudge.
        Ok(text_response("still thinking", Some("end_turn"))),
        // Round 3 (post-nudge): the model finally calls StructuredOutput.
        Ok(llm_client::LlmResponse {
            content: vec![llm_client::ContentBlock::ToolCall {
                id: ToolUseId::new().to_string(),
                name: "StructuredOutput".into(),
                input: structured.clone(),
            }],
            ..tool_use_response("StructuredOutput", Some("tool_use"))
        }),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 6);
    // A non-empty `tool_schemas` means the injected StructuredOutput tool is
    // NOT the only tool in the registry.
    ctx.tool_schemas = vec![serde_json::json!({
        "name": "OtherTool",
        "description": "an unrelated tool the schema run must still be able to call",
        "input_schema": {"type": "object"}
    })];
    ctx.schema = Some(
        r#"{"type":"object","required":["answer"],"properties":{"answer":{"type":"integer"}}}"#
            .to_string(),
    );
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    // (d) unchanged capture + schema validation.
    assert_eq!(one_completed(&evs), structured);

    // Precondition, not the gate: the run really did reach the other tool's
    // dispatch path. (This one line is polarity-neutral — the canned response
    // names `OtherTool` whether or not the round was forced; the
    // `forced_tools()` assertion below is what flips.)
    assert_eq!(
        invoker.call_count(),
        1,
        "OtherTool must have been dispatched"
    );
    // (a) no round is forced while another tool is advertised.
    let forced = api.forced_tools();
    assert_eq!(
        forced,
        vec![None, None, None],
        "tool_choice must stay unpinned on every round while another tool is \
         advertised, including the round right after a no-call nudge — got {forced:?}"
    );

    // (b, reinterpreted per verified oracle behavior): the nudge text rides
    // as a plain conversational message on round 3's request, NOT a
    // tool_choice restriction.
    let round3 = &api.messages_per_call()[2];
    let nudged = round3.iter().any(|m| {
        matches!(m, ConversationMessage::User { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. }
                if text.contains("You did not call StructuredOutput"))))
    });
    assert!(
        nudged,
        "round 3 must carry the in-conversation nudge from round 2's no-call turn: {round3:?}"
    );
}

/// (c): when the registry has NOTHING but the synthetic StructuredOutput
/// tool, the first round is forced immediately (nothing else the model
/// could usefully call, so pinning it removes a redundant no-call round-trip
/// without changing observable model behavior).
#[tokio::test]
async fn schema_only_tool_in_registry_forces_from_round_one() {
    let structured = serde_json::json!({ "answer": 7 });
    let api = RecordingForceApiClient::new(vec![Ok(llm_client::LlmResponse {
        content: vec![llm_client::ContentBlock::ToolCall {
            id: ToolUseId::new().to_string(),
            name: "StructuredOutput".into(),
            input: structured.clone(),
        }],
        ..tool_use_response("StructuredOutput", Some("tool_use"))
    })]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 4);
    // No pre-existing tools: after injection the registry holds ONLY the
    // synthetic StructuredOutput tool.
    ctx.tool_schemas = vec![];
    ctx.schema = Some(
        r#"{"type":"object","required":["answer"],"properties":{"answer":{"type":"integer"}}}"#
            .to_string(),
    );
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(one_completed(&evs), structured, "(d) capture still works");
    assert_eq!(
        api.forced_tools(),
        vec![Some("StructuredOutput".to_string())],
        "the sole-tool registry must force StructuredOutput from round 1"
    );
}

/// Companion to the relaxation above: once the model is free to keep calling
/// other tools, a schema run CAN reach `max_turns` without ever producing a
/// structured result — an exit that was unreachable while `tool_choice` was
/// pinned every round. That exit must still honour the schema contract and
/// fail, not resolve the caller's `agent()` with the off-schema
/// `{"reason":"max_turns_exhausted"}` completion payload. Oracle 2.1.258
/// @172635430 checks `structured === undefined` after the whole attempt ends,
/// for any exit reason, and throws this exact error.
#[tokio::test]
async fn schema_run_exhausting_max_turns_fails_instead_of_completing_off_schema() {
    // Every round the model calls the unrelated tool and never StructuredOutput;
    // `max_turns` (3) is reached with `should_continue` still true each time.
    let api = RecordingForceApiClient::new(vec![
        Ok(tool_use_response("OtherTool", Some("tool_use"))),
        Ok(tool_use_response("OtherTool", Some("tool_use"))),
        Ok(tool_use_response("OtherTool", Some("tool_use"))),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 3);
    ctx.tool_schemas = vec![serde_json::json!({
        "name": "OtherTool",
        "description": "an unrelated tool the schema run keeps calling",
        "input_schema": {"type": "object"}
    })];
    ctx.schema = Some(
        r#"{"type":"object","required":["answer"],"properties":{"answer":{"type":"integer"}}}"#
            .to_string(),
    );
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    // The precondition this test exists for: the run really did burn all 3
    // turns on the other tool rather than ending early.
    assert_eq!(api.forced_tools().len(), 3, "3 round-trips expected");
    assert_eq!(invoker.call_count(), 3, "OtherTool dispatched every turn");

    let completed: Vec<&serde_json::Value> = evs
        .iter()
        .filter_map(|e| match e {
            SubagentEvent::Completed { result, .. } => Some(result),
            _ => None,
        })
        .collect();
    assert!(
        completed.is_empty(),
        "a schema run must not report success with an off-schema payload \u{2014} got {completed:?}"
    );
    let err = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("a Failed event naming the missing StructuredOutput");
    assert_eq!(
        err,
        "agent({schema}): subagent completed without calling StructuredOutput (after in-conversation nudge)"
    );
}

#[test]
fn structured_output_validation_and_cap_helpers() {
    // Valid input passes; type-mismatch fails with a leaf message.
    let schema = r#"{"type":"object","required":["n"],"properties":{"n":{"type":"integer"}}}"#;
    assert!(validate_structured_output(Some(schema), &serde_json::json!({"n":1})).is_ok());
    assert!(validate_structured_output(Some(schema), &serde_json::json!({"n":"x"})).is_err());
    // No schema ⇒ always Ok. A malformed schema ⇒ Ok (LingXi schema bug,
    // not a model error).
    assert!(validate_structured_output(None, &serde_json::json!({})).is_ok());
    assert!(validate_structured_output(Some("{ not json"), &serde_json::json!({})).is_ok());
    // Default cap is 5 (claude `OBp`).
    assert_eq!(structured_output_retry_cap(), 5);
}

#[test]
fn local_app_operator_schema_requires_complete_host_qa_projection() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../plugins/lingxi-local-app/schemas/workflow-agent-results.schema.json"
    ))
    .expect("parse checked-in Local App workflow role schemas");
    let mut schema = document["$defs"]["operator_result"].clone();
    schema["$defs"] = document["$defs"].clone();
    let schema = serde_json::to_string(&schema).expect("serialize operator role schema");

    let complete = serde_json::json!({
        "ok": true,
        "qa_handle": "qa_00000000000000000000000000000000",
        "evidence_ids": ["evidence-1"],
        "status": "evidence_collected",
        "issues": [],
        "summary": "Host evidence collected",
        "verification_scope": {
            "declared_target_ids": ["primary", "ipad"],
            "in_scope_target_ids": ["primary"],
            "unverified_target_ids": ["ipad"],
            "unverified_scenario_ids": ["ipad-layout"]
        },
        "upstream_failures": [{
            "id": "source:save",
            "message": "save did not persist",
            "introduced_at_ms": 10
        }],
        "upstream_findings": [{
            "id": "source:save",
            "message": "save did not persist",
            "blocking": true,
            "resolved_by_evidence_ids": []
        }]
    });
    assert!(
        validate_structured_output(Some(&schema), &complete).is_ok(),
        "the complete Host QaBegin projection must satisfy the production validator"
    );

    let mut missing_scope = complete.clone();
    missing_scope
        .as_object_mut()
        .expect("operator result object")
        .remove("verification_scope");
    assert!(
        validate_structured_output(Some(&schema), &missing_scope).is_err(),
        "operator output without canonical Host scope must fail closed"
    );

    for ledger_field in ["upstream_failures", "upstream_findings"] {
        let mut missing_ledger = complete.clone();
        missing_ledger
            .as_object_mut()
            .expect("operator result object")
            .remove(ledger_field);
        assert!(
            validate_structured_output(Some(&schema), &missing_ledger).is_err(),
            "operator output without {ledger_field} must fail closed"
        );
    }

    let mut incomplete_finding = complete;
    incomplete_finding["upstream_findings"][0]
        .as_object_mut()
        .expect("upstream finding object")
        .remove("resolved_by_evidence_ids");
    assert!(
        validate_structured_output(Some(&schema), &incomplete_finding).is_err(),
        "Host upstream finding projection must include resolution evidence ids"
    );
}

#[tokio::test]
async fn loop_g1_completed_carries_final_turn_usage_and_tool_count() {
    // G1: the terminal Completed event carries the FINAL turn's usage (claude
    // reads only the last message usage, not a cross-turn sum) plus the
    // run-wide tool-use count. Turn 1: tool_use with usage A (dispatched).
    // Turn 2: end_turn text with usage B → carried usage == B; tool_uses == 1.
    let usage_a = llm_client::Usage {
        billable_tokens: llm_client::TokenUsage {
            input: 1000,
            output: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let usage_b = llm_client::Usage {
        billable_tokens: llm_client::TokenUsage {
            input: 10,
            output: 5,
            cache_write: 3,
            cache_read: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response_with_usage(
            "Read",
            Some("tool_use"),
            usage_a,
        )),
        Ok(llm_client::LlmResponse {
            usage: usage_b.clone(),
            ..text_response("done", Some("end_turn"))
        }),
    ]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let (usage, tool_count) = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed {
                usage,
                total_tool_use_count,
                ..
            } => Some((usage.clone(), *total_tool_use_count)),
            _ => None,
        })
        .expect("one Completed");
    // The carried usage is the FINAL turn's (B), NOT a sum with A.
    assert_eq!(
        usage.billable_tokens.input, 10,
        "final-turn input, not summed"
    );
    assert_eq!(usage.billable_tokens.output, 5);
    assert_eq!(usage.billable_tokens.cache_write, 3);
    assert_eq!(usage.billable_tokens.cache_read, 2);
    // One tool_use across the run (turn 1).
    assert_eq!(tool_count, 1, "run-wide tool-use count");
}

/// WP2a item 2: the runner no longer clamps REPORTED usage against
/// `max_output_tokens_per_turn` — that ceiling reaches the provider on the
/// WIRE (via `SubagentApiCallOpts::max_output_tokens`, asserted in
/// `orchestrator::provider_adapter`'s own tests), and the terminal `Completed`
/// carries the provider's REAL usage even when it exceeds the requested cap
/// (a provider can still overrun its own advertised ceiling).
#[tokio::test]
async fn loop_completed_cumulative_usage_sums_turns_and_reports_real_uncapped_output() {
    let usage_a = llm_client::Usage {
        billable_tokens: llm_client::TokenUsage {
            input: 1000,
            output: 40,
            ..Default::default()
        },
        ..Default::default()
    };
    let usage_b = llm_client::Usage {
        billable_tokens: llm_client::TokenUsage {
            input: 10,
            output: 50,
            cache_write: 3,
            cache_read: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response_with_usage(
            "Read",
            Some("tool_use"),
            usage_a,
        )),
        Ok(llm_client::LlmResponse {
            usage: usage_b.clone(),
            ..text_response("done", Some("end_turn"))
        }),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    ctx.max_output_tokens_per_turn = Some(8);
    ctx.max_input_bytes_per_turn = Some(4096);
    // Keep the cap active while leaving both mandatory seed units safely
    // representable; strict over-cap rejection is covered by the focused
    // cap helper tests below.
    ctx.prompt_messages = vec![
        ConversationMessage::user(MessageId::new(), "keep-me".into()),
        ConversationMessage::user(MessageId::new(), "x".repeat(200)),
    ];
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    let (usage, cumulative) = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed {
                usage,
                cumulative_usage,
                ..
            } => Some((usage.clone(), cumulative_usage.clone())),
            _ => None,
        })
        .expect("one Completed");
    assert_eq!(
        usage.billable_tokens.output, 50,
        "final-turn output is the provider's REAL usage, not clamped to the \
         requested per-turn ceiling (8)"
    );
    assert_ne!(
        usage.billable_tokens.input, cumulative.billable_tokens.input,
        "cumulative input must include earlier turns"
    );
    assert_eq!(cumulative.billable_tokens.input, 1010);
    assert_eq!(
        cumulative.billable_tokens.output, 90,
        "real 40 + real 50, uncapped by max_output_tokens_per_turn"
    );
    let last = api.last_messages();
    let joined = format!("{last:?}");
    assert!(
        joined.contains(&"x".repeat(200)),
        "the second seed unit fits the active cap and must remain in the API call"
    );
    assert!(
        serde_json::to_vec(&last).unwrap().len() <= 4096,
        "the request must still honor max_input_bytes_per_turn"
    );
    assert!(
        joined.contains("keep-me"),
        "the pinned head unit must survive trimming: {joined}"
    );
    assert!(!last.is_empty(), "at least the newest message is retained");
}

/// `SubagentApiClient` that implements ONLY the two `_in_opts` methods
/// (`messages_create_stream_in_opts` / `messages_create_stream_forced_in_opts`
/// — the seam `run_subagent_loop` actually calls, per `runner.rs`'s
/// `call_opts` construction). Every other method — including the ones the
/// trait's OWN default chain would fall back to (`messages_create_stream_in`,
/// `..._forced_in`, `..._stream`, `..._forced`, `messages_create`) —
/// is `unreachable!()`, so any call that reaches this client through a
/// non-opts method panics instead of silently dropping `opts`. Records the
/// `SubagentApiCallOpts` seen on every round-trip.
struct OptsOnlyCapturingApiClient {
    responses: Mutex<VecDeque<llm_client::LlmResponse>>,
    opts_seen: Mutex<Vec<crate::api::SubagentApiCallOpts>>,
}

impl OptsOnlyCapturingApiClient {
    fn new(responses: Vec<llm_client::LlmResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            opts_seen: Mutex::new(Vec::new()),
        })
    }

    fn opts_seen(&self) -> Vec<crate::api::SubagentApiCallOpts> {
        self.opts_seen.lock().unwrap().clone()
    }

    fn record_and_stream(
        &self,
        opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        use futures::StreamExt;
        self.opts_seen.lock().unwrap().push(opts);
        let resp = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text_response("(exhausted)", Some("end_turn")));
        let events = crate::accumulator::response_to_stream_events(resp);
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

#[async_trait]
impl crate::api::SubagentApiClient for OptsOnlyCapturingApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        unreachable!("driven only through the _opts streaming seam")
    }

    async fn messages_create_stream_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _effort: Option<serde_json::Value>,
        opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.record_and_stream(opts)
    }

    async fn messages_create_stream_forced_in_opts(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _forced_tool: Option<&str>,
        _effort: Option<serde_json::Value>,
        opts: crate::api::SubagentApiCallOpts,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.record_and_stream(opts)
    }
}

/// WP2a item 2 (F002 sub-claim 3), round 2: the Fusion-only decorator
/// `WorkflowWatchdogApiClient` sits between the runner and the production
/// `ProviderApiAdapter` on the ONLY path that spawns a Fusion panel
/// (`panel.rs` -> `handle.rs`'s `WORKFLOW_QUERY_WATCHDOG_OVERRIDE` machinery
/// wraps `ctx.api_client` in it). If the decorator does not override the two
/// `_in_opts` methods, the trait's default implementation re-dispatches
/// through the NON-opts chain on `self` (the wrapper), which the wrapper only
/// overrides down to `messages_create_stream_forced_in` / `..._stream_in` —
/// so `opts` (the Fusion per-turn `max_output_tokens` ceiling and the COGS
/// `query_source_label`) is silently dropped before it ever reaches the inner
/// adapter, and a client that implements ONLY the opts seam is driven through
/// its unimplemented non-opts fallback and panics.
#[tokio::test]
async fn workflow_watchdog_wrapper_threads_opts_to_the_inner_client() {
    let api = OptsOnlyCapturingApiClient::new(vec![
        tool_use_response("Read", Some("tool_use")),
        text_response("done", Some("end_turn")),
    ]);
    let wrapped: Arc<dyn crate::api::SubagentApiClient> =
        Arc::new(crate::api::WorkflowWatchdogApiClient::new(
            api.clone(),
            platform_api::WorkflowQueryWatchdog {
                stall_timeout_ms: 60_000,
                max_retries: 0,
            },
            Vec::new(),
        ));
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(wrapped, Some(invoker.clone()), 4);
    ctx.max_output_tokens_per_turn = Some(4096);
    ctx.query_source_label = Some("fusion_panel".to_string());
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Completed { .. })),
        "the run must complete through the wrapper: {evs:?}"
    );
    let opts = api.opts_seen();
    assert_eq!(
        opts.len(),
        2,
        "both turns must reach the inner client through the opts seam \
         (a fallback to the non-opts chain panics before this point)"
    );
    for (turn, o) in opts.iter().enumerate() {
        assert_eq!(
            o.max_output_tokens,
            Some(4096),
            "turn {turn}: WorkflowWatchdogApiClient dropped max_output_tokens"
        );
        assert_eq!(
            o.query_source_label.as_deref(),
            Some("fusion_panel"),
            "turn {turn}: WorkflowWatchdogApiClient dropped query_source_label"
        );
    }
}

#[tokio::test]
async fn loop_consumes_streaming_seam_end_to_end() {
    // Proves the runner drives the loop through `messages_create_stream`:
    // the mock's non-streaming `messages_create` is `unreachable!`. Turn 1
    // streams a `tool_use` (dispatched via the invoker); turn 2 streams the
    // final text. Asserts two streamed round-trips, one tool dispatch, and
    // that the accumulated turn carries the streamed text + stop_reason.
    let api = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Read", "tool_use"),
        streamed_text_turn("streamed answer", "end_turn"),
    ]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 2, "two streamed round-trips");
    assert_eq!(invoker.call_count(), 1, "streamed tool_use dispatched once");
    let result = one_completed(&evs);
    assert_eq!(result["text"], "streamed answer");
    assert_eq!(result["stop_reason"], "end_turn");
}

#[tokio::test]
async fn synchronous_child_dispatch_preserves_scheduled_headless_session_mode() {
    let api = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Read", "tool_use"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let invoker = SessionModeRecordingInvoker::new();
    let mut ctx = loop_ctx(api, Some(invoker.clone()), 4);
    ctx.is_async = false;
    ctx.session_interactive = Some(false);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        *invoker.captured.lock().unwrap(),
        Some(true),
        "scheduled work must not become interactive at the child tool boundary"
    );
}

#[tokio::test]
async fn loop_advertises_context_tool_schemas_to_the_seam() {
    // `ctx.tool_schemas` must reach `messages_create_stream`'s `tools` arg
    // on every round-trip (this is what lets the model emit `tool_use`).
    let api = StreamingMockApiClient::new(vec![streamed_text_turn("done", "end_turn")]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    let schemas = vec![serde_json::json!({
        "name": "Read",
        "description": "Reads a file.",
        "input_schema": {"type": "object"}
    })];
    ctx.tool_schemas = schemas.clone();

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        api.last_tools(),
        schemas,
        "ctx.tool_schemas must be forwarded verbatim to the streaming seam"
    );
}

#[tokio::test]
async fn schema_replaces_inherited_structured_output_tool() {
    let api =
        StreamingMockApiClient::new(vec![streamed_tool_use_turn("StructuredOutput", "tool_use")]);
    let mut ctx = loop_ctx(api.clone(), Some(CountingInvoker::new()), 4);
    ctx.tool_schemas = vec![
        serde_json::json!({
            "name": "Read",
            "description": "Reads a file.",
            "input_schema": {"type": "object"}
        }),
        serde_json::json!({
            "name": "StructuredOutput",
            "description": "Generic structured output inherited from the registry.",
            "input_schema": {
                "type": "object",
                "additionalProperties": true
            }
        }),
    ];
    let stage_schema = serde_json::json!({
        "type": "object",
        "required": ["ok"],
        "properties": {"ok": {"type": "boolean"}}
    });
    ctx.schema = Some(stage_schema.to_string());

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let tools = api.last_tools();
    let structured: Vec<&serde_json::Value> = tools
        .iter()
        .filter(|tool| tool["name"] == "StructuredOutput")
        .collect();
    assert_eq!(
        structured.len(),
        1,
        "the provider request must not contain duplicate tool names: {tools:?}"
    );
    assert_eq!(
        structured[0]["input_schema"], stage_schema,
        "the workflow stage schema must replace the inherited generic schema"
    );
}

#[tokio::test]
async fn loop_streaming_protocol_error_surfaces_failed() {
    // A streamed turn that ends without `message_stop` accumulates to
    // `LlmError::StreamInterrupted`, which the loop surfaces as Failed
    // (same path as a non-streaming api error).
    let truncated = vec![
        ev_message_start(),
        llm_client::LlmEvent::ContentBlockStart {
            index: 0,
            content_block: llm_client::ContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
        },
        // no content_block_stop, no message_stop
    ];
    let api = StreamingMockApiClient::new(vec![truncated]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    let failed = evs.iter().find_map(|e| match e {
        SubagentEvent::Failed { error, .. } => Some(error.clone()),
        _ => None,
    });
    let err = failed.expect("Failed on truncated stream");
    assert!(err.starts_with("subagent api error:"), "got: {err}");
}

#[tokio::test]
async fn loop_refuses_tool_outside_allowed_list_without_dispatching() {
    // allowed_tools = ["Bash"]; the model asks for "Read" → refused WITHOUT
    // dispatch (invoker never called), surfaced as an is_error ToolResult,
    // and the loop continues to a clean end_turn.
    let api = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Read", "tool_use"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    ctx.allowed_tools = vec!["Bash".to_string()];

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(
        invoker.call_count(),
        0,
        "a tool outside allowed_tools must NOT be dispatched"
    );
    // The refusal must be a structured is_error ToolResult carrying the
    // reason (so the model sees it like any tool error and can recover) —
    // deserialize the emitted Message rather than substring-matching.
    let refused = evs.iter().any(|e| {
        let SubagentEvent::Message { message, .. } = e else {
            return false;
        };
        let Ok(ConversationMessage::User { content, .. }) =
            serde_json::from_value::<ConversationMessage>(message.clone())
        else {
            return false;
        };
        content.iter().any(|b| {
            matches!(b, ContentBlock::ToolResult { is_error: true, content, .. }
                if content.contains("not in this agent's allowed tools"))
        })
    });
    assert!(
        refused,
        "refusal must be an is_error ToolResult carrying the reason; got {evs:?}"
    );
    let result = one_completed(&evs);
    assert_eq!(result["stop_reason"], "end_turn");
}

// ── companion note (yyo / nke) tests ──────────────────────────────────

/// Unit test for `companion_note_for_disallowed_tool` (pure function, no
/// async). Verifies the `nke` set membership + the exact §7 note text.
#[test]
fn companion_note_returned_for_nke_tools() {
    // Non-ant: all base nke tools AND Workflow must return the note.
    for name in &[
        "TaskOutput",
        "ExitPlanMode",
        "EnterPlanMode",
        "AskUserQuestion",
        "ConnectGitHub",
        "WaitForMcpServers",
        "ScheduleWakeup",
        "Workflow",
    ] {
        let note = companion_note_for_disallowed_tool(name, /*is_ant=*/ false);
        assert!(
            note.is_some(),
            "expected companion note for {name} (non-ant); got None"
        );
        let note = note.unwrap();
        // Binary §7 verbatim: leading `. `, toolName interpolated.
        assert!(
            note.starts_with(". "),
            "note must start with \". \"; got {note:?}"
        );
        assert!(
            note.contains(name),
            "note must contain tool name {name:?}; got {note:?}"
        );
        assert!(
            note.contains("not available inside subagents"),
            "note must contain 'not available inside subagents'; got {note:?}"
        );
        assert!(
            note.contains("return findings to the orchestrator"),
            "note must contain 'return findings to the orchestrator'; got {note:?}"
        );
    }
}

#[test]
fn companion_note_absent_for_non_nke_tools() {
    // Regular tools must NOT get the companion note.
    for name in &["Read", "Bash", "Grep", "Glob", "Agent"] {
        assert!(
            companion_note_for_disallowed_tool(name, false).is_none(),
            "unexpected companion note for non-nke tool {name}"
        );
    }
}

#[test]
fn companion_note_workflow_absent_for_ant() {
    // Workflow is in nke only for non-ant (HDd ant-gate).
    assert!(
        companion_note_for_disallowed_tool("Workflow", /*is_ant=*/ true).is_none(),
        "Workflow must NOT get the companion note for ant users"
    );
    // TaskOutput (base set) is still in nke for ant.
    assert!(
        companion_note_for_disallowed_tool("TaskOutput", /*is_ant=*/ true).is_some(),
        "TaskOutput must get the companion note for ant users"
    );
}

#[tokio::test]
async fn loop_nke_tool_refusal_includes_companion_note() {
    // A non-ant subagent whose pool dropped "Workflow" (allowed_tools = ["Read"])
    // calls "Workflow" → the refusal ToolResult must contain the §7 companion
    // note with "Workflow" interpolated. `AskUserQuestion` is verified too.
    for nke_tool in &["Workflow", "AskUserQuestion"] {
        let api = StreamingMockApiClient::new(vec![
            streamed_tool_use_turn(nke_tool, "tool_use"),
            streamed_text_turn("done", "end_turn"),
        ]);
        let invoker = CountingInvoker::new();
        let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
        ctx.allowed_tools = vec!["Read".to_string()];

        let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        let note_present = evs.iter().any(|e| {
            let SubagentEvent::Message { message, .. } = e else {
                return false;
            };
            let Ok(ConversationMessage::User { content, .. }) =
                serde_json::from_value::<ConversationMessage>(message.clone())
            else {
                return false;
            };
            content.iter().any(|b| {
                matches!(b,
                    ContentBlock::ToolResult { is_error: true, content, .. }
                    if content.contains("not available inside subagents")
                        && content.contains(nke_tool)
                        && content.contains("return findings to the orchestrator")
                )
            })
        });
        assert!(
            note_present,
            "§7 companion note missing from refusal for nke tool {nke_tool}; got {evs:?}"
        );
    }
}

#[tokio::test]
async fn loop_non_nke_tool_refusal_has_no_companion_note() {
    // A non-nke tool (e.g. "Bash") blocked by the allow-list must NOT include
    // the companion note — the note is nke-specific.
    let api = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Bash", "tool_use"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    ctx.allowed_tools = vec!["Read".to_string()];

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    let companion_note_absent = evs.iter().all(|e| {
        let SubagentEvent::Message { message, .. } = e else {
            return true;
        };
        let Ok(ConversationMessage::User { content, .. }) =
            serde_json::from_value::<ConversationMessage>(message.clone())
        else {
            return true;
        };
        content.iter().all(|b| {
            !matches!(b,
                ContentBlock::ToolResult { is_error: true, content, .. }
                if content.contains("not available inside subagents")
            )
        })
    });
    assert!(
        companion_note_absent,
        "companion note must NOT appear for non-nke tool 'Bash'; got {evs:?}"
    );
}

#[tokio::test]
async fn loop_allows_tool_in_allowed_list() {
    // allowed_tools = ["Read"]; "Read" is dispatched normally.
    let api = StreamingMockApiClient::new(vec![
        streamed_tool_use_turn("Read", "tool_use"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
    ctx.allowed_tools = vec!["Read".to_string()];

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        invoker.call_count(),
        1,
        "a tool inside allowed_tools is dispatched"
    );
}

#[tokio::test]
async fn loop_budget_exhausted_stops_before_any_round_trip() {
    // Test A: the inherited budget is already over the limit. The per-turn
    // gate fires BEFORE the first model round-trip, so the loop emits a
    // single budget-exhausted Failed and makes ZERO model calls. The error
    // is the 2.1.217 byte-locked background-agent halt string formatted from
    // current_nano_usd (1.5e9 -> "$1.50") and the $1 ceiling.
    let api = MockSubagentApiClient::new(vec![Ok(text_response("unused", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.budget = Some(Arc::new(MockBudget { exceeded: true }));

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(
        api.call_count(),
        0,
        "the budget gate precedes messages_create — no round-trip"
    );
    let failed = evs.iter().find_map(|e| match e {
        SubagentEvent::Failed { error, .. } => Some(error.clone()),
        _ => None,
    });
    assert_eq!(
        failed.as_deref(),
        Some("Budget limit reached ($1.50 of $1); stopping background agents."),
        "byte-locked 2.1.217 denial string; got events: {evs:?}"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Completed { .. })),
        "no Completed when stopped on budget; got events: {evs:?}"
    );
}

#[tokio::test]
async fn loop_budget_ok_does_not_interfere_with_completion() {
    // Test B (non-interference): a within-limit budget lets the existing
    // single-end_turn path complete with aggregated text and the api is
    // called exactly once — the gate is transparent when `Ok`.
    let api = MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.budget = Some(Arc::new(MockBudget { exceeded: false }));

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 1, "exactly one model round-trip");
    let result = one_completed(&evs);
    assert_eq!(result["text"], "final answer");
    assert_eq!(result["stop_reason"], "end_turn");
}

#[tokio::test]
async fn loop_tool_use_then_end_turn_invokes_tool_and_runs_two_turns() {
    // Core happy path: turn 1 emits a tool_use (stop_reason tool_use) ->
    // tool is invoked -> results fed back -> turn 2 ends. Asserts 2 model
    // calls, exactly one tool invocation, and final aggregated text.
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response("Read", Some("tool_use"))),
        Ok(text_response("done", Some("end_turn"))),
    ]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 2, "two model round-trips");
    assert_eq!(
        invoker.call_count(),
        1,
        "tool invoked once (1:1 with tool_use)"
    );
    let result = one_completed(&evs);
    assert_eq!(result["text"], "done");
    assert_eq!(result["stop_reason"], "end_turn");
}

#[tokio::test]
async fn permission_abort_fails_subagent_without_recoverable_tool_result() {
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response("Bash", Some("tool_use"))),
        Ok(text_response("must not run", Some("end_turn"))),
    ]);
    let ctx = loop_ctx(api.clone(), Some(Arc::new(AbortInvoker)), 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let events = drain(out_rx).await;

    assert_eq!(api.call_count(), 1, "the abort must stop the model loop");
    assert!(events.iter().any(|event| matches!(
        event,
        SubagentEvent::Failed { error, .. }
            if error == "Agent aborted: too many classifier denials in headless mode"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, SubagentEvent::Completed { .. })));
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SubagentEvent::Message { message, .. }
                if message.to_string().contains("tool_result")
        )),
        "a terminal permission abort must not be fed back to the model: {events:?}"
    );
}

#[tokio::test]
async fn loop_end_turn_with_tool_use_still_dispatches_then_completes() {
    // MAJOR #1 regression: an `end_turn` response that ALSO carries a
    // tool_use must NOT silently drop the tool. The reference dispatches
    // tools whenever present, then terminates on end_turn. Assert the tool
    // was invoked AND the run completed in a single turn (no continuation).
    let api = MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("end_turn")))]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(
        api.call_count(),
        1,
        "end_turn terminates after one round-trip"
    );
    assert_eq!(
        invoker.call_count(),
        1,
        "tool_use on an end_turn response is still dispatched"
    );
    let result = one_completed(&evs);
    assert_eq!(result["stop_reason"], "end_turn");
}

#[tokio::test]
async fn loop_truncated_tool_use_terminates_instead_of_looping() {
    // MAJOR #2 regression: a non-`tool_use` reason (e.g. max_tokens) that
    // also carried a tool_use must dispatch the tool then TERMINATE — not
    // continue looping until max_turns. With max_turns=4 the loop would
    // make 4 calls if it (incorrectly) continued; the fix caps it at 1.
    let api = MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("max_tokens")))]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(
        api.call_count(),
        1,
        "max_tokens terminates after one round-trip (no loop-to-max_turns)"
    );
    assert_eq!(
        invoker.call_count(),
        1,
        "the truncated turn's tool is still dispatched"
    );
    let result = one_completed(&evs);
    assert_eq!(result["stop_reason"], "max_tokens");
}

#[tokio::test]
async fn loop_api_error_surfaces_failed() {
    let api = MockSubagentApiClient::new(vec![Err(llm_client::LlmError::InvalidRequest {
        message: "boom".into(),
    })]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    let failed = evs.iter().find_map(|e| match e {
        SubagentEvent::Failed { error, .. } => Some(error.clone()),
        _ => None,
    });
    let err = failed.expect("Failed on api error");
    assert!(err.starts_with("subagent api error:"), "got: {err}");
}

#[tokio::test]
async fn loop_api_error_persists_seed_and_terminal_reason() {
    let dir = tempfile::tempdir().unwrap();
    let api = MockSubagentApiClient::new(vec![Err(llm_client::LlmError::InvalidRequest {
        message: "provider rejected tool_choice".into(),
    })]);
    let mut ctx = loop_ctx(api, None, 4);
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    ctx.prompt_messages = vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "design the local app".to_string(),
    )];
    let agent_id = ctx.agent_id;

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let path = dir.path().join(format!("agent-{agent_id}.jsonl"));
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "failed run transcript at {} should exist: {e}",
            path.display()
        )
    });
    assert!(
        body.contains("design the local app"),
        "the seed must survive a first-request API failure: {body}"
    );
    assert!(
        body.contains("provider rejected tool_choice"),
        "the terminal provider reason must be inspectable: {body}"
    );
    assert!(
        body.contains("\"status\":\"failed\""),
        "the transcript must distinguish terminal failure: {body}"
    );
}

#[tokio::test]
async fn loop_tool_use_without_invoker_fails() {
    // A tool_use with tool_invoker = None surfaces Failed.
    let api = MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("tool_use")))]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    let failed = evs.iter().find_map(|e| match e {
        SubagentEvent::Failed { error, .. } => Some(error.clone()),
        _ => None,
    });
    assert_eq!(
        failed.as_deref(),
        Some("subagent requested a tool but no tool_invoker was inherited")
    );
}

#[tokio::test]
async fn loop_exhausts_max_turns_when_never_terminal() {
    // Every turn emits a tool_use with stop_reason tool_use, so the loop
    // continues. With max_turns=3 it makes exactly 3 model calls then
    // surfaces Completed{reason: "max_turns_exhausted"}.
    let api = MockSubagentApiClient::new(vec![
        Ok(tool_use_response("Read", Some("tool_use"))),
        Ok(tool_use_response("Read", Some("tool_use"))),
        Ok(tool_use_response("Read", Some("tool_use"))),
        // a 4th would only be reached on an off-by-one bug:
        Ok(text_response("should-not-reach", Some("end_turn"))),
    ]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 3);

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 3, "exactly max_turns model round-trips");
    assert_eq!(invoker.call_count(), 3, "one tool dispatch per turn");
    let result = one_completed(&evs);
    assert_eq!(result["reason"], "max_turns_exhausted");
    assert_eq!(result["max_turns"], 3);
}

#[tokio::test]
async fn loop_user_interrupt_mid_flight_surfaces_killed() {
    // A UserInterrupt delivered while the loop is racing the API future
    // aborts to Killed. We pre-load the event so the biased select! takes
    // the termination arm on the first poll.
    let api = MockSubagentApiClient::new(vec![Ok(text_response("unused", Some("end_turn")))]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    event_tx
        .send(lingxi_core::Event::UserInterrupt)
        .await
        .unwrap();

    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Killed { .. })),
        "expected Killed on UserInterrupt; got: {evs:?}"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Completed { .. })),
        "no Completed when killed mid-flight; got: {evs:?}"
    );
}

// ---- Persist-mode tests (ctx.persistent = true) ----------------------

#[tokio::test]
async fn persist_mode_processes_second_message_after_idling() {
    // Turn-set 1: a single end_turn turn completes, then the runner parks
    // (it does NOT return because persistent = true). We then inject a
    // second UserMessage which un-idles it and drives turn-set 2; finally
    // we close the channel to terminate gracefully. Asserts: exactly two
    // model round-trips and two Completed events (one per turn-set).
    let api = MockSubagentApiClient::new(vec![
        Ok(text_response("answer one", Some("end_turn"))),
        Ok(text_response("answer two", Some("end_turn"))),
    ]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Wait for turn-set 1 to complete (the runner has now idled), then
    // inject the second message that drives turn-set 2.
    let mut out_rx = out_rx;
    let first_completed = loop {
        let ev = out_rx.recv().await.expect("turn-set 1 should complete");
        if matches!(ev, SubagentEvent::Completed { .. }) {
            break ev;
        }
    };
    let SubagentEvent::Completed { result, .. } = &first_completed else {
        unreachable!()
    };
    assert_eq!(result["text"], "answer one", "turn-set 1 result");

    event_tx
        .send(lingxi_core::Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: "second question".into(),
        })
        .await
        .unwrap();

    // Wait for turn-set 2 to complete.
    let second_completed = loop {
        let ev = out_rx.recv().await.expect("turn-set 2 should complete");
        if matches!(ev, SubagentEvent::Completed { .. }) {
            break ev;
        }
    };
    let SubagentEvent::Completed { result, .. } = &second_completed else {
        unreachable!()
    };
    assert_eq!(result["text"], "answer two", "turn-set 2 result");

    // Close the channel: the parked runner terminates gracefully.
    drop(event_tx);
    handle.await.unwrap();

    assert_eq!(
        api.call_count(),
        2,
        "exactly two model round-trips (one per turn-set)"
    );
}

#[tokio::test]
async fn persist_mode_transcript_distinguishes_idle_from_true_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let api = MockSubagentApiClient::new(vec![
        Ok(text_response("one", Some("end_turn"))),
        Ok(text_response("two", Some("end_turn"))),
    ]);
    let mut ctx = loop_ctx(api, None, 4);
    ctx.persistent = true;
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    let agent_id = ctx.agent_id;
    let path = dir.path().join(format!("agent-{agent_id}.jsonl"));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, mut out_rx) = mpsc::channel::<SubagentEvent>(16);
    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // The first completed turn parks the persistent runner; its durable state
    // must be idle, not terminal-completed.
    while !matches!(
        out_rx.recv().await.expect("first turn"),
        SubagentEvent::Completed { .. }
    ) {}
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    let statuses: Vec<String> = body
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|line| {
            line.get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(statuses.first().map(String::as_str), Some("running"));
    assert_eq!(statuses.last().map(String::as_str), Some("idle"));
    assert!(
        body.contains("\"agent_type\":\"test\""),
        "resolved agent type metadata is persisted"
    );

    event_tx
        .send(lingxi_core::Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: "next".into(),
        })
        .await
        .unwrap();
    while !matches!(
        out_rx.recv().await.expect("second turn"),
        SubagentEvent::Completed { .. }
    ) {}
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    let last_status = body
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|line| {
            line.get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    assert_eq!(last_status.as_deref(), Some("idle"));

    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    handle.await.unwrap();
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    let last_status = body
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|line| {
            line.get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    assert_eq!(last_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn persist_mode_terminates_on_channel_close_after_turn_set() {
    // With persistent = true, closing the event channel after the first
    // turn-set completes makes the parked runner return gracefully (no
    // further events, no Failed).
    let api = MockSubagentApiClient::new(vec![Ok(text_response("done", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

    // Drop the sender immediately: the runner runs turn-set 1, parks, sees
    // the channel already closed, and returns.
    drop(event_tx);

    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    assert_eq!(api.call_count(), 1, "one turn-set ran before EOF");
    let completed = evs
        .iter()
        .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
        .count();
    assert_eq!(completed, 1, "one Completed; got: {evs:?}");
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "no Failed on graceful EOF; got: {evs:?}"
    );
}

#[tokio::test]
async fn persist_mode_user_exit_while_idle_surfaces_killed() {
    // While parked between turn-sets, a UserExit terminates the teammate
    // with Killed (cooperative shutdown).
    let api = MockSubagentApiClient::new(vec![Ok(text_response("done", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    let mut out_rx = out_rx;
    // Wait for turn-set 1 to complete (runner now idle).
    loop {
        let ev = out_rx.recv().await.expect("turn-set 1 completes");
        if matches!(ev, SubagentEvent::Completed { .. }) {
            break;
        }
    }
    // Deliver UserExit to the idle runner.
    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    handle.await.unwrap();

    let evs = drain(out_rx).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Killed { agent_id: aid } if *aid == agent_id)),
        "UserExit while idle yields Killed; got: {evs:?}"
    );
}

// ---- Wake-on-message (cc 2.1.198, M9) ---------------------------------

/// `SubagentApiClient` whose FIRST round-trip hangs forever (a teammate
/// "stuck" mid-request / in the client's internal retry backoff) and whose
/// subsequent round-trips capture their `messages` then answer end_turn.
struct StuckThenCapturingApiClient {
    calls: AtomicUsize,
    first_call_started: tokio::sync::Notify,
    later_messages: Mutex<Vec<Vec<ConversationMessage>>>,
}
impl StuckThenCapturingApiClient {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            first_call_started: tokio::sync::Notify::new(),
            later_messages: Mutex::new(Vec::new()),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl crate::api::SubagentApiClient for StuckThenCapturingApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            // Stuck: never resolves. The runner's select must drop this
            // future on the inbound UserMessage and re-issue.
            self.first_call_started.notify_one();
            std::future::pending::<()>().await;
            unreachable!("the stuck first call must be dropped, not resolved")
        }
        self.later_messages.lock().unwrap().push(messages);
        Ok(text_response("woke and answered", Some("end_turn")))
    }
}

/// cc 2.1.198 (M9): messaging a stuck teammate wakes it to retry immediately.
/// Binary mechanism: SendMessage emits the recipient task's `retryWake` signal
/// after the mailbox write (`TDo` @215134403: `r.retryWake?.emit()`), which
/// `subscribeRetryWake` (@216289770) threads into the API retry loop so the
/// backoff sleep is interrupted and the queued message rides into the turn.
/// Rust analog: a `UserMessage` racing the in-flight round-trip drops the
/// stuck `api_call` future, appends the message to history, and re-issues the
/// round-trip immediately — previously the message text was silently DROPPED.
#[tokio::test]
async fn persist_mode_message_wakes_stuck_round_trip_and_carries_the_text() {
    let api = StuckThenCapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Wait until the first round-trip is in flight (stuck).
    api.first_call_started.notified().await;

    // Message the stuck teammate.
    event_tx
        .send(lingxi_core::Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: "are you alive? try again".into(),
        })
        .await
        .unwrap();

    // The wake re-issues the round-trip immediately; the retry completes.
    let mut out_rx = out_rx;
    let completed = loop {
        let ev = out_rx.recv().await.expect("the woken turn-set completes");
        if matches!(ev, SubagentEvent::Completed { .. }) {
            break ev;
        }
    };
    let SubagentEvent::Completed { result, .. } = &completed else {
        unreachable!()
    };
    assert_eq!(result["text"], "woke and answered");
    assert_eq!(api.call_count(), 2, "stuck call dropped + immediate retry");

    // The retried round-trip CARRIES the message (cc queues it via
    // pendingUserMessages; here it rides on the re-issued history).
    let later = api.later_messages.lock().unwrap().clone();
    let retry_history = later.first().expect("retry captured");
    let carried = retry_history.iter().any(|m| {
        matches!(m, ConversationMessage::User { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. }
                if text.contains("are you alive? try again"))))
    });
    assert!(
        carried,
        "the wake message must ride on the retried round-trip, not be dropped: {retry_history:?}"
    );

    // Cooperative shutdown of the parked (persistent) runner.
    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    handle.await.unwrap();
}

// ---- Launcher message ≠ permission approval (cc 2.1.198, M10) ----------

/// `ToolInvoker` that PARKS inside `invoke` — a pending permission prompt
/// living in the permission gate below the invoker seam — until the test
/// releases it, then resolves as the gate's DENY. Tracks whether the pending
/// prompt was resolved and how many times the tool ran.
struct PendingPermissionInvoker {
    invoke_started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: AtomicUsize,
    resolved: std::sync::atomic::AtomicBool,
}
impl PendingPermissionInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            invoke_started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
            resolved: std::sync::atomic::AtomicBool::new(false),
        })
    }
}
#[async_trait]
impl platform_api::ToolInvoker for PendingPermissionInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.invoke_started.notify_one();
        self.release.notified().await;
        self.resolved.store(true, Ordering::SeqCst);
        Err(platform_api::tool_invoker::ToolInvokerError::Internal(
            "Permission to use SlowTool has been denied.".into(),
        ))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `SubagentApiClient` whose FIRST round-trip returns one `SlowTool` tool_use
/// and whose subsequent round-trips capture their `messages` then end the turn.
struct ToolUseThenCapturingApiClient {
    calls: AtomicUsize,
    later_messages: Mutex<Vec<Vec<ConversationMessage>>>,
}
impl ToolUseThenCapturingApiClient {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            later_messages: Mutex::new(Vec::new()),
        })
    }
}
#[async_trait]
impl crate::api::SubagentApiClient for ToolUseThenCapturingApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            return Ok(tool_use_response("SlowTool", Some("tool_use")));
        }
        self.later_messages.lock().unwrap().push(messages);
        Ok(text_response("done after direction", Some("end_turn")))
    }
}

/// cc 2.1.198 (M10): "Fixed an issue where messages sent by the agent that
/// launched a subagent could be treated as user approval" — a launcher/lead
/// message to a running subagent is NEW TASK DIRECTION and must NEVER satisfy
/// a pending permission request. Structurally, LingXi keeps the two channels
/// separate: permission approval reaches a pending prompt only through the
/// permission gate below the `ToolInvoker` seam, while a launcher message
/// arrives as `lingxi_core::Event::UserMessage` on the runner's event channel and
/// is appended to history as a user message. This test locks that separation:
/// with a permission prompt PENDING inside `invoke`, an inbound launcher
/// message (1) does not resolve/approve the prompt, (2) does not re-run the
/// tool, and (3) rides into the next round-trip as a plain user message AFTER
/// the gate's own deny result.
#[tokio::test]
async fn launcher_message_is_direction_not_approval_of_pending_permission() {
    let api = ToolUseThenCapturingApiClient::new();
    let invoker = PendingPermissionInvoker::new();
    let mut ctx = loop_ctx(
        api.clone(),
        Some(invoker.clone() as Arc<dyn platform_api::ToolInvoker>),
        4,
    );
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Wait until the tool call is in flight with its permission prompt pending.
    invoker.invoke_started.notified().await;

    // The launcher messages the running subagent (SendMessage → UserMessage).
    event_tx
        .send(lingxi_core::Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: "switch to auditing the docs instead".into(),
        })
        .await
        .unwrap();

    // The message must NOT satisfy the pending permission: the prompt is still
    // parked after the message has been delivered.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !invoker.resolved.load(Ordering::SeqCst),
        "a launcher message must never resolve a pending permission request"
    );

    // Only the permission gate's own channel resolves the prompt — as a DENY.
    invoker.release.notify_one();

    // The turn-set completes: deny tool_result fed back, launcher message
    // drained as task direction, final end_turn.
    let mut out_rx = out_rx;
    loop {
        let ev = out_rx.recv().await.expect("the turn-set completes");
        if matches!(ev, SubagentEvent::Completed { .. }) {
            break;
        }
    }

    // The tool ran exactly once — the message triggered no approval-driven
    // (re-)execution.
    assert_eq!(invoker.calls.load(Ordering::SeqCst), 1);

    // The next round-trip's history carries the gate's DENY as the tool_result
    // and the launcher message as a plain user TEXT message after it.
    let later = api.later_messages.lock().unwrap().clone();
    let hist = later.first().expect("second round-trip captured");
    let deny_idx = hist
        .iter()
        .position(|m| {
            matches!(m, ConversationMessage::User { content, .. }
                if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { content, is_error, .. }
                    if *is_error && content == "Error: Permission to use SlowTool has been denied.")))
        })
        .expect("the deny tool_result is in history");
    let direction_idx = hist
        .iter()
        .position(|m| {
            matches!(m, ConversationMessage::User { content, .. }
                if content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. }
                    if text.contains("switch to auditing the docs instead"))))
        })
        .expect("the launcher message rides as task direction");
    assert!(
        direction_idx > deny_idx,
        "direction is appended after the deny result, never in its place"
    );

    // Cooperative shutdown of the parked (persistent) runner.
    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    handle.await.unwrap();
}

// ── G4 (SubagentStart additionalContext) + G5 (skills preload) ──────────

/// `SubagentApiClient` that captures the `messages` of its FIRST round-trip
/// so a test can assert what the runner seeded as the child's initial
/// history (the preload messages are sent to the model, not emitted as
/// events). Replies with a single end_turn text turn.
struct CapturingApiClient {
    first_messages: Mutex<Option<Vec<ConversationMessage>>>,
}
impl CapturingApiClient {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            first_messages: Mutex::new(None),
        })
    }
    fn captured(&self) -> Vec<ConversationMessage> {
        self.first_messages
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }
}
#[async_trait]
impl crate::api::SubagentApiClient for CapturingApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        let mut slot = self.first_messages.lock().unwrap();
        if slot.is_none() {
            *slot = Some(messages);
        }
        Ok(text_response("done", Some("end_turn")))
    }
}

/// A `SubagentStart` builtin hook handler that returns one `additionalContext`
/// string so the runner injects it into the child's initial history (G4).
/// `handler_id` lets a test register more than one handler (distinct ids).
struct AdditionalContextStartHook {
    handler_id: String,
    context: String,
}
#[async_trait]
impl hooks::executor::BuiltinHookHandler for AdditionalContextStartHook {
    fn id(&self) -> &str {
        &self.handler_id
    }
    async fn handle(
        &self,
        _event: &hooks::events::HookEvent,
        _ctx: &hooks::registry::HookContext,
    ) -> hooks::response::HookResult {
        hooks::response::HookResult {
            outcome: hooks::response::HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: Some(hooks::response::HookResponse {
                additional_context: Some(self.context.clone()),
                ..Default::default()
            }),
        }
    }
}

/// Build an `Arc<HookExecutorImpl>` with ONE registered SubagentStart hook
/// that returns `context` as additionalContext.
async fn exec_with_start_context(context: &str) -> Arc<hooks::HookExecutorImpl> {
    use hooks::definition::{HookDefinition, HookExecutor, HookSource};
    use hooks::events::HookEventType;
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    registry.write().await.register(HookDefinition {
        id: protocol::HookId::new(),
        name: "additional-context-start".into(),
        events: vec![HookEventType::SubagentStart],
        if_condition: None,
        executor: HookExecutor::Builtin {
            handler_id: "additional-context-start".into(),
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
    });
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(AdditionalContextStartHook {
        handler_id: "additional-context-start".into(),
        context: context.to_string(),
    }));
    Arc::new(exec)
}

/// Build an `Arc<HookExecutorImpl>` with TWO registered SubagentStart hooks,
/// each returning its own additionalContext — to prove the runner JOINS them
/// into a single `<system-reminder>` message (claude byte-parity).
async fn exec_with_two_start_contexts(c0: &str, c1: &str) -> Arc<hooks::HookExecutorImpl> {
    use hooks::definition::{HookDefinition, HookExecutor, HookSource};
    use hooks::events::HookEventType;
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    for (i, handler_id) in ["start-ctx-0", "start-ctx-1"].iter().enumerate() {
        registry.write().await.register(HookDefinition {
            id: protocol::HookId::new(),
            name: (*handler_id).into(),
            events: vec![HookEventType::SubagentStart],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: (*handler_id).into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            // Distinct DESCENDING priorities pin the firing order so the
            // join is deterministic (c0 then c1). `match_event` sorts
            // priority-descending, so index 0 (priority 0) fires before
            // index 1 (priority -1).
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            priority: -(i as i32),
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        });
    }
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(AdditionalContextStartHook {
        handler_id: "start-ctx-0".into(),
        context: c0.to_string(),
    }));
    exec.register_builtin(Arc::new(AdditionalContextStartHook {
        handler_id: "start-ctx-1".into(),
        context: c1.to_string(),
    }));
    Arc::new(exec)
}

/// A builtin hook handler that records every `SubagentStop` it sees (the
/// `status` carried on the event) — used to prove a frontmatter
/// `Stop`→`SubagentStop` hook actually fires inside the child runner (#9).
struct RecordingStopHook {
    seen: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl hooks::executor::BuiltinHookHandler for RecordingStopHook {
    fn id(&self) -> &str {
        "record-subagent-stop-in-runner"
    }
    async fn handle(
        &self,
        event: &hooks::events::HookEvent,
        _ctx: &hooks::registry::HookContext,
    ) -> hooks::response::HookResult {
        if let hooks::events::HookEvent::SubagentStop { status, .. } = event {
            self.seen.lock().unwrap().push(status.clone());
        }
        hooks::response::HookResult {
            outcome: hooks::response::HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: None,
        }
    }
}

/// Build an executor wired with the [`RecordingStopHook`] builtin and NO
/// source/plugin hooks — so any SubagentStop the recorder sees must have come
/// from the agent-scoped frontmatter fire (the runner path), not a chokepoint.
fn exec_recording_stop(seen: Arc<Mutex<Vec<String>>>) -> Arc<hooks::HookExecutorImpl> {
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(RecordingStopHook { seen }));
    Arc::new(exec)
}

/// Records the `HookContext.last_assistant_message` carried by a runner-fired
/// agent-scoped `SubagentStop`.
struct RecordingStopContextHook {
    seen: Arc<Mutex<Vec<Option<String>>>>,
}
#[async_trait]
impl hooks::executor::BuiltinHookHandler for RecordingStopContextHook {
    fn id(&self) -> &str {
        "record-subagent-stop-context"
    }
    async fn handle(
        &self,
        event: &hooks::events::HookEvent,
        ctx: &hooks::registry::HookContext,
    ) -> hooks::response::HookResult {
        if matches!(event, hooks::events::HookEvent::SubagentStop { .. }) {
            let live = ctx
                .prompt_transcript
                .as_ref()
                .expect("worker Stop must carry live history");
            assert!(live.messages.iter().any(|message| matches!(message,
                ConversationMessage::User { content, .. } if content.iter().any(|block| matches!(block, protocol::ContentBlock::Text { text } if text == "go")))));
            assert!(live.messages.iter().any(|message| matches!(message,
                ConversationMessage::Assistant { content, .. } if content.iter().any(|block| matches!(block, protocol::ContentBlock::Text { text } if text == "done")))));
            self.seen
                .lock()
                .unwrap()
                .push(ctx.last_assistant_message.clone());
        }
        hooks::response::HookResult {
            outcome: hooks::response::HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: None,
        }
    }
}

fn exec_recording_stop_context(
    seen: Arc<Mutex<Vec<Option<String>>>>,
) -> Arc<hooks::HookExecutorImpl> {
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(RecordingStopContextHook { seen }));
    Arc::new(exec)
}

/// A frontmatter `Stop` hook (Builtin executor) the runner retargets to
/// `SubagentStop` (registerFrontmatterHooks isAgent=true).
fn frontmatter_stop_hook(handler_id: &str) -> hooks::definition::HookDefinition {
    use hooks::definition::{HookExecutor, HookSource};
    use hooks::events::HookEventType;
    hooks::definition::HookDefinition {
        id: protocol::HookId::new(),
        name: handler_id.into(),
        events: vec![HookEventType::Stop],
        if_condition: None,
        executor: HookExecutor::Builtin {
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

#[tokio::test]
async fn frontmatter_stop_hook_fires_as_subagent_stop_in_runner() {
    // #9: a frontmatter `Stop` hook is retargeted to `SubagentStop`
    // (isAgent=true) and MUST fire at the child loop's clean end — BEFORE
    // `clear_agent_hooks` removes it. A clean (end_turn) run yields one
    // `SubagentStop` with status "completed".
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "stop-agent".into();
    ctx.agent_definition.frontmatter_hooks =
        vec![frontmatter_stop_hook("record-subagent-stop-in-runner")];
    ctx.hook_executor = Some(exec_recording_stop(seen.clone()));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let recorded = seen.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec!["completed".to_string()],
        "frontmatter Stop→SubagentStop must fire exactly once (status completed): {recorded:?}"
    );
}

#[tokio::test]
async fn runner_subagent_stop_context_carries_final_assistant_text() {
    let seen = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "stop-agent".into();
    ctx.agent_definition.frontmatter_hooks =
        vec![frontmatter_stop_hook("record-subagent-stop-context")];
    ctx.hook_executor = Some(exec_recording_stop_context(seen.clone()));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![Some("done".to_string())],
        "agent-scoped SubagentStop should carry the final assistant text"
    );
}

#[tokio::test]
async fn no_frontmatter_hooks_means_no_runner_subagent_stop() {
    // With NO frontmatter hooks the runner takes the passthrough path and
    // fires NO agent-scoped SubagentStop (the orchestrator chokepoint owns
    // session/plugin SubagentStop). The recorder sees nothing.
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    // No frontmatter_hooks (default empty). Executor still wired.
    ctx.hook_executor = Some(exec_recording_stop(seen.clone()));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert!(
        seen.lock().unwrap().is_empty(),
        "no frontmatter hooks ⇒ no agent-scoped SubagentStop fired"
    );
}

/// Counts every SubagentStart and SubagentStop event the runner fires, so a
/// test can assert each canonical lifecycle hook fires EXACTLY once through
/// the REAL runner (R7 — no double-fire).
struct StartStopCounter {
    starts: Arc<Mutex<u32>>,
    stops: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl hooks::executor::BuiltinHookHandler for StartStopCounter {
    fn id(&self) -> &str {
        "r7-start-stop-counter"
    }
    async fn handle(
        &self,
        event: &hooks::events::HookEvent,
        _ctx: &hooks::registry::HookContext,
    ) -> hooks::response::HookResult {
        match event {
            hooks::events::HookEvent::SubagentStart { .. } => {
                *self.starts.lock().unwrap() += 1;
            }
            hooks::events::HookEvent::SubagentStop { status, .. } => {
                self.stops.lock().unwrap().push(status.clone());
            }
            _ => {}
        }
        hooks::response::HookResult {
            outcome: hooks::response::HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: None,
        }
    }
}

#[tokio::test]
async fn runner_fires_subagent_start_and_frontmatter_stop_exactly_once_each() {
    // R7 integration: a REAL runner run with BOTH a SubagentStart hook AND a
    // frontmatter `Stop`→`SubagentStop` hook (isAgent=true) must fire
    // SubagentStart EXACTLY once (the canonical, additionalContext-collecting
    // fire) and the frontmatter SubagentStop EXACTLY once (agent-scoped,
    // BEFORE clear_agent_hooks). This is the assertion the orchestrator-side
    // FakeAgentTool fixtures (no runner) cannot make.
    use hooks::definition::{HookDefinition, HookExecutor, HookSource};
    use hooks::events::HookEventType;

    let starts = Arc::new(Mutex::new(0u32));
    let stops = Arc::new(Mutex::new(Vec::<String>::new()));

    // One executor with: a SESSION-level SubagentStart hook (fires in G4) and
    // the frontmatter Stop hook is supplied via `frontmatter_hooks` below
    // (the runner registers + retargets it to SubagentStop, isAgent=true).
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    registry.write().await.register(HookDefinition {
        id: protocol::HookId::new(),
        name: "r7-start".into(),
        events: vec![HookEventType::SubagentStart],
        if_condition: None,
        executor: HookExecutor::Builtin {
            handler_id: "r7-start-stop-counter".into(),
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
    });
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(StartStopCounter {
        starts: starts.clone(),
        stops: stops.clone(),
    }));
    let exec = Arc::new(exec);

    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "r7-agent".into();
    // The frontmatter Stop hook points at the SAME counter handler; the
    // runner retargets Stop→SubagentStop (isAgent=true) and fires it
    // agent-scoped at the loop's clean end.
    ctx.agent_definition.frontmatter_hooks = vec![frontmatter_stop_hook("r7-start-stop-counter")];
    ctx.hook_executor = Some(exec);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        *starts.lock().unwrap(),
        1,
        "SubagentStart must fire EXACTLY once through the real runner (no double-fire)"
    );
    assert_eq!(
        stops.lock().unwrap().clone(),
        vec!["completed".to_string()],
        "frontmatter Stop→SubagentStop must fire EXACTLY once (status completed) in the runner"
    );
}

/// Mock [`SkillLoader`] that resolves a fixed name to canned content, else None.
struct MockSkillLoader {
    known: String,
    content_text: String,
}
#[async_trait]
impl platform_api::skill_loader::SkillLoader for MockSkillLoader {
    async fn resolve_and_load(
        &self,
        skill_name: &str,
        _agent_type: &str,
    ) -> Option<platform_api::skill_loader::SkillLoad> {
        if skill_name == self.known {
            Some(platform_api::skill_loader::SkillLoad {
                display_name: skill_name.to_string(),
                progress_message: None,
                content: vec![ContentBlock::Text {
                    text: self.content_text.clone(),
                }],
            })
        } else {
            None
        }
    }
}

fn user_text(msg: &ConversationMessage) -> Option<String> {
    if let ConversationMessage::User { content, .. } = msg {
        let t: Vec<String> = content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        Some(t.join("\n"))
    } else {
        None
    }
}

#[tokio::test]
async fn subagent_start_additional_context_injected_as_system_reminder() {
    // G4: a SubagentStart hook's additionalContext lands as a
    // `<system-reminder>` user message in the child's initial history,
    // AFTER the prompt seed and BEFORE turn 1.
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
    ctx.hook_executor = Some(exec_with_start_context("extra from hook").await);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let msgs = api.captured();
    // The prompt is first, the injected system-reminder follows it.
    let texts: Vec<String> = msgs.iter().filter_map(user_text).collect();
    assert!(
        texts.iter().any(|t| t == "do it"),
        "prompt seed present: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t
            == "<system-reminder>\nSubagentStart hook additional context: extra from hook\n</system-reminder>"),
        "additionalContext injected as the claude-byte <system-reminder> message: {texts:?}"
    );
}

// ── G008 (Fusion panel is hook-silent) ───────────────────────────────────

/// A `SubagentStart` builtin hook handler that BOTH counts every invocation
/// AND returns an `additionalContext` string, so a single fixture can pin
/// both halves of G008 at once: whether the hook machinery fired at all
/// (the count) and whether the message it would have produced landed in
/// history (the text). A hook that fires but returns nothing useful would
/// pass a text-only assertion while still violating the "chokepoint already
/// accounts for Fusion as ONE hook pair" contract in `build_preload_messages`
/// — hence asserting the call count separately from the message.
struct CountingAdditionalContextStartHook {
    calls: Arc<Mutex<u32>>,
    context: String,
}
#[async_trait]
impl hooks::executor::BuiltinHookHandler for CountingAdditionalContextStartHook {
    fn id(&self) -> &str {
        "g008-counting-start-hook"
    }
    async fn handle(
        &self,
        _event: &hooks::events::HookEvent,
        _ctx: &hooks::registry::HookContext,
    ) -> hooks::response::HookResult {
        *self.calls.lock().unwrap() += 1;
        hooks::response::HookResult {
            outcome: hooks::response::HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            response: Some(hooks::response::HookResponse {
                additional_context: Some(self.context.clone()),
                ..Default::default()
            }),
        }
    }
}

/// Build an `Arc<HookExecutorImpl>` with ONE registered SubagentStart hook
/// that both counts its invocations into `calls` and would inject `context`
/// as additionalContext if it fires.
async fn exec_with_counting_start_context(
    calls: Arc<Mutex<u32>>,
    context: &str,
) -> Arc<hooks::HookExecutorImpl> {
    use hooks::definition::{HookDefinition, HookExecutor, HookSource};
    use hooks::events::HookEventType;
    let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
    registry.write().await.register(HookDefinition {
        id: protocol::HookId::new(),
        name: "g008-counting-start".into(),
        events: vec![HookEventType::SubagentStart],
        if_condition: None,
        executor: HookExecutor::Builtin {
            handler_id: "g008-counting-start-hook".into(),
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
    });
    let mut exec = hooks::HookExecutorImpl::new(
        registry,
        Arc::new(test_harness::mocks::MockHttpTransport::new()),
        Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
    );
    exec.register_builtin(Arc::new(CountingAdditionalContextStartHook {
        calls,
        context: context.to_string(),
    }));
    Arc::new(exec)
}

#[tokio::test]
async fn fusion_panel_agent_type_fires_no_subagent_start_hook() {
    // G008 runner half: a `fusion-panel` child must invoke the SubagentStart
    // hook machinery ZERO times — the orchestrator chokepoint (turn_loop.rs)
    // already accounts for a whole Fusion run as a SINGLE hook pair via
    // `fusion_tool_result`'s `subagentHooksFired` marker. Deleting the
    // `agent_type != FUSION_PANEL_TYPE` guard in `build_preload_messages`
    // must turn this red.
    let calls = Arc::new(Mutex::new(0u32));
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = platform_api::FUSION_PANEL_TYPE.into();
    ctx.hook_executor = Some(exec_with_counting_start_context(calls.clone(), "panel-marker").await);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "fusion-panel agent_type must fire the SubagentStart hook handler ZERO times"
    );
    let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
    assert!(
        texts
            .iter()
            .all(|t| !t.contains("SubagentStart hook additional context")),
        "fusion-panel history must not contain a SubagentStart additionalContext message: {texts:?}"
    );
}

#[tokio::test]
async fn ordinary_agent_type_still_fires_subagent_start_hook_once() {
    // Companion to `fusion_panel_agent_type_fires_no_subagent_start_hook`:
    // proves the G008 gate is scoped to EXACTLY `fusion-panel` and cannot be
    // widened (e.g. inverted, or matched on a prefix) to also swallow
    // ordinary Agent-tool children — the hook must still fire once and its
    // additionalContext message must still land in history.
    let calls = Arc::new(Mutex::new(0u32));
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "ordinary-agent".into();
    ctx.hook_executor =
        Some(exec_with_counting_start_context(calls.clone(), "ordinary-marker").await);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "ordinary agent_type must still fire the SubagentStart hook handler exactly once"
    );
    let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
    assert!(
        texts.iter().any(|t| t
            == "<system-reminder>\nSubagentStart hook additional context: ordinary-marker\n</system-reminder>"),
        "ordinary agent_type history must contain the additionalContext message: {texts:?}"
    );
}

#[tokio::test]
async fn mobile_runtime_reminder_is_the_fixed_prefix_before_task_and_hooks() {
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
    ctx.mobile_runtime_environment_reminder = Some(Arc::from(
        "<system-reminder>\nMOBILE RUNTIME\n</system-reminder>",
    ));
    ctx.mobile_runtime_workspace_reminder = Some(Arc::from(
        "<system-reminder>\nMOBILE WORKSPACE\n</system-reminder>",
    ));
    ctx.hook_executor = Some(exec_with_start_context("extra from hook").await);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
    assert_eq!(
        texts[0],
        "<system-reminder>\nMOBILE RUNTIME\n</system-reminder>"
    );
    assert_eq!(
        texts[1],
        "<system-reminder>\nMOBILE WORKSPACE\n</system-reminder>"
    );
    assert_eq!(texts[2], "do it");
    assert!(texts[3].contains("SubagentStart hook additional context"));
}

#[tokio::test]
async fn subagent_start_multiple_contexts_join_into_one_reminder() {
    // G4 byte-parity (runAgent.ts:530-555 + messages.ts:4117-4128): when
    // multiple SubagentStart hooks each return additionalContext, claude
    // collects them into ONE `string[]` and emits a SINGLE
    // `hook_additional_context` attachment whose body is
    // `SubagentStart hook additional context: ` + contexts.join("\n").
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
    ctx.hook_executor = Some(exec_with_two_start_contexts("alpha", "beta").await);

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
    // Exactly ONE system-reminder message (not one per context).
    let reminders: Vec<&String> = texts
        .iter()
        .filter(|t| t.starts_with("<system-reminder>\nSubagentStart hook additional context: "))
        .collect();
    assert_eq!(
        reminders.len(),
        1,
        "exactly one joined SubagentStart reminder: {texts:?}"
    );
    assert_eq!(
        reminders[0],
        "<system-reminder>\nSubagentStart hook additional context: alpha\nbeta\n</system-reminder>",
        "contexts joined with \\n in a single reminder"
    );
}

#[tokio::test]
async fn no_hook_executor_means_no_preload_injection() {
    // G4: with hook_executor=None the child history carries ONLY the prompt
    // seed — byte-identical to legacy (no SubagentStart fire).
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
    // hook_executor + skill_loader both unset (default).

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let msgs = api.captured();
    assert_eq!(msgs.len(), 1, "only the prompt seed; got: {msgs:?}");
    assert_eq!(user_text(&msgs[0]).as_deref(), Some("do it"));
}

#[tokio::test]
async fn resolved_skill_prepends_metadata_then_content() {
    // G5: a resolved skill is injected as a user message whose first block is
    // the byte-locked loading metadata, followed by the loaded content.
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "my-agent".into();
    ctx.agent_definition.skills = vec!["my-skill".into()];
    ctx.skill_loader = Some(Arc::new(MockSkillLoader {
        known: "my-skill".into(),
        content_text: "SKILL BODY".into(),
    }));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let msgs = api.captured();
    // The skill meta message carries the <skill-format> marker block first,
    // then the loaded content.
    let skill_text = msgs
        .iter()
        .filter_map(user_text)
        .find(|t| t.contains("<skill-format>true</skill-format>"))
        .expect("skill meta message present");
    assert_eq!(
        skill_text,
        "<command-message>my-skill</command-message>\n\
<command-name>my-skill</command-name>\n\
<skill-format>true</skill-format>\nSKILL BODY",
        "metadata block then content"
    );
}

#[tokio::test]
async fn missing_skill_is_skipped_no_message() {
    // G5: an unresolved skill injects NOTHING (claude logs the warn + skips).
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "my-agent".into();
    ctx.agent_definition.skills = vec!["nope".into()];
    ctx.skill_loader = Some(Arc::new(MockSkillLoader {
        known: "my-skill".into(),
        content_text: "SKILL BODY".into(),
    }));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let msgs = api.captured();
    assert_eq!(
        msgs.len(),
        1,
        "only the prompt seed (missing skill skipped): {msgs:?}"
    );
}

#[tokio::test]
async fn preload_order_additional_context_then_skills() {
    // Ordering parity (runAgent.ts 530→577): additionalContext message(s)
    // come BEFORE the skills message(s) in the seeded history.
    let api = CapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 2);
    ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
    ctx.agent_definition.agent_type = "my-agent".into();
    ctx.agent_definition.skills = vec!["my-skill".into()];
    ctx.hook_executor = Some(exec_with_start_context("ctx0").await);
    ctx.skill_loader = Some(Arc::new(MockSkillLoader {
        known: "my-skill".into(),
        content_text: "SKILL BODY".into(),
    }));

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    drop(event_tx);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
    let ac_idx = texts
        .iter()
        .position(|t| t.contains("ctx0"))
        .expect("additionalContext present");
    let skill_idx = texts
        .iter()
        .position(|t| t.contains("<skill-format>"))
        .expect("skill present");
    assert!(
        ac_idx < skill_idx,
        "additionalContext must precede skills: {texts:?}"
    );
}

// ── P1-04: CC 2.1.207 subagent `api_error_partial` recovery ──────────────
//
// On a mid-stream API termination whose kind is in `CTy`
// ({rate_limit,overloaded,server_error}) AND with content already produced, the
// runner recovers the partial work as a `completed` result with the byte-locked
// `cutoffNote` prepended — instead of failing the tool and discarding the
// child's work (`Wyd`/`wTy`, AgentTool sync recovery).

/// Streaming mock whose per-turn scripts are RAW `Result<LlmEvent, LlmError>`
/// sequences, so a turn can inject a trailing MID-STREAM `Err` (no
/// `message_stop`). Overrides the streaming seam; the non-streaming path is
/// unreachable.
struct ResultStreamMockApiClient {
    turns: Mutex<VecDeque<Vec<Result<llm_client::LlmEvent, llm_client::LlmError>>>>,
    calls: AtomicUsize,
}
impl ResultStreamMockApiClient {
    fn new(turns: Vec<Vec<Result<llm_client::LlmEvent, llm_client::LlmError>>>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into_iter().collect()),
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl crate::api::SubagentApiClient for ResultStreamMockApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        unreachable!("streaming mock must be driven through messages_create_stream")
    }
    async fn messages_create_stream(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
        _effort: Option<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        use futures::StreamExt;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        Ok(futures::stream::iter(events).boxed())
    }
}

/// A partial streamed turn: one COMPLETED text block, then a mid-stream `Err`
/// (no `message_stop`). The block is salvageable; the error is not.
fn partial_text_then_err(
    text: &str,
    err: llm_client::LlmError,
) -> Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> {
    use llm_client::{ContentBlock as LB, ContentDelta, LlmEvent};
    vec![
        Ok(ev_message_start()),
        Ok(LlmEvent::ContentBlockStart {
            index: 0,
            content_block: LB::Text {
                text: String::new(),
                cache_control: None,
            },
        }),
        Ok(LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta { text: text.into() },
        }),
        Ok(LlmEvent::ContentBlockStop { index: 0 }),
        Err(err),
    ]
}

/// The exact `cutoffNote` for a server-error-class termination: the
/// `AgentApiErrorTerminationError` message + the byte-locked incomplete-output
/// notice (`\u{2014}` = em dash), joined by a blank line (two newlines), matching
/// CC 2.1.207/2.1.208 `wTy` (`cutoffNote:`+"${e.message}\n\n"+"Everything below…").
const EXPECTED_SERVER_ERROR_CUTOFF: &str = "Agent terminated early due to an API error: API Error: Server error mid-response. The response above may be incomplete.\n\nEverything below is PARTIAL output recovered from the agent before it was cut off. The agent did NOT finish its task \u{2014} treat these results as incomplete.";

/// A second round-trip cut off by `RateLimited` mid-stream — after a completed
/// first turn AND with a block salvaged from the failing turn — recovers as a
/// `Completed` result whose FIRST content block is the exact `cutoffNote`, with
/// the salvaged partial text following it. NOT a `Failed`.
#[tokio::test]
async fn rate_limit_midstream_recovers_partial_with_cutoff_note() {
    let dir = tempfile::tempdir().unwrap();
    // Turn 1: a complete tool_use turn (drives the loop into turn 2 after the
    // tool is dispatched). Turn 2: a partial text block then a mid-stream 429.
    let turn1: Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> =
        streamed_tool_use_turn("Read", "tool_use")
            .into_iter()
            .map(Ok)
            .collect();
    let turn2 = partial_text_then_err(
        "Partial answer before the cutoff",
        llm_client::LlmError::RateLimited {
            retry_after: None,
            scope: None,
        },
    );
    let api = ResultStreamMockApiClient::new(vec![turn1, turn2]);
    let api2 = api.clone();
    let invoker = CountingInvoker::new();
    let mut ctx = loop_ctx(api, Some(invoker), 10);
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    let agent_id = ctx.agent_id;
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;

    // No Failed event — the partial was preserved as a completion.
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "a recoverable mid-stream 429 must NOT surface as Failed: {evs:?}"
    );
    let result = evs
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("a Completed event carrying the recovered partial");

    let content = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .expect("content array");
    // First text block is the exact cutoffNote.
    assert_eq!(
        content[0].get("text").and_then(serde_json::Value::as_str),
        Some(EXPECTED_SERVER_ERROR_CUTOFF),
        "first content block must be the byte-exact cutoffNote"
    );
    // The salvaged partial text follows the note.
    assert!(
        content
            .iter()
            .any(|b| b.get("text").and_then(serde_json::Value::as_str)
                == Some("Partial answer before the cutoff")),
        "the salvaged mid-turn block must survive: {content:?}"
    );
    // Both round-trips were attempted (the loop reached turn 2 before erroring).
    assert_eq!(api2.call_count(), 2);
    let transcript = std::fs::read_to_string(dir.path().join(format!("agent-{agent_id}.jsonl")))
        .expect("partial completion must persist its transcript before Completed");
    assert!(
        transcript.contains("Partial answer before the cutoff"),
        "salvaged response must remain inspectable: {transcript}"
    );
    assert!(
        transcript.contains("\"status\":\"completed\""),
        "partial recovery must persist a true terminal marker: {transcript}"
    );
}

/// A qualifying error (`RateLimited`) at request-start on the FIRST turn — with
/// no content produced yet — still fails (CC's `Zor(r)===void 0` guard: nothing
/// to recover).
#[tokio::test]
async fn qualifying_error_with_no_content_fails() {
    let api = MockSubagentApiClient::new(vec![Err(llm_client::LlmError::RateLimited {
        retry_after: None,
        scope: None,
    })]);
    let ctx = loop_ctx(api, Some(CountingInvoker::new()), 10);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "a qualifying error with an empty transcript must Fail: {evs:?}"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Completed { .. })),
        "no partial exists to recover, so no Completed: {evs:?}"
    );
}

/// A NON-qualifying error (`QuotaExceeded`/`InvalidRequest` — kinds NOT in
/// `CTy`) fails even when content exists: CC rethrows these terminal API errors.
#[tokio::test]
async fn nonqualifying_error_after_content_still_fails() {
    // Turn 1 completes with a tool_use (content in history + tool dispatched);
    // turn 2 errors mid-stream with a NON-CTy kind → no recovery.
    let turn1: Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> =
        streamed_tool_use_turn("Read", "tool_use")
            .into_iter()
            .map(Ok)
            .collect();
    let turn2 = partial_text_then_err(
        "text that will NOT be recovered",
        llm_client::LlmError::QuotaExceeded,
    );
    let api = ResultStreamMockApiClient::new(vec![turn1, turn2]);
    let ctx = loop_ctx(api, Some(CountingInvoker::new()), 10);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let evs = drain(out_rx).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, SubagentEvent::Failed { .. })),
        "a non-CTy error must Fail even with content: {evs:?}"
    );
    // The salvaged text must NOT leak into any Completed result.
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SubagentEvent::Completed { .. })),
        "non-qualifying error must not recover a partial: {evs:?}"
    );
}

/// The per-agent transcript is actually WRITTEN. Before this, the
/// `SubagentStop` hook reported an `agent_transcript_path` while nothing
/// created the file — the payload named something that did not exist, and a
/// background agent's conversation lived only in memory.
#[tokio::test]
async fn run_subagent_persists_its_conversation_to_the_agent_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let api = MockSubagentApiClient::new(vec![Ok(text_response("the answer", Some("end_turn")))]);
    let mut ctx = loop_ctx(api, None, 4);
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    ctx.prompt_messages = vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "do the thing".to_string(),
    )];
    let agent_id = ctx.agent_id;

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let path = dir.path().join(format!("agent-{agent_id}.jsonl"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("transcript at {} should exist: {e}", path.display()));
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "transcript has content");
    for line in &lines {
        let entry: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
        assert!(entry.get("agent_id").is_some(), "stamped with its agent");
        assert!(entry.get("message").is_some(), "carries the message");
    }
    assert!(
        body.contains("\"status\":\"completed\""),
        "terminal completion is persisted for session-agent status discovery"
    );
    // The SEEDED prompt is on disk, not only the assistant turns — a resume
    // needs the conversation from its start.
    assert!(
        body.contains("do the thing"),
        "the seeded prompt is persisted: {body}"
    );
    assert!(
        body.contains("the answer"),
        "the reply is persisted: {body}"
    );
}

/// A child transcript is also the live UI's source of truth. The seeded user
/// prompt must therefore be durable and observable before the first model
/// response; otherwise a stalled first request leaves the agent detail screen
/// empty even though the model already received its task.
#[tokio::test]
async fn run_subagent_exposes_seed_before_first_model_response() {
    let dir = tempfile::tempdir().unwrap();
    let api = StuckThenCapturingApiClient::new();
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    ctx.prompt_messages = vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "design the airplane game".to_string(),
    )];
    let agent_id = ctx.agent_id;

    let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, mut out_rx) = mpsc::channel::<SubagentEvent>(16);
    let task = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    api.first_call_started.notified().await;

    let path = dir.path().join(format!("agent-{agent_id}.jsonl"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("seed transcript at {} should exist: {e}", path.display()));
    assert!(
        body.contains("design the airplane game"),
        "the prompt must be readable while the first request is still pending: {body}"
    );

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
        .await
        .expect("seed message event before the first model response")
        .expect("runner output remains open");
    let SubagentEvent::Message { message, .. } = event else {
        panic!("expected the seeded prompt as the first live message, got {event:?}");
    };
    let message: ConversationMessage =
        serde_json::from_value(message).expect("seed event is a conversation message");
    assert_eq!(message.text_content(), "design the airplane game");

    event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
    task.await.unwrap();
}

/// A host that wires no transcript filesystem persists nothing and behaves
/// exactly as before — the seam is additive.
#[tokio::test]
async fn run_subagent_without_a_transcript_fs_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let api = MockSubagentApiClient::new(vec![Ok(text_response("x", Some("end_turn")))]);
    let mut ctx = loop_ctx(api, None, 4);
    ctx.transcript_subdir = dir.path().to_path_buf();
    let agent_id = ctx.agent_id;

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    assert!(!dir.path().join(format!("agent-{agent_id}.jsonl")).exists());
}

/// A RESTORED agent is seeded from its recovered conversation, and that history
/// REPLACES the normal seeding rather than prefixing it: the prompt, the fork
/// context and the `SubagentStart` / skills preload are all already inside the
/// recovered messages, so re-adding them would duplicate context the agent has
/// seen and re-fire start hooks for a run that began in another process.
#[tokio::test]
async fn a_resumed_history_replaces_the_seed_rather_than_prefixing_it() {
    let api = MockSubagentApiClient::new(vec![Ok(text_response("ok", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.prompt_messages = vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "ORIGINAL PROMPT".to_string(),
    )];
    ctx.fork_context_messages = Some(vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "FORK CONTEXT".to_string(),
    )]);
    ctx.resumed_history = Some(vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "RECOVERED".to_string(),
    )]);
    ctx.mobile_runtime_environment_reminder = Some(Arc::from(
        "<system-reminder>\nMOBILE RUNTIME MUST NOT DUPLICATE\n</system-reminder>",
    ));
    ctx.mobile_runtime_workspace_reminder = Some(Arc::from(
        "<system-reminder>\nMOBILE WORKSPACE MUST NOT DUPLICATE\n</system-reminder>",
    ));

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let sent = api.last_messages();
    let rendered = format!("{sent:?}");
    assert!(
        rendered.contains("RECOVERED"),
        "resumed history is sent: {rendered}"
    );
    assert!(
        !rendered.contains("ORIGINAL PROMPT"),
        "the prompt is NOT re-sent: {rendered}"
    );
    assert!(
        !rendered.contains("FORK CONTEXT"),
        "the fork-context prefix is NOT re-added: {rendered}"
    );
    assert!(
        !rendered.contains("MOBILE RUNTIME MUST NOT DUPLICATE"),
        "the runtime reminder is already persisted in recovered history and is NOT re-added: {rendered}"
    );
    assert!(
        !rendered.contains("MOBILE WORKSPACE MUST NOT DUPLICATE"),
        "the workspace reminder is already persisted in recovered history and is NOT re-added: {rendered}"
    );
}

/// A RESTORED run must not re-append its recovered conversation to the
/// transcript — the watermark starts past it, so the file grows by what is new
/// rather than doubling every time the agent is restored.
#[tokio::test]
async fn a_restored_run_appends_only_new_messages_to_its_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let api = MockSubagentApiClient::new(vec![Ok(text_response("fresh reply", Some("end_turn")))]);
    let mut ctx = loop_ctx(api, None, 4);
    ctx.transcript_subdir = dir.path().to_path_buf();
    ctx.transcript_fs = Some(Arc::new(platform_posix::PosixFileSystem::new(
        dir.path().to_path_buf(),
    )) as Arc<dyn platform_api::FileSystem>);
    ctx.resumed_history = Some(vec![protocol::ConversationMessage::user(
        MessageId::new(),
        "ALREADY ON DISK".to_string(),
    )]);
    let agent_id = ctx.agent_id;

    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    run_subagent(ctx, event_rx, out_tx).await;
    let _ = drain(out_rx).await;

    let body = std::fs::read_to_string(dir.path().join(format!("agent-{agent_id}.jsonl"))).unwrap();
    assert!(
        !body.contains("ALREADY ON DISK"),
        "the recovered conversation is not written a second time: {body}"
    );
    assert!(
        body.contains("fresh reply"),
        "new turns are appended: {body}"
    );
}

/// A stalled stream must NOT be recovered as `api_error_partial`.
///
/// Claude Code 2.1.238 classifies it through `xtt` as `"api_timeout"`, and
/// `CTy = {"rate_limit","overloaded","server_error"}` does not contain that, so
/// the oracle rethrows. Recovering it instead is a one-bit difference with a
/// large blast radius: the caller receives a `completed` result whose content
/// says the agent was cut off and did nothing, which a workflow stage records
/// as output and moves past. Three stalled generate attempts each "succeeded"
/// that way before the verify stage noticed the app was still the template.
#[test]
fn a_stalled_stream_is_terminal_and_an_ordinary_interruption_still_recovers() {
    use llm_client::model::stream_watchdog::{STREAM_IDLE_TIMEOUT_PREFIX, STREAM_SUSPENDED_PREFIX};

    for message in [
        format!("{STREAM_IDLE_TIMEOUT_PREFIX}: no bytes for 300000ms"),
        format!("{STREAM_SUSPENDED_PREFIX}; aborting to retry on a fresh connection"),
    ] {
        assert!(
            super::classify_api_termination(&llm_client::LlmError::StreamInterrupted {
                message: message.clone(),
            })
            .is_none(),
            "a stall must rethrow, not recover: {message}"
        );
    }

    // The kinds CTy DOES contain keep recovering, so this change cannot be
    // mistaken for "stop recovering api errors".
    for error in [
        llm_client::LlmError::Overloaded { repeated: false },
        llm_client::LlmError::ProviderInternal,
        llm_client::LlmError::StreamInterrupted {
            message: "stream ended before message_stop".to_string(),
        },
    ] {
        assert!(
            super::classify_api_termination(&error).is_some(),
            "{error:?} is in CTy and must still recover as api_error_partial"
        );
    }
}

// ---- Fusion panel input cap must not split tool_use/tool_result pairs ----

#[tokio::test]
async fn oversized_mandatory_prompt_is_rejected_before_provider_call() {
    let api = MockSubagentApiClient::new(vec![Ok(text_response("must not run", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 1);
    ctx.prompt_messages = vec![ConversationMessage::user(
        MessageId::new(),
        "TASK-MARKER: ".to_string() + &"x".repeat(10_000),
    )];
    ctx.max_input_bytes_per_turn = Some(128);
    let (_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

    run_subagent(ctx, event_rx, out_tx).await;
    let events = drain(out_rx).await;
    assert_eq!(
        api.call_count(),
        0,
        "the provider must not receive an over-cap mandatory prompt"
    );
    assert!(events.iter().any(|event| {
        matches!(event, SubagentEvent::Failed { error, .. } if error.contains("mandatory initial prompt exceeds"))
    }));
}

/// Build an assistant message whose only content block is a `ToolUse`.
fn assistant_tool_use(id: ToolUseId, name: &str) -> ConversationMessage {
    ConversationMessage::Assistant {
        id: MessageId::new(),
        content: vec![ContentBlock::ToolUse {
            id,
            name: name.to_string(),
            input: serde_json::json!({}),
            provider_id: None,
        }],
        stop_reason: Some("tool_use".into()),
    }
}

/// Build a user message whose only content block is the matching `ToolResult`.
fn user_tool_result(tool_use_id: ToolUseId, content: &str) -> ConversationMessage {
    ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id,
            content: content.to_string(),
            is_error: false,
            provider_tool_use_id: None,
            content_blocks: None,
        }],
        is_meta: false,
        is_compact_summary: false,
        is_visible_in_transcript_only: false,
    }
}

/// A byte cap tight enough to keep only the LAST message by itself would,
/// under naive whole-message trimming from the tail, keep a trailing
/// `ToolResult` while dropping the `ToolUse` message that produced it —
/// wire-invalid history (a `tool_result` with no matching `tool_use` in the
/// same request). `cap_input_bytes` must keep or drop such a pair together.
#[test]
fn cap_input_bytes_keeps_tool_use_and_tool_result_paired() {
    let tool_id = ToolUseId::new();
    let history = vec![
        assistant_text("turn one, filler text to add up bytes so the cap bites here"),
        assistant_tool_use(tool_id.clone(), "Read"),
        user_tool_result(tool_id.clone(), "file contents"),
    ];
    // The complete seed + tool pair fits. The cap implementation must retain
    // that pair atomically, rather than trimming one half to satisfy a
    // per-message approximation.
    let max = serde_json::to_vec(&history).unwrap().len() as u64;
    let capped = super::cap_input_bytes(&history, Some(max)).expect("cap fits");

    let has_tool_use = capped.iter().any(|m| {
        matches!(m, ConversationMessage::Assistant { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if *id == tool_id)))
    });
    let has_tool_result = capped.iter().any(|m| {
        matches!(m, ConversationMessage::User { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if *tool_use_id == tool_id)))
    });
    assert_eq!(
        has_tool_use, has_tool_result,
        "cap_input_bytes split a tool_use/tool_result pair: tool_use kept={has_tool_use}, tool_result kept={has_tool_result}"
    );
    assert!(
        has_tool_result,
        "the pair that fits the budget must be kept, not dropped entirely"
    );
}

/// F009: the task seed and newest tool pair are both mandatory. Even when
/// each fits separately, their exact serialized array may not; reject that
/// request instead of splitting the pair or exceeding the declared cap.
#[test]
fn cap_input_bytes_rejects_task_plus_latest_pair_over_the_exact_cap() {
    let tool_id = ToolUseId::new();
    let prompt = ConversationMessage::user(MessageId::new(), "TASK-MARKER".repeat(20));
    let pair = [
        assistant_tool_use(tool_id.clone(), "Read"),
        user_tool_result(tool_id, &"result".repeat(20)),
    ];
    let mut history = vec![prompt.clone()];
    history.extend(pair);
    let exact_bytes = u64::try_from(serde_json::to_vec(&history).unwrap().len()).unwrap();
    let cap = exact_bytes - 1;
    assert!(u64::try_from(serde_json::to_vec(&vec![prompt]).unwrap().len()).unwrap() <= cap);

    let error = super::cap_input_bytes(&history, Some(cap))
        .expect_err("the mandatory task/latest pair is one serialized byte over cap");
    assert!(error.contains("mandatory latest tool/message unit exceeds"));
}

/// [Finding 9] A Fusion panel seeds its ENTIRE task text as `history`'s
/// first unit (`ctx.prompt_messages`, extended in before the first turn)
/// and nothing re-injects it on later turns — it is the only production
/// caller that sets `max_input_bytes_per_turn` (`panel.rs`'s
/// `spawn_request`). The tail-only fill in `cap_input_bytes` must not evict
/// that first unit while a newer tool-result pair still fits the budget: two
/// large tool-result pairs from earlier turns must not silently erase the
/// task, leaving the panel to emit a `PanelReport` written against no task
/// at all.
#[test]
fn cap_input_bytes_pins_the_task_prompt_when_tool_results_crowd_it_out() {
    let prompt = ConversationMessage::user(MessageId::new(), "TASK-MARKER: what is 2+2?".into());
    let tool_id_1 = ToolUseId::new();
    let tool_id_2 = ToolUseId::new();
    let pair1 = [
        assistant_tool_use(tool_id_1.clone(), "Read"),
        user_tool_result(tool_id_1, &"y".repeat(50_000)),
    ];
    let pair2 = [
        assistant_tool_use(tool_id_2.clone(), "Read"),
        user_tool_result(tool_id_2, &"z".repeat(50_000)),
    ];
    let mut history = vec![prompt.clone()];
    history.extend(pair1.iter().cloned());
    history.extend(pair2.iter().cloned());

    let prompt_bytes = serde_json::to_vec(&prompt).unwrap().len() as u64;
    let newest_pair_bytes: u64 = pair2
        .iter()
        .map(|m| serde_json::to_vec(m).unwrap().len() as u64)
        .sum();
    // Room for the prompt plus exactly the NEWEST pair, not both pairs.
    let max = prompt_bytes + newest_pair_bytes + 16;

    let capped = super::cap_input_bytes(&history, Some(max)).expect("cap fits");
    let joined = format!("{capped:?}");
    assert!(
        joined.contains("TASK-MARKER"),
        "the panel's task prompt must survive per-turn trimming while a \
         newer tool-result pair still fits the budget; capped history: {joined}"
    );
    assert!(
        !joined.contains(&"y".repeat(50_000)),
        "the OLDER, over-budget tool-result pair must still be dropped — a \
         `cap_input_bytes` that just returned the whole history unchanged \
         would also contain TASK-MARKER, so this must go red on its own"
    );
}

/// F009: a task prompt that cannot fit the declared cap must be rejected.
/// Sending it whole violates the cap, while dropping it silently removes the
/// only task-carrying unit from later turns.
#[test]
fn cap_input_bytes_rejects_when_the_task_prompt_alone_exceeds_the_cap() {
    let prompt = ConversationMessage::user(
        MessageId::new(),
        format!("TASK-MARKER: {}", "x".repeat(50_000)),
    );
    let tool_id = ToolUseId::new();
    let pair = [
        assistant_tool_use(tool_id.clone(), "Read"),
        user_tool_result(tool_id, "small result"),
    ];
    let mut history = vec![prompt];
    history.extend(pair.iter().cloned());

    let pair_bytes: u64 = pair
        .iter()
        .map(|m| serde_json::to_vec(m).unwrap().len() as u64)
        .sum();
    // Comfortably fits the newest pair, nowhere near fitting the ~50 KB
    // prompt too.
    let max = pair_bytes + 32;

    let error = super::cap_input_bytes(&history, Some(max))
        .expect_err("an oversized mandatory prompt must be rejected before sending");
    assert!(
        error.contains("mandatory initial prompt exceeds"),
        "the rejection must explain the mandatory prompt cap violation: {error}"
    );
}

/// [F009] A mandatory oversized seed must be rejected instead of being sent
/// whole (over the declared cap) or silently dropped (losing the task).
#[test]
fn cap_input_bytes_rejects_an_oversized_head_without_sending_it() {
    let prompt = ConversationMessage::user(MessageId::new(), "x".repeat(50_000));
    let pair1 = [
        assistant_tool_use(ToolUseId::new(), "Read"),
        user_tool_result(ToolUseId::new(), &"y".repeat(200)),
    ];
    let pair2 = [
        assistant_tool_use(ToolUseId::new(), "Read"),
        user_tool_result(ToolUseId::new(), &"z".repeat(50_000)),
    ];
    let mut history = vec![prompt];
    history.extend(pair1.iter().cloned());
    history.extend(pair2.iter().cloned());

    let pair1_bytes: u64 = pair1
        .iter()
        .map(|m| serde_json::to_vec(m).unwrap().len() as u64)
        .sum();
    // Room for one small pair, nowhere near enough for the ~50 KB mandatory
    // head.
    let max = pair1_bytes + 16;

    let error = super::cap_input_bytes(&history, Some(max))
        .expect_err("an oversized mandatory prompt must be rejected before sending");
    assert!(error.contains("mandatory initial prompt exceeds"));
}

struct OwnerNotificationRegistry {
    rest_acknowledged: AtomicBool,
    wake_checked: tokio::sync::Notify,
    drains: AtomicUsize,
    parked_fold: tokio::sync::Notify,
    owner: protocol::AgentId,
    pending: Mutex<Vec<platform_api::task_registry::TaskNotification>>,
    revision: tokio::sync::watch::Sender<u64>,
}
impl OwnerNotificationRegistry {
    fn publish(&self) {
        self.pending
            .lock()
            .unwrap()
            .push(platform_api::task_registry::TaskNotification {
                task_id: "achild".into(),
                task_type: "local_agent".into(),
                status: "completed".into(),
                recipient_agent_id: Some(self.owner),
                description: "child finished".into(),
                ..Default::default()
            });
        self.revision.send_modify(|n| *n += 1);
    }
}
#[async_trait]
impl platform_api::task_registry::TaskRegistryHandle for OwnerNotificationRegistry {
    async fn create(
        &self,
        _: platform_api::task_registry::TaskCreateInput,
    ) -> Result<
        platform_api::task_registry::TaskRecord,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn get(
        &self,
        _: &str,
    ) -> Result<
        Option<platform_api::task_registry::TaskRecord>,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn list(
        &self,
        _: platform_api::task_registry::TaskListFilter,
    ) -> Result<
        Vec<platform_api::task_registry::TaskRecord>,
        platform_api::task_registry::TaskRegistryError,
    > {
        Ok(vec![])
    }
    async fn update(
        &self,
        _: &str,
        _: platform_api::task_registry::TaskUpdatePatch,
    ) -> Result<
        platform_api::task_registry::TaskRecord,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn set_status(
        &self,
        _: &str,
        _: &str,
    ) -> Result<
        platform_api::task_registry::TaskRecord,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn kill(
        &self,
        _: &str,
    ) -> Result<
        platform_api::task_registry::TaskRecord,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn output(
        &self,
        _: &str,
        _: Option<u64>,
    ) -> Result<
        platform_api::task_registry::TaskOutputChunk,
        platform_api::task_registry::TaskRegistryError,
    > {
        unreachable!()
    }
    async fn can_wake_agent_for_task_notification(&self, _: protocol::AgentId) -> bool {
        let acknowledged = self.rest_acknowledged.load(Ordering::SeqCst);
        self.wake_checked.notify_one();
        acknowledged
    }
    fn subscribe_task_notifications(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(self.revision.subscribe())
    }
    async fn take_pending_task_notifications_for(
        &self,
        recipient: Option<protocol::AgentId>,
    ) -> Result<
        Vec<platform_api::task_registry::TaskNotification>,
        platform_api::task_registry::TaskRegistryError,
    > {
        assert_eq!(recipient, Some(self.owner));
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        if self.drains.fetch_add(1, Ordering::SeqCst) >= 2 {
            self.parked_fold.notify_one();
        }
        Ok(pending)
    }
}

#[tokio::test]
async fn owner_notification_wakes_parked_runner_without_user_message() {
    let api = MockSubagentApiClient::new(vec![
        Ok(text_response("first", Some("end_turn"))),
        Ok(text_response("second", Some("end_turn"))),
    ]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;
    let registry = Arc::new(OwnerNotificationRegistry {
        rest_acknowledged: AtomicBool::new(true),
        wake_checked: tokio::sync::Notify::new(),
        drains: AtomicUsize::new(0),
        parked_fold: tokio::sync::Notify::new(),
        owner: ctx.agent_id,
        pending: Mutex::new(vec![]),
        revision: tokio::sync::watch::channel(0).0,
    });
    ctx.task_registry = Some(registry.clone());
    let (event_tx, event_rx) = mpsc::channel(8);
    let (out_tx, mut out_rx) = mpsc::channel(32);
    let runner = tokio::spawn(run_subagent(ctx, event_rx, out_tx));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !matches!(out_rx.recv().await, Some(SubagentEvent::Completed { .. })) {}
        registry.parked_fold.notified().await;
        registry.publish();
        while !matches!(out_rx.recv().await, Some(SubagentEvent::Completed { .. })) {}
    })
    .await
    .unwrap();
    assert_eq!(api.call_count(), 2);
    let json = serde_json::to_string(&api.last_messages()).unwrap();
    assert_eq!(json.matches("<task-id>achild</task-id>").count(), 1);
    drop(event_tx);
    runner.await.unwrap();
}

struct NotificationDuringRequestApi {
    registry: Arc<OwnerNotificationRegistry>,
    completed: AtomicBool,
    calls: AtomicUsize,
    last_messages: Mutex<Vec<ConversationMessage>>,
}
#[async_trait]
impl crate::api::SubagentApiClient for NotificationDuringRequestApi {
    async fn messages_create(
        &self,
        _: &str,
        _: Option<&str>,
        messages: Vec<ConversationMessage>,
        _: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        *self.last_messages.lock().unwrap() = messages;
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.registry.publish();
            // Notification arrives with a genuinely pending provider future.
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            self.completed.store(true, Ordering::SeqCst);
        }
        Ok(text_response("done", Some("end_turn")))
    }
}
#[tokio::test]
async fn owner_notification_folds_after_inflight_request_without_cancelling_it() {
    let mut ctx = fresh_subagent_ctx();
    let registry = Arc::new(OwnerNotificationRegistry {
        rest_acknowledged: AtomicBool::new(true),
        wake_checked: tokio::sync::Notify::new(),
        drains: AtomicUsize::new(0),
        parked_fold: tokio::sync::Notify::new(),
        owner: ctx.agent_id,
        pending: Mutex::new(vec![]),
        revision: tokio::sync::watch::channel(0).0,
    });
    let api = Arc::new(NotificationDuringRequestApi {
        registry: registry.clone(),
        completed: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        last_messages: Mutex::new(vec![]),
    });
    ctx.api_client = Some(api.clone());
    ctx.task_registry = Some(registry);
    ctx.agent_definition.max_turns = 4;
    let (event_tx, event_rx) = mpsc::channel(8);
    let (out_tx, mut out_rx) = mpsc::channel(32);
    let runner = tokio::spawn(run_subagent(ctx, event_rx, out_tx));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !matches!(out_rx.recv().await, Some(SubagentEvent::Completed { .. })) {}
    })
    .await
    .unwrap();
    assert!(
        api.completed.load(Ordering::SeqCst),
        "original provider future survived notification"
    );
    assert_eq!(api.calls.load(Ordering::SeqCst), 2);
    assert!(serde_json::to_string(&*api.last_messages.lock().unwrap())
        .unwrap()
        .contains("<task-id>achild</task-id>"));
    drop(event_tx);
    runner.await.unwrap();
}

#[tokio::test]
async fn owner_notification_waits_for_handler_rest_acknowledgement() {
    let api = MockSubagentApiClient::new(vec![Ok(text_response("first", Some("end_turn"))), Ok(text_response("second", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;
    let registry = Arc::new(OwnerNotificationRegistry {
        rest_acknowledged: AtomicBool::new(false), wake_checked: tokio::sync::Notify::new(),
        drains: AtomicUsize::new(0), parked_fold: tokio::sync::Notify::new(), owner: ctx.agent_id,
        pending: Mutex::new(vec![]), revision: tokio::sync::watch::channel(0).0,
    });
    ctx.task_registry = Some(registry.clone());
    let (event_tx, event_rx) = mpsc::channel(8);
    let (out_tx, mut out_rx) = mpsc::channel(32);
    let runner = tokio::spawn(run_subagent(ctx, event_rx, out_tx));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !matches!(out_rx.recv().await, Some(SubagentEvent::Completed { .. })) {}
        registry.wake_checked.notified().await;
        registry.publish();
        registry.wake_checked.notified().await;
        assert_eq!(api.call_count(), 1, "pending notification cannot outrun handler rest acknowledgement");
        registry.rest_acknowledged.store(true, Ordering::SeqCst);
        registry.revision.send_modify(|revision| *revision += 1);
        while !matches!(out_rx.recv().await, Some(SubagentEvent::Completed { .. })) {}
    }).await.unwrap();
    assert_eq!(api.call_count(), 2);
    drop(event_tx);
    runner.await.unwrap();
}

// ── Subagent refusal cascade ───────────────────────────────────────────────
//
// claude-code runs subagents through the SAME query generator as the main
// thread, so a refusing subagent hops to the fallback model and retries. This
// port's subagent loop is separate and treated `refusal` as an ordinary
// terminal stop reason, so the run simply ended.

#[tokio::test]
async fn a_refusing_subagent_hops_to_the_fallback_and_retries() {
    let api = StreamingMockApiClient::new(vec![
        streamed_text_turn("", "refusal"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let mut ctx = loop_ctx(api.clone(), None, 3);
    ctx.refusal_fallback_chain = vec!["fallback-model".to_string()];

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;

    let events = drain(out_rx).await;
    let text = events
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed { result, .. } => {
                Some(result["text"].as_str().unwrap_or_default().to_string())
            }
            _ => None,
        })
        .expect("the run completed");
    assert!(
        text.contains("done"),
        "the retry's answer is the run's result: {text:?}"
    );
    // The answer also carries the `ICe` note naming the model that produced it
    // — see `a_hopped_subagents_answer_carries_the_refusal_note`.
    assert!(text.contains('\u{26A0}'), "…prefixed by the note: {text:?}");
    assert_eq!(
        api.call_count(),
        2,
        "a refusal with a hop left must re-issue, not end the run"
    );
    assert_eq!(
        api.models().get(1).map(String::as_str),
        Some("fallback-model"),
        "the retry must go to the fallback, not back to the refusing model: {:?}",
        api.models()
    );
}

/// The control: with no chain configured a refusal is still terminal, which is
/// every subagent's behaviour before this. Without it the test above would
/// pass just as well if the loop retried unconditionally.
#[tokio::test]
async fn a_refusal_with_no_chain_configured_still_ends_the_run() {
    let api = StreamingMockApiClient::new(vec![
        streamed_text_turn("", "refusal"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let ctx = loop_ctx(api.clone(), None, 3);
    assert!(
        ctx.refusal_fallback_chain.is_empty(),
        "precondition: nothing configured"
    );

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;

    assert_eq!(
        api.call_count(),
        1,
        "no chain ⇒ the refusal is terminal, as before"
    );
}

/// The cascade is bounded by its chain: each hop is consumed, so a subagent
/// that keeps refusing stops rather than looping over the same models.
#[tokio::test]
async fn a_subagent_cascade_stops_when_the_chain_is_exhausted() {
    let api = StreamingMockApiClient::new(vec![
        streamed_text_turn("", "refusal"),
        streamed_text_turn("", "refusal"),
        streamed_text_turn("", "refusal"),
        streamed_text_turn("done", "end_turn"),
    ]);
    let mut ctx = loop_ctx(api.clone(), None, 8);
    ctx.refusal_fallback_chain = vec!["hop-one".to_string(), "hop-two".to_string()];

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;

    assert_eq!(
        api.call_count(),
        3,
        "the original call plus one per chain entry, then terminal: {:?}",
        api.models()
    );
    assert_eq!(
        api.models()[1..].to_vec(),
        vec!["hop-one".to_string(), "hop-two".to_string()],
        "each hop is consumed once, in order"
    );
}

/// A subagent's swap is `scope: "local"` — it lasts for this run and does not
/// touch the session model, unlike the main thread's, which is `"session"`.
/// claude-code's `ICe` looks for exactly the local one.
#[test]
fn a_subagent_refusal_frame_is_scoped_local() {
    let frame = super::refusal_fallback_frame(
        MessageId::new(),
        &platform_api::refusal_notice::RefusalNotice {
            origin_model: "refusing-model".to_string(),
            serving_model: "fallback-model".to_string(),
            ..platform_api::refusal_notice::RefusalNotice::default()
        },
    );
    match frame {
        ConversationMessage::System {
            subtype,
            refusal_fallback: Some(meta),
            ..
        } => {
            assert_eq!(subtype.as_deref(), Some("model_refusal_fallback"));
            assert_eq!(meta.scope.as_deref(), Some("local"));
            assert_eq!(meta.original_model, "refusing-model");
            assert_eq!(meta.fallback_model, "fallback-model");
        }
        other => panic!("expected a typed system frame, got {other:?}"),
    }
}

// ── `ICe` / `PZo`: the harness note and the retraction filter ───────────────

/// `iht`'s `⚠ ${notice.content}` note. The parent asked a subagent a question
/// and got an answer from a DIFFERENT model than it dispatched; upstream says
/// so in the result. Without the note the swap is invisible to the caller.
#[tokio::test]
async fn a_hopped_subagents_answer_carries_the_refusal_note() {
    let api = StreamingMockApiClient::new(vec![
        streamed_text_turn("", "refusal"),
        streamed_text_turn("the answer", "end_turn"),
    ]);
    let mut ctx = loop_ctx(api.clone(), None, 3);
    ctx.refusal_fallback_chain = vec!["fallback-model".to_string()];

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;

    let events = drain(out_rx).await;
    let result = events
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("the run completed");
    let text = result["text"].as_str().unwrap_or_default();
    assert!(
        text.contains('\u{26A0}') && text.contains("fallback-model"),
        "the answer must name the model that actually produced it: {text:?}"
    );
    assert!(
        text.contains("the answer"),
        "and it must still carry the report: {text:?}"
    );
    let first = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        first.starts_with('\u{26A0}'),
        "the note is a leading block, ahead of the report: {first:?}"
    );
}

/// The control: a run that never hopped has no note. Without it the test above
/// would pass just as well if the note were unconditional.
#[tokio::test]
async fn a_subagent_that_never_refused_carries_no_note() {
    let api = StreamingMockApiClient::new(vec![streamed_text_turn("the answer", "end_turn")]);
    let mut ctx = loop_ctx(api.clone(), None, 3);
    ctx.refusal_fallback_chain = vec!["fallback-model".to_string()];

    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_tx);
    let (out_tx, out_rx) = mpsc::channel(32);
    run_subagent(ctx, event_rx, out_tx).await;

    let events = drain(out_rx).await;
    let result = events
        .iter()
        .find_map(|e| match e {
            SubagentEvent::Completed { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("the run completed");
    assert_eq!(result["text"].as_str(), Some("the answer"));
}

/// `PZo` — a notice that supersedes an earlier hop names the messages that hop
/// produced, and those must not survive into the answer. System messages always
/// do: the notices are how the retraction is expressed at all.
#[test]
fn retracted_messages_are_dropped_but_notices_survive() {
    let assistant = |text: &str| ConversationMessage::Assistant {
        id: MessageId::new(),
        content: vec![protocol::ContentBlock::Text {
            text: text.to_string(),
        }],
        stop_reason: None,
    };
    let doomed = assistant("superseded output");
    let doomed_uuid = doomed.id().as_uuid().to_string();
    let kept = assistant("live output");
    let notice = super::refusal_fallback_frame(
        MessageId::new(),
        &platform_api::refusal_notice::RefusalNotice {
            origin_model: "refusing".to_string(),
            serving_model: "fallback".to_string(),
            retracted_message_uuids: vec![doomed_uuid],
            ..platform_api::refusal_notice::RefusalNotice::default()
        },
    );

    let live = super::drop_retracted(&[doomed, notice, kept]);

    assert_eq!(live.len(), 2, "the superseded message is gone: {live:?}");
    assert!(
        live.iter()
            .any(|m| matches!(m, ConversationMessage::System { .. })),
        "the notice itself survives"
    );
    assert!(
        live.iter().any(|m| matches!(
            m,
            ConversationMessage::Assistant { content, .. }
                if content.iter().any(|b| matches!(b, protocol::ContentBlock::Text { text } if text == "live output"))
        )),
        "the unretracted message survives"
    );
}

/// A notice for a model that is NOT the one serving the answer is not this
/// run's explanation — matching upstream's `fallbackModel === answer's model`.
#[test]
fn a_notice_for_another_model_is_not_picked() {
    let notice = super::refusal_fallback_frame(
        MessageId::new(),
        &platform_api::refusal_notice::RefusalNotice {
            serving_model: "hop-one".to_string(),
            ..platform_api::refusal_notice::RefusalNotice::default()
        },
    );
    let history = vec![notice];
    assert!(super::local_refusal_notice(&history, "hop-two").is_none());
    assert!(super::local_refusal_notice(&history, "hop-one").is_some());
}
