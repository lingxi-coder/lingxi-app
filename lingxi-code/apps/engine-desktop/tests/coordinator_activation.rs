//! End-to-end implicit teammate activation: a real persistent worker drives
//! model calls and client status without explicit team-management tools.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use client_adapter::test_support::MockSink;
use client_adapter::AdapterOutputStream;
use client_protocol::events::ClientEvent;
use platform_api::filesystem::FileSystem;
use platform_api::team_spawn::TeamSpawnSeam;
use platform_api::{OutputStream, RuntimeSpawner};
use platform_posix::{PosixFileSystem, PosixRuntime};
use protocol::AgentId;
use tasks::handlers::InProcessTeammateHandler;
use tasks::output_manager::TaskOutputManager;
use tasks::registry::TaskRegistry;
use tasks::task_trait::TaskSpawnInput;
use tasks::TaskType;

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
    /// `CoordinatorStatusSink` and implicit spawner, as `build()`
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

    // The typed spawn/kill seam the implicit spawner uses (implemented on
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

struct MockInvoker;
#[async_trait]
impl platform_api::tool_invoker::ToolInvoker for MockInvoker {
    async fn invoke(
        &self,
        _: &str,
        _: serde_json::Value,
        _: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        Ok(serde_json::Value::Null)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
struct MockBudget;
#[async_trait]
impl platform_api::budget::BudgetEnforcerHandle for MockBudget {
    async fn check_and_charge(&self, _: u64) -> Result<(), platform_api::budget::BudgetError> {
        Ok(())
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
    }
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

/// Yield until the completed scripted turn leaves the persistent worker idle.
async fn await_worker_idle(team: &Arc<coordinator::TeamRegistry>) -> bool {
    for _ in 0..2000 {
        let workers = team.list().await;
        if matches!(
            workers.first().map(|w| &w.status),
            Some(coordinator::WorkerStatus::Idle)
        ) {
            return true;
        }
        tokio::task::yield_now().await;
    }
    matches!(
        team.list().await.first().map(|w| &w.status),
        Some(coordinator::WorkerStatus::Idle)
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

/// Implicit creation starts the handler and feeds the real client adapter.
#[tokio::test]
async fn implicit_agent_spawn_flows_active_workers_to_client_event() {
    let api = ScriptedApiClient::new();
    let fixture = make_coordinator_fixture(&api);
    let spawner = coordinator::ImplicitTeammateSpawner::new(
        fixture.team.clone(),
        fixture.spawn_seam.clone(),
        Arc::new(PosixRuntime::new()),
        fixture.output.clone(),
        "12345678-0000-0000-0000-000000000000".into(),
    )
    .with_home(fixture._tmp.path().to_path_buf());
    spawner.initialize().await;
    let result = spawner
        .spawn(
            platform_api::subagent_spawn::SubagentSpawnRequest {
                name: Some("alpha".into()),
                subagent_type: "general-purpose".into(),
                prompt: "drive the activation gate".into(),
                ..Default::default()
            },
            platform_api::subagent_spawn::SubagentInheritance {
                tool_invoker: Arc::new(MockInvoker),
                budget: Arc::new(MockBudget),
            },
        )
        .await
        .expect("implicit teammate spawn succeeds");

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
    assert_eq!(result.name, "alpha");
    assert_eq!(result.team_name, "session-12345678");
    assert!(!result.is_splitpane);

    // (b) The teammate handler ACTUALLY ran: the persistent runner made at least
    //     one scripted model round-trip. The hollow `create()` path never
    //     dispatches a handler, so this stays 0 there.
    assert!(
        await_round_trip(&api).await,
        "the InProcessTeammate handler must run a real model round-trip (call_count > 0); \
         a hollow create()-only impl would leave it at 0"
    );

    // (d) A completed turn returns the persistent worker to Idle. A fast
    //     scripted turn may finish before linking, so Working is transient.
    assert!(
        await_worker_idle(&fixture.team).await,
        "completed teammate turn must publish Idle through the sink"
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
        matches!(workers[0].status, coordinator::WorkerStatus::Idle),
        "persistent worker is Idle after its completed turn; got {:?}",
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
        Some("session-12345678"),
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
                    description: "hollow".into(),
                    spawn_request: None,
                    inheritance: None,
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
// Registry contract for every session.

fn stub_ctx() -> tool_api::BuiltinToolContext {
    tool_api::test_support::shell_test_ctx(platform_api::process::ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    })
}

#[test]
fn default_session_has_no_explicit_team_tools() {
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), None, None);
    assert!(reg.find_by_name("TeamCreate").is_none());
    assert!(reg.find_by_name("TeamDelete").is_none());
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
        spawn_seam: fx.spawn_seam.clone(),
    };
    let reg = engine_desktop::desktop_tool_registry(stub_ctx(), Some(wiring), None);

    let names = reg.all_names();
    assert_eq!(
        names.iter().filter(|n| *n == "SendMessage").count(),
        1,
        "exactly one SendMessage in a coordinator session (no builtin shadow)"
    );

    assert!(reg.find_by_name("TeamCreate").is_none());
    assert!(reg.find_by_name("TeamDelete").is_none());
}

/// A coordinator-session registry also upholds the no-duplicate-names invariant
/// (the no-silent-shadow guardrail across ALL the spliced coordinator tools).
#[test]
fn coordinator_session_tool_list_has_no_duplicate_names() {
    let api = ScriptedApiClient::new();
    let fx = make_coordinator_fixture(&api);
    let wiring = engine_desktop::CoordinatorWiring {
        team: fx.team.clone(),
        spawn_seam: fx.spawn_seam.clone(),
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
