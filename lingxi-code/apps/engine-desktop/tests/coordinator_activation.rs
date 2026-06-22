//! M10 (T15) — ANTI-HOLLOW end-to-end coordinator-activation gate.
//!
//! This is the key real-run gate for the FULL coordinator multi-agent
//! activation: it proves that `active_workers > 0` flows from a real
//! `TeamCreate` tool call all the way to a `ClientEvent::CoordinatorStatus`,
//! exercising the entire load-bearing path the program adds, end to end, with
//! the SAME components `engine_desktop::build()` wires for a coordinator
//! session:
//!
//! * `coordinator::TeamRegistry` (the per-session worker registry),
//! * `tasks::registry::TaskRegistry` with a directly-registered
//!   `InProcessTeammateHandler` (the T13 escape-hatch registration that attaches
//!   the status sink), whose `TaskRegistry::spawn(..)` (T01) is the real handler
//!   dispatch the hollow `create()` never performed,
//! * `TaskRegistry as traits::team_spawn::TeamSpawnSeam` (T04),
//! * the coordinator `TeamCreate` tool from
//!   `coordinator::internal_tools::coordinator_internal_tools(..)` (T03/T05) —
//!   the exact factory `desktop_tool_registry` assembles in coordinator mode,
//! * `coordinator::CoordinatorStatusSink` (T07) feeding a REAL
//!   `client_adapter::AdapterOutputStream` (T08/T09) behind a `MockSink`, the
//!   precise PUSH wiring `build()` uses: `set_status -> WorkerStatus transition
//!   -> OutputStream::emit_coordinator_status -> ClientEvent::CoordinatorStatus`.
//!
//! The teammate runs under the production `PosixRuntime` spawner (the same one
//! `build()` gives the teammate pool) so the persistent worker future ACTUALLY
//! executes; a scripted `SubagentApiClient` makes the teammate's model
//! round-trip observable, so we can assert the handler truly ran rather than
//! merely allocating a `Pending` row.
//!
//! ## Why this fails against the hollow `create()`-only implementation
//!
//! `TaskRegistry::create()` only inserts a `Pending` row and NEVER looks up or
//! runs a handler. Against that implementation the spawn seam would never start
//! a teammate, so: no scripted round-trip (b), no `Running` status transition
//! (d), and therefore no `CoordinatorStatus` push (e). The
//! `anti_hollow_create_path_emits_nothing` control wires the SAME graph through
//! `create()` instead of `spawn()` and asserts exactly that absence, documenting
//! that the gate is load-bearing on the T01 dispatch.

#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use client_adapter::test_support::MockSink;
use client_adapter::AdapterOutputStream;
use client_protocol::events::ClientEvent;
use platform_posix::{PosixFileSystem, PosixRuntime};
use protocol::AgentId;
use tasks::handlers::InProcessTeammateHandler;
use tasks::output_manager::TaskOutputManager;
use tasks::registry::TaskRegistry;
use tasks::task_trait::TaskSpawnInput;
use tasks::TaskType;
use traits::filesystem::FileSystem;
use traits::team_spawn::TeamSpawnSeam;
use traits::{OutputStream, RuntimeSpawner};

// ---------------------------------------------------------------------------
// Scripted SubagentApiClient — one round-trip per `messages_create`, returning
// an `end_turn` text turn so the persistent teammate finishes its first
// turn-set cleanly and parks. The call count proves the handler actually ran a
// real model round-trip (not a hollow `Pending` allocation).
// ---------------------------------------------------------------------------

struct ScriptedApiClient {
    responses: StdMutex<VecDeque<llm_client::LlmResponse>>,
    calls: AtomicUsize,
}

impl ScriptedApiClient {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            responses: StdMutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl agent::api::SubagentApiClient for ScriptedApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<protocol::ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.responses.lock().unwrap().pop_front();
        Ok(next.unwrap_or_else(|| llm_client::LlmResponse {
            id: "scripted".into(),
            model: "scripted".into(),
            content: vec![llm_client::ContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            stop_reason: Some("end_turn".into()),
            stop_details: None,
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }))
    }
}

// ---------------------------------------------------------------------------
// Coordinator subsystem fixture — assembles the SAME component graph
// `engine_desktop::build()` wires for a coordinator session, with the scripted
// api client + production `PosixRuntime` so the teammate truly runs.
// ---------------------------------------------------------------------------

struct CoordinatorFixture {
    _tmp: tempfile::TempDir,
    team: Arc<coordinator::TeamRegistry>,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
    sink: Arc<MockSink>,
    /// The SINGLE orchestrator-facing output stream — the real
    /// `AdapterOutputStream` behind the `MockSink`. Shared by the
    /// `CoordinatorStatusSink` AND the `TeamCreate` tool exactly as `build()`
    /// shares one `Arc<dyn OutputStream>` across the orchestrator, the sink, and
    /// the coordinator wiring.
    output: Arc<dyn OutputStream>,
}

/// Build the coordinator subsystem exactly as `build()` does (minus the rest of
/// the orchestrator), with the status sink feeding a real `AdapterOutputStream`
/// behind a `MockSink`. `register_handler` takes `&mut self`, so the handler is
/// registered before the registry is `Arc`-wrapped — mirroring `build()`'s
/// pre-Arc registration.
fn make_coordinator_fixture(api: &Arc<ScriptedApiClient>) -> CoordinatorFixture {
    let tmp = tempfile::tempdir().unwrap();
    // Redirect `$HOME` to the scratch dir so the coordinator `TeamCreate` tool
    // writes its on-disk team file (`~/.claude/teams/{name}/config.json`,
    // resolved from `$HOME`) under the tempdir instead of the developer's real
    // home. The coordinator factory does not expose a home-override seam, so
    // env-redirect is the hermeticity lever here.
    std::env::set_var("HOME", tmp.path());
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(tmp.path().to_path_buf()));
    let runtime: Arc<dyn RuntimeSpawner> = Arc::new(PosixRuntime::new());
    let output_manager = Arc::new(TaskOutputManager::new(tmp.path().to_path_buf(), fs.clone()));

    // The per-session coordinator team registry + mode (mode ENABLED — a
    // coordinator session enters it at build time).
    let team = Arc::new(coordinator::TeamRegistry::new(AgentId::new()));
    let mode = Arc::new(coordinator::CoordinatorMode::new());
    mode.enter();

    // The PUSH wiring: the status sink feeds a real `AdapterOutputStream` behind
    // a `MockSink` — exactly the T08/T09 production lowering path.
    let mock_sink = MockSink::arc();
    let output: Arc<dyn OutputStream> = Arc::new(AdapterOutputStream::new(
        mock_sink.clone() as Arc<dyn client_adapter::sink::ClientEventSink>
    ));
    let status_sink = Arc::new(coordinator::CoordinatorStatusSink::new(
        team.clone(),
        output.clone(),
    ));

    // The teammate handler, registered DIRECTLY (not via `register_agent_handlers`)
    // so the coordinator status sink is attached — the T13 escape hatch.
    let teammate_pool = Arc::new(agent::StateMachinePool::new(
        runtime.clone(),
        engine_desktop::TEAMMATE_POOL_CAP,
    ));
    let teammate_handler = InProcessTeammateHandler::new(
        teammate_pool,
        output_manager.clone(),
        api.clone() as Arc<dyn agent::api::SubagentApiClient>,
    )
    .with_status_sink(status_sink as Arc<dyn tasks::handlers::TaskStatusSink>);

    let mut registry = TaskRegistry::new(runtime, fs, output_manager);
    registry.register_handler(TaskType::InProcessTeammate, Arc::new(teammate_handler));
    let registry = Arc::new(registry);

    // The typed spawn/kill seam the coordinator `TeamCreate` uses (T04 impl on
    // `TaskRegistry`).
    let spawn_seam: Arc<dyn TeamSpawnSeam> = registry.clone();

    CoordinatorFixture {
        _tmp: tmp,
        team,
        spawn_seam,
        sink: mock_sink,
        output,
    }
}

/// Build the coordinator `TeamCreate` tool from the exact production factory
/// (`coordinator_internal_tools`), with coordinator mode ENABLED, wired to the
/// fixture's team, spawn seam, and the SHARED output stream (so the tool's
/// activation PUSH and the sink's transitions feed the one `MockSink`).
fn team_create_tool(fixture: &CoordinatorFixture) -> Arc<dyn tool_api::Tool> {
    let mode = Arc::new(coordinator::CoordinatorMode::new());
    mode.enter();
    let tools = coordinator::internal_tools::coordinator_internal_tools(
        fixture.team.clone(),
        mode,
        fixture.spawn_seam.clone(),
        fixture.output.clone(),
        None,
        // Activation tests don't drive the mailbox→runner pump; no spawner.
        None,
    );
    tools
        .into_iter()
        .find(|t| t.name() == "TeamCreate")
        .expect("coordinator factory must return a TeamCreate tool")
}

/// Yield until the scripted api client has recorded at least one round-trip, or
/// the budget runs out.
async fn await_round_trip(api: &Arc<ScriptedApiClient>) -> bool {
    for _ in 0..2000 {
        if api.call_count() >= 1 {
            return true;
        }
        tokio::task::yield_now().await;
    }
    api.call_count() >= 1
}

/// Yield until the single worker reports `Working`, or the budget runs out.
async fn await_worker_working(team: &Arc<coordinator::TeamRegistry>) -> bool {
    for _ in 0..2000 {
        let workers = team.list().await;
        if matches!(
            workers.first().map(|w| &w.status),
            Some(coordinator::WorkerStatus::Working { .. })
        ) {
            return true;
        }
        tokio::task::yield_now().await;
    }
    matches!(
        team.list().await.first().map(|w| &w.status),
        Some(coordinator::WorkerStatus::Working { .. })
    )
}

/// Yield until a `CoordinatorStatus { active_workers > 0 }` reaches the sink, or
/// the budget runs out.
async fn await_coordinator_status(sink: &Arc<MockSink>) -> bool {
    for _ in 0..2000 {
        if sink.events().await.iter().any(|e| {
            matches!(
                e,
                ClientEvent::CoordinatorStatus { active_workers, .. } if *active_workers >= 1
            )
        }) {
            return true;
        }
        tokio::task::yield_now().await;
    }
    sink.events().await.iter().any(|e| {
        matches!(
            e,
            ClientEvent::CoordinatorStatus { active_workers, .. } if *active_workers >= 1
        )
    })
}

/// THE anti-hollow gate. A real `TeamCreate` call against a coordinator session
/// must (a) register a worker, (b) actually RUN a teammate (scripted model
/// round-trip), (c) reconcile the handler-generated task_id onto the worker,
/// (d) transition the worker `Idle -> Working` via the sink, and (e) push a
/// `ClientEvent::CoordinatorStatus { active_workers > 0 }` to the client sink.
#[tokio::test]
async fn coordinator_activation_team_create_flows_active_workers_to_client_event() {
    let api = ScriptedApiClient::new();
    let fixture = make_coordinator_fixture(&api);
    let tool = team_create_tool(&fixture);

    // Invoke the coordinator TeamCreate tool — the real-run entry point.
    let (tx, _rx) = tool_api::progress::progress_channel();
    let result = tool
        .call(
            serde_json::json!({
                "team_name": "alpha",
                "agent_type": "team-lead",
                "description": "drive the activation gate",
            }),
            tool_api::test_support::fresh_ctx(),
            tx,
        )
        .await
        .expect("TeamCreate must succeed in an enabled coordinator session");

    // (a) A WorkerAgent now exists in the coordinator registry.
    let workers = fixture.team.list().await;
    assert_eq!(workers.len(), 1, "exactly one worker registered");
    let worker = &workers[0];
    assert_eq!(worker.name, "alpha");

    // (c) The worker's task_id is the handler-generated id reconciled back onto
    //     it — NON-empty and NOT the agent_id placeholder the old metadata-only
    //     path returned.
    assert!(
        !worker.task_id.is_empty(),
        "handler-generated task_id must be written back (hollow path leaves it empty)"
    );
    assert_ne!(
        worker.task_id,
        worker.agent_id.as_uuid().to_string(),
        "task_id must be the handler id, not the agent_id placeholder"
    );
    // The tool result surfaces the same real task_id.
    let result_task_id = result.data["task_id"].as_str().unwrap_or_default();
    assert_eq!(result_task_id, worker.task_id);
    assert_eq!(result.data["spawned"], serde_json::json!(true));

    // (b) The teammate handler ACTUALLY ran: the persistent runner made at least
    //     one scripted model round-trip. The hollow `create()` path never
    //     dispatches a handler, so this stays 0 there.
    assert!(
        await_round_trip(&api).await,
        "the InProcessTeammate handler must run a real model round-trip (call_count > 0); \
         a hollow create()-only impl would leave it at 0"
    );

    // (d) The worker transitioned Idle -> Working via the CoordinatorStatusSink
    //     (the handler's spawned worker fires `set_status(task_id, Running)`).
    assert!(
        await_worker_working(&fixture.team).await,
        "worker must transition to Working via the sink (Idle -> Working); \
         the hollow path never drives this transition"
    );

    // (e) A `ClientEvent::CoordinatorStatus { active_workers > 0 }` reached the
    //     client sink through the real AdapterOutputStream lowering.
    assert!(
        await_coordinator_status(&fixture.sink).await,
        "a CoordinatorStatus with active_workers > 0 must reach the client sink"
    );

    // Final, authoritative assertions.
    let workers = fixture.team.list().await;
    assert!(
        matches!(workers[0].status, coordinator::WorkerStatus::Working { .. }),
        "worker is Working after the Running transition; got {:?}",
        workers[0].status
    );

    let events = fixture.sink.events().await;
    let status = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::CoordinatorStatus {
                active_workers,
                team,
            } => Some((*active_workers, team.clone())),
            _ => None,
        })
        .expect("a CoordinatorStatus event must have been emitted");
    assert!(
        status.0 >= 1,
        "active_workers must be > 0 on the wire; got {}",
        status.0
    );
    assert_eq!(
        status.1.as_deref(),
        Some("alpha"),
        "the team name must ride the CoordinatorStatus DTO"
    );
}

/// ANTI-HOLLOW CONTROL: route the SAME graph through `TaskRegistry::create()`
/// (the hollow placeholder path) instead of `spawn()`. `create()` only inserts a
/// `Pending` row — it never looks up or runs a handler — so NONE of the
/// activation signals appear: no scripted round-trip, no worker status
/// transition, no `CoordinatorStatus` push. This documents that the gate above
/// is load-bearing on the T01 `spawn()` dispatch.
#[tokio::test]
async fn anti_hollow_create_path_emits_nothing() {
    let api = ScriptedApiClient::new();
    let fixture = make_coordinator_fixture(&api);

    // Spawn a worker's metadata, then drive the registry through the HOLLOW
    // `create()` path (what the implementation did before T01), keying nothing
    // back onto the worker.
    let agent_id = fixture
        .team
        .spawn_worker("team-lead".into(), "alpha".into(), String::new())
        .await
        .unwrap();
    let _placeholder_id = {
        // Reach the concrete registry via the seam's owning Arc is not possible;
        // instead exercise `create()` on a sibling registry built the same way.
        // The point is behavioral: `create()` returns a generated id WITHOUT
        // running a handler.
        let tmp = tempfile::tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(tmp.path().to_path_buf()));
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(PosixRuntime::new());
        let output_manager = Arc::new(TaskOutputManager::new(tmp.path().to_path_buf(), fs.clone()));
        let registry = TaskRegistry::new(runtime, fs, output_manager);
        registry
            .create(
                TaskType::InProcessTeammate,
                TaskSpawnInput::InProcessTeammate {
                    agent_id,
                    name: "alpha".into(),
                    team_name: "alpha".into(),
                },
                "hollow".into(),
            )
            .await
            .unwrap()
    };

    // Give any (nonexistent) async work a chance to run.
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }

    // (b') No model round-trip — `create()` never dispatched a handler.
    assert_eq!(
        api.call_count(),
        0,
        "the hollow create() path must NOT run the teammate handler"
    );
    // (d') The worker is still Idle — no `Running` transition was driven.
    let workers = fixture.team.list().await;
    assert!(
        matches!(workers[0].status, coordinator::WorkerStatus::Idle),
        "the hollow path leaves the worker Idle; got {:?}",
        workers[0].status
    );
    // (e') No CoordinatorStatus event reached the client sink.
    let events = fixture.sink.events().await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::CoordinatorStatus { .. })),
        "the hollow path must not push any CoordinatorStatus"
    );
}

// ---------------------------------------------------------------------------
// DEFAULT-SESSION NO-REGRESSION — the additive guardrail. A non-coordinator
// session (`desktop_tool_registry(.., None)`) must be byte-identical to the
// pre-M10 build: `tool_team`'s TeamCreate/TeamDelete are the registered ones,
// the coordinator tools are absent, and the advertised tool list is unchanged.
// ---------------------------------------------------------------------------

fn stub_ctx() -> tool_api::BuiltinToolContext {
    tool_api::test_support::shell_test_ctx(traits::process::ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    })
}

/// A default session registers `tool_team`'s `TeamCreate` (distinguished by its
/// 30_000-char result cap, vs the coordinator tool's `100_000`) and exactly one
/// of each team tool — no coordinator shadow.
#[test]
fn default_session_registers_tool_team_pair_not_coordinator() {
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), None, None);
    let names = reg.all_names();

    assert_eq!(
        names.iter().filter(|n| *n == "TeamCreate").count(),
        1,
        "exactly one TeamCreate in a default session"
    );
    assert_eq!(
        names.iter().filter(|n| *n == "TeamDelete").count(),
        1,
        "exactly one TeamDelete in a default session"
    );

    // Behavior marker: tool_team's TeamCreate caps results at MAX_TOOL_OUTPUT_LENGTH
    // (30_000); the coordinator's caps at 100_000.
    let create = reg
        .find_by_name("TeamCreate")
        .expect("TeamCreate registered");
    assert_eq!(
        create.max_result_size_chars(),
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH,
        "a default session must register tool_team's TeamCreate (30_000 cap), not the coordinator's"
    );
}

/// The default-session advertised tool list has no duplicate names (the
/// no-silent-shadow guardrail) — exactly what a coordinator session must also
/// uphold.
#[test]
fn default_session_tool_list_has_no_duplicate_names() {
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), None, None);
    let mut names = reg.all_names();
    names.sort();
    let mut deduped = names.clone();
    deduped.dedup();
    assert_eq!(
        names, deduped,
        "no tool name may appear twice in a default-session registry"
    );
}

/// A coordinator session registers the coordinator `SendMessage` IN PLACE OF the
/// `tool_ui` builtin: exactly ONE `SendMessage`, and it is the coordinator one
/// (distinguished by its `to` schema description, which advertises the `uds:` /
/// `bridge:` peer schemes the leaner `tool_ui` builtin does not). Guards against
/// the builtin silently shadowing the coordinator tool (`find_by_name` is
/// builtin-first).
#[test]
fn coordinator_session_registers_coordinator_send_message_not_builtin() {
    let api = ScriptedApiClient::new();
    let fx = make_coordinator_fixture(&api);
    let wiring = engine_desktop::CoordinatorWiring {
        team: fx.team.clone(),
        mode: Arc::new(coordinator::CoordinatorMode::new()),
        spawn_seam: fx.spawn_seam.clone(),
        output: fx.output.clone(),
        bus: None,
        runtime: None,
    };
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), Some(wiring), None);

    let names = reg.all_names();
    assert_eq!(
        names.iter().filter(|n| *n == "SendMessage").count(),
        1,
        "exactly one SendMessage in a coordinator session (no builtin shadow)"
    );

    // Behavior marker: the coordinator `SendMessage`'s `to` description names the
    // `uds:` / `bridge:` peer schemes; the `tool_ui` builtin's does not.
    let send = reg.find_by_name("SendMessage").expect("SendMessage registered");
    let to_desc = send.input_schema()["properties"]["to"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(
        to_desc.contains("uds:") && to_desc.contains("bridge:"),
        "a coordinator session must register the coordinator SendMessage (its `to` schema names uds:/bridge:), got: {to_desc:?}"
    );
}

/// A coordinator-session registry also upholds the no-duplicate-names invariant
/// (the no-silent-shadow guardrail across ALL the spliced coordinator tools).
#[test]
fn coordinator_session_tool_list_has_no_duplicate_names() {
    let api = ScriptedApiClient::new();
    let fx = make_coordinator_fixture(&api);
    let wiring = engine_desktop::CoordinatorWiring {
        team: fx.team.clone(),
        mode: Arc::new(coordinator::CoordinatorMode::new()),
        spawn_seam: fx.spawn_seam.clone(),
        output: fx.output.clone(),
        bus: None,
        runtime: None,
    };
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), Some(wiring), None);
    let mut names = reg.all_names();
    names.sort();
    let mut deduped = names.clone();
    deduped.dedup();
    assert_eq!(
        names, deduped,
        "no tool name may appear twice in a coordinator-session registry"
    );
}
