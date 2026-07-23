//! Extracted tests for `agent::runner`.

use super::*;
use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use crate::display::{AgentColor, AgentDisplay};
use async_trait::async_trait;
use engine::token::Usage;
use protocol::{ContentBlock, ConversationMessage, MessageId, RequestId, ToolUseId};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// ---- Scripted loop-mode fixtures -------------------------------------

/// `SubagentApiClient` that hands back a pre-scripted queue of responses,
/// one per `messages_create` call. Counts calls so tests can assert the
/// number of model round-trips (`max_turns` bound, multi-turn loop).
struct MockSubagentApiClient {
    responses: Mutex<VecDeque<Result<llm_client::LlmResponse, llm_client::LlmError>>>,
    calls: AtomicUsize,
}

impl MockSubagentApiClient {
    fn new(responses: Vec<Result<llm_client::LlmResponse, llm_client::LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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
}

impl StreamingMockApiClient {
    fn new(turns: Vec<Vec<llm_client::LlmEvent>>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into_iter().collect()),
            calls: AtomicUsize::new(0),
            last_tools: Mutex::new(Vec::new()),
        })
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
        _model: &str,
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
        *self.last_tools.lock().unwrap() = tools;
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
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
impl traits::ToolInvoker for CountingInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: traits::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
impl traits::budget::BudgetEnforcerHandle for MockBudget {
    async fn check_and_charge(&self, _: u64) -> Result<(), traits::budget::BudgetError> {
        if self.exceeded {
            Err(traits::budget::BudgetError::Exceeded {
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
    tool_invoker: Option<Arc<dyn traits::ToolInvoker>>,
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
        },
        prompt_messages: vec![],
        fork_context_messages: None,
        allowed_tools: vec![],
        worktree_handle: None,
        cwd: None,
        is_async: false,
        persistent: false,
        can_show_permission_prompts: true,
        mcp_clients: vec![],
        transcript_subdir: "/tmp".into(),
        rendered_system_prompt: Some(Arc::from("")),
        content_replacement_state: None,
        agent_memory: None,
        display: AgentDisplay {
            color: AgentColor::Cyan,
            icon: None,
        },
        model_profile: None,
        api_client: None,
        tool_invoker: None,
        tool_schemas: vec![],
        schema: None,
        budget: None,
        hook_executor: None,
        skill_loader: None,
        hook_session_id: protocol::SessionId::nil(),
        hook_cwd: std::path::PathBuf::new(),
        depth: 0,
        permission_mode_override: None,
    }
}

/// Drain the `SubagentEvent` receiver into a `Vec`.
async fn drain(mut rx: mpsc::Receiver<SubagentEvent>) -> Vec<SubagentEvent> {
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
        .send(engine::Event::UserMessage {
            message_id: msg_id,
            request_id: req,
            content: "hi".into(),
        })
        .await
        .unwrap();
    event_tx
        .send(engine::Event::ApiStreamStart { request_id: req })
        .await
        .unwrap();
    event_tx
        .send(engine::Event::ApiStreamEnd {
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Drive directly to terminal via UserExit. The fast-path in the
    // runner short-circuits to Killed on this input event.
    event_tx.send(engine::Event::UserExit).await.unwrap();
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    event_tx.send(engine::Event::UserInterrupt).await.unwrap();
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
async fn loop_end_turn_with_tool_use_still_dispatches_then_completes() {
    // MAJOR #1 regression: an `end_turn` response that ALSO carries a
    // tool_use must NOT silently drop the tool. The reference dispatches
    // tools whenever present, then terminates on end_turn. Assert the tool
    // was invoked AND the run completed in a single turn (no continuation).
    let api = MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("end_turn")))]);
    let invoker = CountingInvoker::new();
    let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
async fn loop_tool_use_without_invoker_fails() {
    // A tool_use with tool_invoker = None surfaces Failed.
    let api = MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("tool_use")))]);
    let ctx = loop_ctx(api.clone(), None, 4);

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    event_tx.send(engine::Event::UserInterrupt).await.unwrap();

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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
        .send(engine::Event::UserMessage {
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
async fn persist_mode_terminates_on_channel_close_after_turn_set() {
    // With persistent = true, closing the event channel after the first
    // turn-set completes makes the parked runner return gracefully (no
    // further events, no Failed).
    let api = MockSubagentApiClient::new(vec![Ok(text_response("done", Some("end_turn")))]);
    let mut ctx = loop_ctx(api.clone(), None, 4);
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    event_tx.send(engine::Event::UserExit).await.unwrap();
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Wait until the first round-trip is in flight (stuck).
    api.first_call_started.notified().await;

    // Message the stuck teammate.
    event_tx
        .send(engine::Event::UserMessage {
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
    event_tx.send(engine::Event::UserExit).await.unwrap();
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
impl traits::ToolInvoker for PendingPermissionInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: traits::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.invoke_started.notify_one();
        self.release.notified().await;
        self.resolved.store(true, Ordering::SeqCst);
        Err(traits::tool_invoker::ToolInvokerError::Internal(
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
/// arrives as `engine::Event::UserMessage` on the runner's event channel and
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
        Some(invoker.clone() as Arc<dyn traits::ToolInvoker>),
        4,
    );
    ctx.persistent = true;

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
    let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
    let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

    // Wait until the tool call is in flight with its permission prompt pending.
    invoker.invoke_started.notified().await;

    // The launcher messages the running subagent (SendMessage → UserMessage).
    event_tx
        .send(engine::Event::UserMessage {
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
    event_tx.send(engine::Event::UserExit).await.unwrap();
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
impl traits::skill_loader::SkillLoader for MockSkillLoader {
    async fn resolve_and_load(
        &self,
        skill_name: &str,
        _agent_type: &str,
    ) -> Option<traits::skill_loader::SkillLoad> {
        if skill_name == self.known {
            Some(traits::skill_loader::SkillLoad {
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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

    let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let ctx = loop_ctx(api, Some(invoker), 10);
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
    let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
