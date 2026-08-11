//! In-process teammate tests.
#![allow(clippy::unwrap_used)]

use super::*;
use std::collections::HashMap as StdHashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

use test_harness::mocks::MockRuntimeSpawner;
use tokio::sync::Mutex as TokioMutex;
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use traits::RuntimeSpawner;

// ---- In-memory FileSystem (mirrors the other handler tests) ------------

struct InMemoryFs {
    files: TokioMutex<StdHashMap<String, String>>,
}
impl InMemoryFs {
    fn new() -> Self {
        Self {
            files: TokioMutex::new(StdHashMap::new()),
        }
    }
}
#[async_trait]
impl FileSystem for InMemoryFs {
    async fn read_file(
        &self,
        path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
    ) -> Result<FileContent, FsError> {
        let map = self.files.lock().await;
        let content = map.get(path).cloned().unwrap_or_default();
        let total_lines = content.lines().count() as u64;
        Ok(FileContent {
            content,
            truncated: false,
            total_lines,
        })
    }
    async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        let mut map = self.files.lock().await;
        map.entry(path.to_string()).or_default().push_str(body);
        Ok(())
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
        Ok(())
    }
    async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }
    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let map = self.files.lock().await;
        Ok(map.get(path).map_or(0, |s| s.len() as u64))
    }
    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
        Ok(())
    }
    async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn fsync(&self, _: &str) -> Result<(), FsError> {
        Ok(())
    }
}

// ---- Scripted SubagentApiClient ----------------------------------------

/// Hands back a pre-scripted queue of responses, one per `messages_create`.
/// When exhausted it returns an `end_turn` text turn so each turn-set
/// terminates and the persistent runner parks for the next message. An
/// `Err` entry surfaces as an API error (driving the runner to `Failed`).
struct ScriptedApiClient {
    responses: StdMutex<VecDeque<Result<llm_client::LlmResponse, String>>>,
    calls: AtomicUsize,
}
impl ScriptedApiClient {
    fn new(texts: Vec<&str>) -> Arc<Self> {
        let responses = texts
            .into_iter()
            .map(|t| Ok(text_response(t)))
            .collect::<VecDeque<_>>();
        Arc::new(Self {
            responses: StdMutex::new(responses),
            calls: AtomicUsize::new(0),
        })
    }
    /// Script a single API error so the first turn-set surfaces `Failed`.
    fn new_error(message: &str) -> Arc<Self> {
        let mut responses = VecDeque::new();
        responses.push_back(Err(message.to_string()));
        Arc::new(Self {
            responses: StdMutex::new(responses),
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl SubagentApiClient for ScriptedApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<protocol::ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.responses.lock().unwrap().pop_front();
        match next {
            Some(Ok(resp)) => Ok(resp),
            Some(Err(msg)) => Err(llm_client::LlmError::InvalidRequest { message: msg }),
            None => Ok(text_response("(idle)")),
        }
    }
}

fn text_response(text: &str) -> llm_client::LlmResponse {
    llm_client::LlmResponse {
        id: "mock".into(),
        model: "mock".into(),
        content: vec![llm_client::ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        stop_reason: Some("end_turn".into()),
        stop_details: None,
        usage: llm_client::Usage::default(),
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

// ---- Recording status sink ---------------------------------------------

#[derive(Default)]
struct RecordingSink {
    statuses: StdMutex<Vec<(String, TaskStatus)>>,
    /// Failure reasons received through the `set_failed` seam (cc 2.1.198:
    /// the failed idle notification's `failureReason` to the lead).
    failures: StdMutex<Vec<(String, String)>>,
}
#[async_trait]
impl TaskStatusSink for RecordingSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }
    async fn set_failed(&self, task_id: &str, error: &str) {
        self.failures
            .lock()
            .unwrap()
            .push((task_id.to_string(), error.to_string()));
        self.set_status(task_id, TaskStatus::Failed).await;
    }
}
impl RecordingSink {
    fn last_status(&self) -> Option<TaskStatus> {
        self.statuses.lock().unwrap().last().map(|(_, s)| *s)
    }
    fn failures(&self) -> Vec<(String, String)> {
        self.failures.lock().unwrap().clone()
    }
}

// ---- Recording TeammateIdle firer --------------------------------------

/// A [`hooks::TeammateIdleFirer`] that records every fire it receives, so a
/// test can assert the per-turn-set idle moment fired with the right payload.
#[derive(Default)]
struct RecordingIdleFirer {
    fires: StdMutex<Vec<hooks::TeammateIdleFire>>,
}
#[async_trait]
impl hooks::TeammateIdleFirer for RecordingIdleFirer {
    async fn fire(&self, fire: hooks::TeammateIdleFire) {
        self.fires.lock().unwrap().push(fire);
    }
}
impl RecordingIdleFirer {
    fn fires(&self) -> Vec<hooks::TeammateIdleFire> {
        self.fires.lock().unwrap().clone()
    }
}

// ---- Helpers ------------------------------------------------------------

fn make_handler(
    api: Arc<ScriptedApiClient>,
) -> (
    tempfile::TempDir,
    Arc<dyn FileSystem>,
    Arc<MockRuntimeSpawner>,
    InProcessTeammateHandler,
) {
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let output = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let pool = Arc::new(StateMachinePool::new(
        runtime.clone() as Arc<dyn RuntimeSpawner>,
        8,
    ));
    let handler = InProcessTeammateHandler::new(pool, output, api);
    (dir, fs, runtime, handler)
}

/// Like [`make_handler`] but returns the attached [`RecordingSink`] so a
/// test can observe terminal status transitions.
fn make_handler_with_sink(
    api: Arc<ScriptedApiClient>,
) -> (
    tempfile::TempDir,
    Arc<dyn FileSystem>,
    Arc<MockRuntimeSpawner>,
    InProcessTeammateHandler,
    Arc<RecordingSink>,
) {
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let output = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let pool = Arc::new(StateMachinePool::new(
        runtime.clone() as Arc<dyn RuntimeSpawner>,
        8,
    ));
    let sink = Arc::new(RecordingSink::default());
    let handler = InProcessTeammateHandler::new(pool, output, api).with_status_sink(sink.clone());
    (dir, fs, runtime, handler, sink)
}

/// Yield until the sink reports a terminal status, or the budget runs out.
async fn await_terminal(sink: &Arc<RecordingSink>) -> Option<TaskStatus> {
    for _ in 0..400 {
        if let Some(s) = sink.last_status() {
            if s.is_terminal() {
                return Some(s);
            }
        }
        tokio::task::yield_now().await;
    }
    sink.last_status()
}

fn ctx(fs: Arc<dyn FileSystem>, runtime: Arc<MockRuntimeSpawner>) -> TaskContext {
    TaskContext {
        fs,
        runtime: runtime as Arc<dyn RuntimeSpawner>,
    }
}

/// Yield until `pred` over the spool body holds, or the budget runs out.
async fn await_spool<F: Fn(&str) -> bool>(
    fs: &Arc<dyn FileSystem>,
    spool: &str,
    pred: F,
) -> String {
    for _ in 0..400 {
        let body = fs.read_file(spool, None, None).await.unwrap().content;
        if pred(&body) {
            return body;
        }
        tokio::task::yield_now().await;
    }
    fs.read_file(spool, None, None).await.unwrap().content
}

// ---- Tests --------------------------------------------------------------

/// Minimal handler for the `build_context` model-resolution tests (no spawn).
fn model_test_handler(default_model: Option<&str>) -> InProcessTeammateHandler {
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let output = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let pool = Arc::new(StateMachinePool::new(runtime as Arc<dyn RuntimeSpawner>, 8));
    let handler = InProcessTeammateHandler::new(pool, output, ScriptedApiClient::new(vec!["ok"]));
    match default_model {
        Some(m) => handler.with_default_model(m),
        None => handler,
    }
}

#[tokio::test]
async fn build_context_resolves_inherit_to_default_model() {
    // DefaultTeammateDefinition yields AgentModel::Inherit; with a default
    // model wired the teammate ctx carries a concrete wire id (folded via
    // the same `resolve_agent_model` seam as the spawner).
    let handler = model_test_handler(Some("claude-opus-4-7"));
    let def = DefaultTeammateDefinition
        .resolve(&protocol::AgentId::new(), "lead")
        .unwrap();
    let ctx = handler
        .build_context(
            protocol::AgentId::new(),
            "lead",
            "alpha",
            "go research",
            def,
        )
        .await
        .expect("context should build");
    assert!(
        matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-opus-4-7")
    );
    // Swarm identity threaded onto the context (claude-code
    // `TeammateContext.agentName` / `.teamName`).
    assert_eq!(ctx.agent_name.as_deref(), Some("lead"));
    assert_eq!(ctx.team_name.as_deref(), Some("alpha"));
    // The TeamCreate description becomes the teammate's first user message.
    assert_eq!(ctx.prompt_messages.len(), 1);
    assert_eq!(ctx.prompt_messages[0].text_content(), "go research");
}

// Full teammate parity (P1): the handler inherits a budget enforcer, seeds
// the TeamCreate description as the first user message, and runs the shared
// tool resolver when a registry is wired (advertising a real pool instead of
// the prior chat-only empty set).
#[tokio::test]
async fn build_context_wires_budget_description_and_tool_resolution() {
    struct DummyBudget;
    #[async_trait]
    impl traits::budget::BudgetEnforcerHandle for DummyBudget {
        async fn check_and_charge(&self, _: u64) -> Result<(), traits::budget::BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }
    let handler = model_test_handler(Some("claude-opus-4-7"))
        .with_budget_enforcer(Arc::new(DummyBudget))
        // An EMPTY registry still exercises the resolution path (returns an
        // empty pool); a populated registry is covered by the agent crate's
        // `resolve_subagent_tools` tests (shared code path).
        .with_tool_registry(Arc::new(agent::ToolRegistry::new()));
    let def = DefaultTeammateDefinition
        .resolve(&protocol::AgentId::new(), "lead")
        .unwrap();
    let ctx = handler
        .build_context(
            protocol::AgentId::new(),
            "lead",
            "alpha",
            "do the task",
            def,
        )
        .await
        .expect("context should build");
    // Budget inherited (was None before this fix).
    assert!(
        ctx.budget.is_some(),
        "teammate must inherit the budget enforcer"
    );
    // Description seeded as the first user message (was empty before).
    assert_eq!(ctx.prompt_messages.len(), 1);
    assert_eq!(ctx.prompt_messages[0].text_content(), "do the task");
    // The resolver ran (empty registry ⇒ empty pool, but the path is wired —
    // no panic, and the allow-list mirrors the advertised set).
    assert_eq!(ctx.tool_schemas.len(), ctx.allowed_tools.len());
}

#[tokio::test]
async fn build_context_without_default_model_leaves_model_raw() {
    // No default model wired → legacy behavior: Inherit is left untouched.
    let handler = model_test_handler(None);
    let def = DefaultTeammateDefinition
        .resolve(&protocol::AgentId::new(), "lead")
        .unwrap();
    let ctx = handler
        .build_context(protocol::AgentId::new(), "lead", "", "", def)
        .await
        .expect("context should build");
    assert!(matches!(&ctx.agent_definition.model, AgentModel::Inherit));
    // Empty team_name spawns standalone → team_name is None (leader default).
    assert_eq!(ctx.agent_name.as_deref(), Some("lead"));
    assert_eq!(ctx.team_name, None);
    // Empty description ⇒ no seed message (parks awaiting first injection).
    assert!(ctx.prompt_messages.is_empty());
}

// ---- #15: opusplan + plan mode resolves an Inherit teammate to Opus -------
//
// The composition root threads `with_permission_mode(cfg.permission_mode)` +
// `with_model_setting(cfg.default_model)` (the RAW user alias, e.g.
// "opusplan") onto the handler alongside
// `with_default_model(resolve_user_specified_model(orch_cfg.model))` — the
// RESOLVED main-loop id (Sonnet for an opusplan install). To stay FAITHFUL to
// production these tests derive the parent the SAME way: feed
// `resolve_user_specified_model("opusplan")` (= "claude-sonnet-5" since the
// 2.1.197 sonnet-family default flip, M1) as
// `with_default_model`, not a hand-picked literal the wired path never emits.
// These three together drive `resolve_agent_model`'s `getRuntimeMainLoopModel`
// branch (model.ts:145-167), proving the wired path end-to-end: an
// `AgentModel::Inherit` teammate on an `opusplan` install IN PLAN MODE resolves
// to Opus (without `[1m]`), NOT the resolved Sonnet main-loop model — i.e. the
// plan-mode swap fires through the builders the composition root populates.

#[tokio::test]
async fn build_context_opusplan_plan_mode_resolves_inherit_to_opus() {
    // Pin firstParty so `getDefaultOpusModel()` is deterministic regardless of
    // any provider env this process inherits.
    let _g = OpusEnvGuard::clear_providers();
    // Derive the parent EXACTLY as the composition root does: resolve the raw
    // "opusplan" alias to the main-loop wire id (Sonnet outside plan mode) —
    // proving the swap below is to OPUS, not a pass-through of a literal.
    let parent = agent::model_resolution::resolve_user_specified_model("opusplan");
    let handler = model_test_handler(Some(&parent))
        .with_permission_mode(PermissionMode::Plan)
        .with_model_setting("opusplan");
    let def = DefaultTeammateDefinition
        .resolve(&protocol::AgentId::new(), "lead")
        .unwrap();
    let ctx = handler
        .build_context(protocol::AgentId::new(), "lead", "", "", def)
        .await
        .expect("context should build");
    assert!(
        matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-opus-4-8"),
        "opusplan + plan mode must resolve an Inherit teammate to Opus, got {:?}",
        ctx.agent_definition.model
    );
}

#[tokio::test]
async fn build_context_opusplan_default_mode_returns_resolved_parent() {
    // Same opusplan setting but NOT in plan mode → the Inherit branch returns
    // the resolved main-loop model unchanged (Sonnet), proving the swap is
    // gated on plan mode (not on the setting alone). Parent derived via the
    // resolver, exactly as the composition root produces it.
    let _g = OpusEnvGuard::clear_providers();
    let parent = agent::model_resolution::resolve_user_specified_model("opusplan");
    assert_eq!(
        parent, "claude-sonnet-5",
        "opusplan resolves to Sonnet outside plan mode"
    );
    let handler = model_test_handler(Some(&parent))
        .with_permission_mode(PermissionMode::Default)
        .with_model_setting("opusplan");
    let def = DefaultTeammateDefinition
        .resolve(&protocol::AgentId::new(), "lead")
        .unwrap();
    let ctx = handler
        .build_context(protocol::AgentId::new(), "lead", "", "", def)
        .await
        .expect("context should build");
    assert!(
        matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-sonnet-5"),
        "opusplan outside plan mode must keep the resolved parent (Sonnet), got {:?}",
        ctx.agent_definition.model
    );
}

/// RAII guard that clears the three provider env vars (Bedrock/Vertex/Foundry)
/// for the duration of a test so `getDefaultOpusModel()` resolves on the
/// firstParty branch deterministically (mirrors `model_resolution`'s test
/// guard). Restores prior values on drop.
struct OpusEnvGuard {
    prev: Vec<(&'static str, Option<String>)>,
}
impl OpusEnvGuard {
    fn clear_providers() -> Self {
        let keys = [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ];
        let prev = keys
            .iter()
            .map(|k| {
                let v = std::env::var(k).ok();
                std::env::remove_var(k);
                (*k, v)
            })
            .collect();
        Self { prev }
    }
}
impl Drop for OpusEnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.prev {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[tokio::test]
async fn name_type_and_supports_messages() {
    let api = ScriptedApiClient::new(vec![]);
    let (_d, _fs, _rt, handler) = make_handler(api);
    assert_eq!(handler.name(), "in_process_teammate");
    assert_eq!(handler.task_type(), TaskType::InProcessTeammate);
    assert!(handler.supports_messages(), "teammates accept messages");
}

#[tokio::test]
async fn non_teammate_input_is_rejected() {
    let api = ScriptedApiClient::new(vec![]);
    let (_d, fs, rt, handler) = make_handler(api);
    let res = handler
        .spawn(
            TaskSpawnInput::LocalBash {
                command: "echo".into(),
                timeout: None,
            },
            ctx(fs, rt),
        )
        .await;
    assert!(matches!(res, Err(TaskError::Internal(_))));
}

#[tokio::test]
async fn spawn_send_message_then_kill_lifecycle() {
    // The persistent runner runs turn-set 1 immediately on spawn (empty
    // history -> "answer one"), then parks. An injected message un-idles it
    // and drives turn-set 2 ("answer two"), proving persistence (the slot
    // did not terminate after turn-set 1). The streaming worker spools a
    // `completed:` line per turn-set. kill then tears the slot down.
    let api = ScriptedApiClient::new(vec!["answer one", "answer two"]);
    let api_handle = api.clone();
    let (dir, fs, rt, handler) = make_handler(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();
    assert!(h.task_id.starts_with('t'), "teammate ids prefix 't'");
    assert!(h.cleanup.is_some(), "cleanup hook present");
    assert_eq!(
        handler.entries.lock().await.len(),
        1,
        "spawn registers slot"
    );

    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();

    // Turn-set 1 completes shortly after spawn.
    let body = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
    assert!(body.contains("completed:"), "turn-set 1 spooled: {body:?}");
    assert!(body.contains("answer one"), "turn-set 1 text: {body:?}");

    // Inject a message — drives turn-set 2 after the runner had idled.
    handler
        .send_message(&h.task_id, "follow-up question".into(), c.clone())
        .await
        .expect("send_message routes to the live slot while idle");
    let body = await_spool(&fs, &spool_str, |b| b.contains("answer two")).await;
    assert!(body.contains("answer two"), "turn-set 2 text: {body:?}");
    assert_eq!(api_handle.call_count(), 2, "one round-trip per turn-set");

    // Kill tears it down; the entry is removed.
    handler.kill(&h.task_id, c.clone()).await.unwrap();
    assert!(
        handler.entries.lock().await.is_empty(),
        "kill deregisters the slot"
    );

    // Killing an unknown id is a graceful no-op.
    handler
        .kill("tdeadbeef", c)
        .await
        .expect("kill of unknown id is a no-op success");
}

#[tokio::test]
async fn send_message_to_unknown_task_is_not_found() {
    let api = ScriptedApiClient::new(vec![]);
    let (_d, fs, rt, handler) = make_handler(api);
    let err = handler
        .send_message("tnope", "hi".into(), ctx(fs, rt))
        .await
        .unwrap_err();
    assert!(matches!(err, TaskError::NotFound(_)), "got {err:?}");
}

/// A teammate whose first turn-set hits an API error surfaces `Failed`: the
/// streaming worker spools a `failed:` line and reports `TaskStatus::Failed`
/// (the terminal-break branch — distinct from the non-terminal per-turn-set
/// `Completed`).
#[tokio::test]
async fn failed_turn_set_spools_failed_and_reports_terminal() {
    let api = ScriptedApiClient::new_error("boom");
    let (dir, fs, rt, handler, sink) = make_handler_with_sink(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c,
        )
        .await
        .unwrap();

    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();

    let body = await_spool(&fs, &spool_str, |b| b.contains("failed:")).await;
    assert!(body.contains("failed:"), "failed line spooled: {body:?}");
    assert!(body.contains("boom"), "error text spooled: {body:?}");

    assert_eq!(
        await_terminal(&sink).await,
        Some(TaskStatus::Failed),
        "Failed event reports terminal TaskStatus::Failed"
    );
}

/// cc 2.1.198 (M9): a teammate dying on an API error reports "failed" WITH the
/// failure reason through the `set_failed` seam — the reason the lead-facing
/// `CoordinatorStatusSink` surfaces on the worker (binary @216293689: the
/// in-process runner's catch sends `{idleReason:"failed",
/// completedStatus:"failed", failureReason}` to the leader). Not a bare
/// `set_status(Failed)` that would drop the error text.
#[tokio::test]
async fn failed_turn_set_reports_error_reason_through_set_failed() {
    let api = ScriptedApiClient::new_error("rate limited: 529 overloaded");
    let (_dir, fs, rt, handler, sink) = make_handler_with_sink(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c,
        )
        .await
        .unwrap();

    assert_eq!(await_terminal(&sink).await, Some(TaskStatus::Failed));
    let failures = sink.failures();
    assert_eq!(failures.len(), 1, "exactly one set_failed: {failures:?}");
    assert_eq!(failures[0].0, h.task_id, "keyed on the teammate task id");
    assert!(
        failures[0].1.contains("rate limited: 529 overloaded"),
        "the REAL error text reaches the sink (lead), not a sentinel: {failures:?}"
    );
}

/// `send_message` to a teammate that was already killed (slot deallocated +
/// entry removed) is `NotFound`; and a message routed to a slot whose runner
/// has terminated (its receiver dropped) surfaces `TerminatedTask`. We drive
/// the latter by killing the slot via the pool directly so the handler's
/// entry still exists but the underlying slot is gone.
#[tokio::test]
async fn send_message_after_runner_terminated_is_terminated_task() {
    let api = ScriptedApiClient::new(vec!["answer one"]);
    let (dir, fs, rt, handler) = make_handler(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();

    // Let turn-set 1 land so the runner is parked and reachable.
    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();
    await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;

    // Tear down the underlying slot out-of-band (UserExit so the runner
    // drops its receiver), WITHOUT removing the handler's entry. The
    // handler still has a control block, so send_message looks the slot up
    // and finds the inbound channel closed -> AgentGone -> TerminatedTask.
    let aid = handler
        .entries
        .lock()
        .await
        .get(&h.task_id)
        .map(|e| e.agent_id)
        .unwrap();
    handler
        .pool
        .send_event(&aid, engine::Event::UserExit)
        .await
        .unwrap();
    // Wait until the slot's inbound channel is observably closed.
    for _ in 0..400 {
        if handler
            .pool
            .send_event(&aid, engine::Event::UserInterrupt)
            .await
            .is_err()
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    let err = handler
        .send_message(&h.task_id, "are you there?".into(), c)
        .await
        .unwrap_err();
    assert!(
        matches!(err, TaskError::TerminatedTask),
        "send to a terminated runner maps to TerminatedTask; got {err:?}"
    );
}

/// Like [`make_handler`] but attaches a [`RecordingIdleFirer`] (plus a
/// `RecordingSink`) so a test can observe the per-turn-set `TeammateIdle`
/// fire.
fn make_handler_with_idle_firer(
    api: Arc<ScriptedApiClient>,
) -> (
    tempfile::TempDir,
    Arc<dyn FileSystem>,
    Arc<MockRuntimeSpawner>,
    InProcessTeammateHandler,
    Arc<RecordingIdleFirer>,
) {
    let dir = tempfile::tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let output = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let pool = Arc::new(StateMachinePool::new(
        runtime.clone() as Arc<dyn RuntimeSpawner>,
        8,
    ));
    let firer = Arc::new(RecordingIdleFirer::default());
    let handler = InProcessTeammateHandler::new(pool, output, api)
        .with_teammate_idle_firer(firer.clone() as Arc<dyn hooks::TeammateIdleFirer>);
    (dir, fs, runtime, handler, firer)
}

/// Yield until the firer has recorded at least `n` fires, or the budget runs
/// out. Returns the recorded fires.
#[allow(clippy::similar_names)] // `fires` is the plural noun form of `firer`'s fires method
async fn await_fires(firer: &Arc<RecordingIdleFirer>, n: usize) -> Vec<hooks::TeammateIdleFire> {
    for _ in 0..400 {
        let fires = firer.fires();
        if fires.len() >= n {
            return fires;
        }
        tokio::task::yield_now().await;
    }
    firer.fires()
}

/// A wired `TeammateIdleFirer` receives one fire per completed turn-set —
/// the "about to go idle" moment — carrying the teammate name and the team
/// name threaded from the spawn input. Driving a second turn-set via an
/// injected message proves it fires again each time the teammate parks.
#[tokio::test]
#[allow(clippy::similar_names)] // `fires` (results) vs `firer` (sender) are semantically distinct
async fn completed_turn_set_fires_teammate_idle_hook() {
    let api = ScriptedApiClient::new(vec!["answer one", "answer two"]);
    let (dir, fs, rt, handler, firer) = make_handler_with_idle_firer(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();

    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();

    // Turn-set 1 completes → exactly one idle fire so far.
    await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
    let fires = await_fires(&firer, 1).await;
    assert_eq!(fires.len(), 1, "one idle fire after turn-set 1: {fires:?}");
    assert_eq!(fires[0].teammate_name, "buddy", "carries the teammate name");
    assert_eq!(
        fires[0].team_name, "alpha",
        "team_name is threaded from the spawn input (claude-code getTeamName())"
    );

    // Inject a message → drives turn-set 2, which parks again → a 2nd fire.
    handler
        .send_message(&h.task_id, "follow-up".into(), c.clone())
        .await
        .unwrap();
    await_spool(&fs, &spool_str, |b| b.contains("answer two")).await;
    let fires = await_fires(&firer, 2).await;
    assert_eq!(fires.len(), 2, "a fire per completed turn-set: {fires:?}");

    handler.kill(&h.task_id, c).await.unwrap();
}

/// With NO firer wired (the default), a completed turn-set is a strict no-op
/// on the hook path: the teammate still runs and parks normally (the spool
/// shows the turn-set), proving the fire is purely additive and absent.
#[tokio::test]
async fn no_idle_firer_is_a_noop() {
    let api = ScriptedApiClient::new(vec!["answer one"]);
    // make_handler builds the handler WITHOUT a teammate idle firer.
    let (dir, fs, rt, handler) = make_handler(api);
    let c = ctx(fs.clone(), rt.clone());

    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();

    // The turn-set completes and parks exactly as before — no firer, no
    // panic, no behavioral change.
    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();
    let body = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
    assert!(
        body.contains("completed:"),
        "turn-set still completes: {body:?}"
    );

    handler.kill(&h.task_id, c).await.unwrap();
}

/// `TeammateIdleFire`'s payload maps 1:1 to `HookEvent::TeammateIdle`'s
/// wire fields — a guard that the seam's struct stays aligned with the event.
#[test]
fn idle_fire_payload_shape() {
    let fire = hooks::TeammateIdleFire {
        teammate_name: "buddy".into(),
        team_name: "alpha".into(),
    };
    assert_eq!(fire.teammate_name, "buddy");
    assert_eq!(fire.team_name, "alpha");
}

// ---- Swarm auto-claim (oracle 2.1.223 zvb / Vvb / rIp) -------------------

/// Serializes the env-mutating auto-claim tests (`LINGXI_CONFIG_DIR`).
static CLAIM_ENV_LOCK: StdMutex<()> = StdMutex::new(());

/// Point the todo store at a throwaway config dir; restore on drop.
struct ClaimEnvGuard {
    prev_config: Option<std::ffi::OsString>,
    prev_list: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl ClaimEnvGuard {
    fn new() -> Self {
        let lock = CLAIM_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lingxi-teammate-claim-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let guard = Self {
            prev_config: std::env::var_os(branding::CONFIG_DIR_ENV),
            prev_list: std::env::var_os("LINGXI_TASK_LIST_ID"),
            dir: dir.clone(),
            _lock: lock,
        };
        std::env::set_var(branding::CONFIG_DIR_ENV, &dir);
        std::env::remove_var("LINGXI_TASK_LIST_ID");
        guard
    }
}
impl Drop for ClaimEnvGuard {
    fn drop(&mut self) {
        match &self.prev_config {
            Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
            None => std::env::remove_var(branding::CONFIG_DIR_ENV),
        }
        match &self.prev_list {
            Some(v) => std::env::set_var("LINGXI_TASK_LIST_ID", v),
            None => std::env::remove_var("LINGXI_TASK_LIST_ID"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn todo(subject: &str, status: engine::TodoState, owner: Option<&str>) -> task_store::TodoTask {
    let mut t =
        task_store::TodoTask::new(subject.into(), "desc".into(), None, serde_json::Map::new());
    t.status = status;
    t.owner = owner.map(str::to_string);
    t
}

#[test]
fn pick_next_task_skips_owned_blocked_and_non_pending() {
    use engine::TodoState::{Completed, InProgress, Pending};
    let mut blocked = todo("blocked", Pending, None);
    blocked.id = "4".into();
    blocked.blocked_by = vec!["2".into()];
    let mut done_blocked = todo("blocked by done", Pending, None);
    done_blocked.id = "5".into();
    done_blocked.blocked_by = vec!["1".into()];
    let tasks = vec![
        {
            let mut t = todo("done", Completed, None);
            t.id = "1".into();
            t
        },
        {
            let mut t = todo("busy", InProgress, None);
            t.id = "2".into();
            t
        },
        {
            let mut t = todo("owned", Pending, Some("other"));
            t.id = "3".into();
            t
        },
        blocked,
        done_blocked,
    ];
    // #1 completed, #2 in_progress, #3 owned, #4 blocked by open #2 —
    // #5's only blocker (#1) is completed, so #5 is the pick.
    assert_eq!(pick_next_task(&tasks).unwrap().id, "5");

    // Empty-string owner is unowned (JS falsy) — flips #3 into the pick.
    let mut tasks2 = tasks;
    tasks2[2].owner = Some(String::new());
    assert_eq!(pick_next_task(&tasks2).unwrap().id, "3");

    assert!(pick_next_task(&[]).is_none());
}

#[test]
fn claimed_task_prompt_is_byte_exact() {
    // Oracle Vvb (2.1.223 @251672219) segment table: `": \n\n "` — a SPACE
    // after the colon at end-of-line and a space before the subject.
    let mut t = todo("Fix the parser", engine::TodoState::Pending, None);
    t.id = "7".into();
    t.description = String::new();
    assert_eq!(
        claimed_task_prompt(&t),
        "Complete all open tasks. Start with task #7: \n\n Fix the parser"
    );
    t.description = "Details here".into();
    assert_eq!(
        claimed_task_prompt(&t),
        "Complete all open tasks. Start with task #7: \n\n Fix the parser\n\nDetails here"
    );
}

#[test]
fn teammate_envelope_wraps_task_list_sender() {
    assert_eq!(
        teammate_message_envelope("task-list", "do it"),
        "<teammate-message teammate_id=\"task-list\">\ndo it\n</teammate-message>"
    );
}

#[test]
fn resolve_list_id_env_overrides_then_team_then_none() {
    let _guard = ClaimEnvGuard::new();
    std::env::set_var("LINGXI_TASK_LIST_ID", "forced-list");
    assert_eq!(
        resolve_teammate_list_id("alpha").as_deref(),
        Some("forced-list")
    );
    std::env::remove_var("LINGXI_TASK_LIST_ID");
    assert_eq!(resolve_teammate_list_id("alpha").as_deref(), Some("alpha"));
    // Teamless spawn = the oracle's `standalone` analogue: no auto-claim.
    assert_eq!(resolve_teammate_list_id(""), None);
}

/// Startup auto-claim (oracle `if(!standalone) await rIp(...)`): spawning a
/// teammate claims the next available task as a side effect — owner set,
/// status in_progress — while the FIRST message stays the description (the
/// returned prompt is discarded at startup).
#[tokio::test]
async fn spawn_auto_claims_next_available_task() {
    let _guard = ClaimEnvGuard::new();
    let team = "claim-team-startup";
    let store = task_store::TodoStore::for_list(team);
    let tid = store
        .create(todo("Startup work", engine::TodoState::Pending, None))
        .await
        .unwrap();

    let api = ScriptedApiClient::new(vec!["answer one"]);
    let (_d, fs, rt, handler) = make_handler(api);
    let c = ctx(fs, rt);
    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: team.into(),
                description: "seeded description".into(),
            },
            c.clone(),
        )
        .await
        .unwrap();

    // The claim is synchronous inside spawn (before the worker starts).
    let t = store.get(&tid).await.unwrap();
    assert_eq!(t.owner.as_deref(), Some("buddy"), "claimed at startup");
    assert_eq!(t.status, engine::TodoState::InProgress);

    handler.kill(&h.task_id, c).await.unwrap();
}

/// Idle auto-claim (oracle poll loop @251675509): a task created AFTER the
/// teammate parks is claimed by the 500ms idle tick and its Vvb prompt is
/// self-injected as the next user message, driving turn-set 2.
#[tokio::test]
async fn idle_poll_claims_late_task_and_drives_next_turn_set() {
    let _guard = ClaimEnvGuard::new();
    let team = "claim-team-idle";
    let store = task_store::TodoStore::for_list(team);

    let api = ScriptedApiClient::new(vec!["answer one", "answer two"]);
    let api_handle = api.clone();
    let (dir, fs, rt, handler) = make_handler(api);
    let c = ctx(fs.clone(), rt);
    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: team.into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();
    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();

    // Turn-set 1 completes and the teammate parks (idle window opens).
    let body = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
    assert!(body.contains("completed:"), "turn-set 1 parked: {body:?}");

    // NOW a task appears on the shared list.
    let tid = store
        .create(todo("Late work", engine::TodoState::Pending, None))
        .await
        .unwrap();

    // Within a few ticks the idle poller claims it and injects the prompt,
    // waking the runner into turn-set 2.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let t = store.get(&tid).await.unwrap();
        if t.owner.as_deref() == Some("buddy")
            && t.status == engine::TodoState::InProgress
            && api_handle.call_count() >= 2
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "idle poller never claimed the late task: owner={:?} status={:?} calls={}",
            t.owner,
            t.status,
            api_handle.call_count()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let body = await_spool(&fs, &spool_str, |b| b.contains("answer two")).await;
    assert!(body.contains("answer two"), "turn-set 2 ran: {body:?}");

    handler.kill(&h.task_id, c).await.unwrap();
}

/// After kill the idle poller stops: a task seeded post-kill stays unclaimed.
#[tokio::test]
async fn killed_teammate_stops_claiming() {
    let _guard = ClaimEnvGuard::new();
    let team = "claim-team-killed";
    let store = task_store::TodoStore::for_list(team);

    let api = ScriptedApiClient::new(vec!["answer one"]);
    let (dir, fs, rt, handler) = make_handler(api);
    let c = ctx(fs.clone(), rt);
    let h = handler
        .spawn(
            TaskSpawnInput::InProcessTeammate {
                agent_id: protocol::AgentId::new(),
                name: "buddy".into(),
                team_name: team.into(),
                description: String::new(),
            },
            c.clone(),
        )
        .await
        .unwrap();
    let spool = dir.path().join(format!("{}.output", h.task_id));
    let spool_str = spool.to_str().unwrap().to_string();
    let _ = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;

    handler.kill(&h.task_id, c).await.unwrap();

    let tid = store
        .create(todo("Post-kill work", engine::TodoState::Pending, None))
        .await
        .unwrap();
    // Two full tick intervals: a live poller would have claimed by now.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let t = store.get(&tid).await.unwrap();
    assert_eq!(t.owner, None, "killed teammate must not claim");
    assert_eq!(t.status, engine::TodoState::Pending);
}
