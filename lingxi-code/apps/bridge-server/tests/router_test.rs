//! F2-08 — Full command/event routing in `bridge-server`.
//!
//! The walking skeleton (F2-05/F2-06) routed only the turn + permission path.
//! This file proves the FULL `ClientCommand` surface reaches its engine entry
//! and that each pull produces the correct framed reply — exercised at the
//! routing seam ([`bridge_server::router::CommandRouter`]) the connection loop
//! delegates non-turn/non-permission commands to.
//!
//! The seam mirrors the proven [`bridge_server::server::TurnDriver`] shape: the
//! production server binds an [`bridge_server::router::EngineCommandRouter`]
//! wrapping the real engine handles (`OrchestratorHandle`, `AuthHandle`,
//! `TaskRegistryHandle`); these tests bind the SAME `EngineCommandRouter` over
//! the engine's own `MockOrchestratorHandle` / mock task + auth handles, so the
//! routing-and-lowering path under test is the production one (no test-only
//! router shim) — only the engine handles are doubles.
//!
//! Tests (plan §2):
//! - `set_model_routes` — `SetModel` → `switch_model` → `ModelChanged`.
//! - `list_models_routes` — `ListModels` → `list_available_models` → `ModelList`.
//! - `list_mcp_routes` — `RefreshListings{Mcp}` → `list_mcp_servers` → `McpServers`.
//! - `slash_command_routes_to_registry` — `RunSlashCommand` reaches the dispatcher.
//! - `task_list_poll_emits_task_row` — the adapter's task poll loop emits a
//!   `TaskRow` per task from `TaskRegistryHandle::list`.
//! - `task_list_command_emits_task_rows` — `TaskList` → `list` → one `TaskRow` each.
//! - `clear_session_rejected_mid_turn` — `ClearSession` is refused with an
//!   `Error` while a turn is in flight, and routed once the turn ends.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::McpEndpoint;
use bridge_server::router::{CommandRouter, EngineCommandRouter};
use bridge_server::server::BridgeConnection;
use client_adapter::{AdapterPermissionGate, ClientEventSink, PermissionRequestSink};
use client_protocol::commands::{ClientCommand, ListingKindDto};
use client_protocol::events::ClientEvent;
use client_protocol::permission::PermissionRequest;
use futures_util::{SinkExt, StreamExt};
use orchestrator::test_support::MockOrchestratorHandle;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;
use traits::auth::{AuthError, AuthHandle, LoginInfo};
use traits::orchestrator::{McpServerInfo, McpStatus, StatusSnapshot};
use traits::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};

// ── Test sink ───────────────────────────────────────────────────────────────

/// A [`ClientEventSink`] that captures every emitted event in emission order so
/// a routing test can assert the reply set.
#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<ClientEvent>>,
}

impl CapturingSink {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    async fn events(&self) -> Vec<ClientEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl ClientEventSink for CapturingSink {
    async fn emit(&self, event: ClientEvent) {
        self.events.lock().await.push(event);
    }
}

// ── Mock auth + task handles ─────────────────────────────────────────────────

/// Auth double — `current_user` returns a fixed signed-in user.
struct MockAuth;

#[async_trait]
impl AuthHandle for MockAuth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        Ok(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_1".into(),
        })
    }
    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }
    async fn current_user(&self) -> Option<LoginInfo> {
        Some(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_1".into(),
        })
    }
}

/// Task-registry double — `list` returns the pre-loaded rows; other CRUD is a
/// no-op success default.
struct MockTaskRegistry {
    rows: Vec<TaskRecord>,
}

#[async_trait]
impl TaskRegistryHandle for MockTaskRegistry {
    async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }
    async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        Ok(self.rows.clone())
    }
    async fn update(
        &self,
        _id: &str,
        _patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn set_status(&self, _id: &str, _status: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
        Ok(TaskRecord {
            task_id: id.to_string(),
            task_type: "local_bash".into(),
            status: "killed".into(),
            description: "stopped".into(),
            command: None,
        })
    }
    async fn output(
        &self,
        id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        Ok(TaskOutputChunk {
            task_id: id.to_string(),
            content: "line1\nline2".into(),
            total_lines: 2,
            truncated: false,
            ..Default::default()
        })
    }
}

// ── Router fixtures ───────────────────────────────────────────────────────────

fn router_with(
    handle: Arc<MockOrchestratorHandle>,
    tasks: Arc<MockTaskRegistry>,
) -> EngineCommandRouter {
    EngineCommandRouter::new(
        handle as Arc<dyn traits::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        tasks as Arc<dyn TaskRegistryHandle>,
        None,
    )
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_model_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetModel {
                model: "claude-opus-4-8".into(),
            },
            sink.clone(),
        )
        .await;

    // The command reached the engine handle.
    assert_eq!(handle.switch_model_call_count(), 1);
    assert_eq!(handle.last_switched_model().as_deref(), Some("claude-opus-4-8"));

    // …and the reply is a `ModelChanged` carrying the new model.
    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ClientEvent::ModelChanged {
            model: "claude-opus-4-8".into()
        }
    );
}

#[tokio::test]
async fn list_models_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["a".into(), "b".into()]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "a".into(),
        ..StatusSnapshot::default()
    });
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router.route(ClientCommand::ListModels, sink.clone()).await;

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ClientEvent::ModelList {
            models: vec!["a".into(), "b".into()],
            current: "a".into(),
        }
    );
}

#[tokio::test]
async fn list_mcp_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_mcp_servers(vec![McpServerInfo {
        name: "fs".into(),
        status: McpStatus::Connected,
        transport: "stdio".into(),
    }]);
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Mcp],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        ClientEvent::McpServers { servers } => {
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0].name, "fs");
            assert_eq!(servers[0].transport, "stdio");
        }
        other => panic!("expected McpServers, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_command_routes_to_registry() {
    // A dispatcher seeded with the builtin handlers — the SAME registry the
    // desktop composition root builds; the routed `/clear` reaches it and yields
    // a non-empty display string, proving the command crossed into the registry.
    use command_api::dispatcher::RegistrySlashDispatcher;
    use command_api::registry::CommandRegistry;
    use command_core::register_all_builtin_commands;
    use tokio::sync::RwLock;

    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let dispatcher = Arc::new(RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg))));

    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = EngineCommandRouter::new(
        handle as Arc<dyn traits::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        Some(dispatcher),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RunSlashCommand {
                raw: "/clear".into(),
            },
            sink.clone(),
        )
        .await;

    // The dispatcher handled the command — the router surfaces its display as a
    // TextDelta (the slash reply is LOSSY: display text + optional injected
    // prompt, per CommandResultDto).
    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::TextDelta { text } if !text.is_empty())),
        "the slash command must route to the registry and surface a display, got {events:?}"
    );
}

#[tokio::test]
async fn task_list_command_emits_task_rows() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![
            TaskRecord {
                task_id: "b3f9zk2xq".into(),
                task_type: "local_bash".into(),
                status: "running".into(),
                description: "build".into(),
                command: None,
            },
            TaskRecord {
                task_id: "a1c2d3e4f".into(),
                task_type: "agent".into(),
                status: "completed".into(),
                description: "review".into(),
                command: None,
            },
        ],
    });
    let router = router_with(handle, tasks);
    let sink = CapturingSink::arc();

    router
        .route(ClientCommand::TaskList { status_filter: None }, sink.clone())
        .await;

    let events = sink.events().await;
    let rows: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::TaskRow { task } => Some(task.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 2, "one TaskRow per task, got {events:?}");
    assert_eq!(rows[0].task_id, "b3f9zk2xq");
    assert_eq!(rows[1].task_id, "a1c2d3e4f");
}

#[tokio::test]
async fn task_list_poll_emits_task_row() {
    // The adapter OWNS a task poll loop (matches the TUI; `TaskRegistryHandle::list`
    // on an interval). One tick of the poll loop emits a `TaskRow` per task.
    let handle = Arc::new(MockOrchestratorHandle::new());
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![TaskRecord {
            task_id: "p0lle3dt1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "poll me".into(),
            command: None,
        }],
    });
    let router = router_with(handle, tasks);
    let sink = CapturingSink::arc();

    // Spawn the poll loop on a short interval, then stop it after one tick.
    let poll = router.spawn_task_poll(sink.clone(), Duration::from_millis(10));

    // Wait until at least one TaskRow lands (bounded so a regression can't hang).
    let mut saw_row = false;
    for _ in 0..200 {
        if sink
            .events()
            .await
            .iter()
            .any(|e| matches!(e, ClientEvent::TaskRow { .. }))
        {
            saw_row = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    poll.stop();
    assert!(saw_row, "the task poll loop must emit a TaskRow per live task");

    let events = sink.events().await;
    let row = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::TaskRow { task } => Some(task.clone()),
            _ => None,
        })
        .expect("a TaskRow must have been emitted");
    assert_eq!(row.task_id, "p0lle3dt1");
}

#[tokio::test]
async fn clear_session_rejected_mid_turn() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    // Mark a turn in flight: `ClearSession` must be REJECTED (an `Error` reply)
    // and must NOT reach the engine handle.
    router.set_turn_active(true);
    router
        .route(ClientCommand::ClearSession, sink.clone())
        .await;

    assert!(
        !handle.was_clear_session_called(),
        "ClearSession must not reach the engine while a turn is in flight"
    );
    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert!(
        matches!(events[0], ClientEvent::Error { .. }),
        "mid-turn ClearSession must surface an Error, got {:?}",
        events[0]
    );

    // Once the turn ends, `ClearSession` routes to the engine and reports
    // `SessionEnded`.
    router.set_turn_active(false);
    let sink2 = CapturingSink::arc();
    router.route(ClientCommand::ClearSession, sink2.clone()).await;

    assert!(
        handle.was_clear_session_called(),
        "ClearSession must reach the engine once the turn has ended"
    );
    let events2 = sink2.events().await;
    assert!(
        events2.iter().any(|e| matches!(e, ClientEvent::SessionEnded)),
        "a successful ClearSession must report SessionEnded, got {events2:?}"
    );
}

// ── End-to-end: routing over the real WebSocket transport ─────────────────────
//
// The unit tests above exercise the router directly; this proves the connection
// loop ([`BridgeConnection`]) actually DELEGATES a non-turn command to the bound
// router over a live loopback WebSocket and frames the reply back as a
// `Frame::Event` — i.e. the F2-08 `bind_router` integration works on the wire,
// not just in isolation.

const E2E_TOKEN: &str = "router-e2e-token-32chars00000000";

/// A no-op turn driver (the e2e command under test is not a turn) — `bind`
/// requires one.
struct NoopTurnDriver;
#[async_trait]
impl bridge_server::server::TurnDriver for NoopTurnDriver {
    async fn run_turn(&self, _prompt: String) {}
}

/// A no-op permission sink — the routed command never triggers `check()`.
struct NoopPermissionSink;
#[async_trait]
impl PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: PermissionRequest) {}
}

async fn connect(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let req = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header("sec-websocket-protocol", "mcp")
        .header("x-claude-code-ide-authorization", E2E_TOKEN)
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    ws
}

async fn next_frame<S>(ws: &mut S) -> Frame
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("a frame must arrive within the timeout")
            .expect("stream must yield a message")
            .expect("message must not be a ws error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).expect("decode Frame");
        }
    }
}

async fn send_command<S>(ws: &mut S, command: &ClientCommand)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(bridge::BridgeRequest {
        id: 7,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send command");
}

#[tokio::test]
async fn set_model_routes_over_ws() {
    // A connection bound to BOTH the (unused) turn/permission path and the F2-08
    // command router over the engine's mock handle.
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = Arc::new(router_with(
        handle.clone(),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    )) as Arc<dyn CommandRouter>;

    let connection = BridgeConnection::new();
    let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
    let connection = connection
        .bind(gate, Arc::new(NoopTurnDriver))
        .bind_router(router);

    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    // Drive a `SetModel` command over the wire (no `hello` first — the trusted
    // local-child skeleton routes a command without a mandatory handshake).
    send_command(
        &mut ws,
        &ClientCommand::SetModel {
            model: "claude-opus-4-8".into(),
        },
    )
    .await;

    // The reply is framed back as a `Frame::Event(ModelChanged)`.
    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::ModelChanged { model }) => {
            assert_eq!(model, "claude-opus-4-8");
        }
        other => panic!("expected Frame::Event(ModelChanged) over WS, got {other:?}"),
    }
    // …and the command reached the engine handle.
    assert_eq!(handle.switch_model_call_count(), 1);
    assert_eq!(handle.last_switched_model().as_deref(), Some("claude-opus-4-8"));

    endpoint.shutdown().await;
}
