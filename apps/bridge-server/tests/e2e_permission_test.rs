//! F2-06 — Permission round-trip over WebSocket (the inverted blocking handshake).
//!
//! This proves the hardest cross-task seam of the Electron transport: a tool
//! dispatch on the ENGINE task blocks inside `AdapterPermissionGate::check()` on
//! a oneshot, while a SEPARATE WS read task resolves it from an inbound
//! `ApprovePermission`/`DenyPermission` command — with no deadlock, because the
//! gate `await`s the oneshot (it never holds a blocking lock across the await).
//!
//! The whole stack is exercised over a REAL loopback WebSocket:
//!
//! ```text
//! WS client ──Frame::Request(SendPrompt)──▶ bridge-server frame pump
//!                                            └▶ spawns a turn on the orchestrator
//!     turn 1 stream: text + tool_use ─────────▶ ToolUseStarted (Frame::Event)
//!                                            └▶ gate.check() PARKS on a oneshot
//!                  ◀──Frame::PermissionRequest{request_id}── emitted to client
//! WS client ──Frame::Request(ApprovePermission{request_id, AllowOnce})──▶ read task
//!                                            └▶ gate.resolve() → oneshot fires → Allow
//!     tool dispatches, turn 2 stream: end_turn ▶ TurnEnded (Frame::Event)
//! ```
//!
//! Two cases:
//! - `permission_request_event_then_approve_resolves_check` — approve path: the
//!   parked `check()` resolves `Allow`, the tool proceeds, `TurnEnded` arrives.
//! - `disconnect_mid_permission_denies` — fail-closed: dropping the connection
//!   drains the gate's parked-request map ⇒ the `check()` resolves `Deny` and
//!   the turn completes with the tool's permission-denied result.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::server::{BridgeConnection, TurnDriver};
use client::adapter::{
    AdapterOutputStream, AdapterPermissionGate, ClientEventSink, PermissionRequestSink,
};
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use client::protocol::permission::{PermissionKindDto, PermissionRequest, PermissionResponseDto};
use futures_util::{SinkExt, StreamExt};
use lingxi_core::types::ToolUseId;
use orchestrator::test_support::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
    message_delta_stop, message_start, message_stop, text_delta, MockApiClient,
    MockStreamingApiClient, StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

mod tool_fixture;
use tool_fixture::AlwaysOkTool;

const TEST_TOKEN: &str = "perm-e2e-token-32chars0000000000";

/// A [`TurnDriver`] backed by a real [`ConversationOrchestrator`] wired with a
/// mock streaming client (two scripted turns) + the connection's
/// `AdapterPermissionGate` + `AdapterOutputStream`. `SendPrompt` drives one
/// streaming turn whose first round emits a `tool_use` (triggering `check()`).
struct OrchestratorTurnDriver {
    orchestrator: Arc<ConversationOrchestrator>,
}

#[async_trait]
impl TurnDriver for OrchestratorTurnDriver {
    async fn run_turn(&self, prompt: String) {
        // Errors surface as adapter `Error` events in the full server; here a
        // turn against the scripted mock always succeeds.
        let _ = self.orchestrator.run_turn_streaming(&prompt).await;
    }
}

/// A deterministic reconnect-regression driver. The first prompt parks until the
/// test releases it; if the connection close path fails to abort that spawned
/// turn loop, its stale event + permission request will leak into the next
/// reconnect because the connection reuses one bound driver.
struct ReconnectIsolationDriver {
    event_sink: Arc<dyn ClientEventSink>,
    permission_sink: Arc<dyn PermissionRequestSink>,
    first_started: Arc<tokio::sync::Notify>,
    release_first: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl TurnDriver for ReconnectIsolationDriver {
    async fn run_turn(&self, prompt: String) {
        match prompt.as_str() {
            "first blocked turn" => {
                self.first_started.notify_one();
                self.release_first.notified().await;
                self.event_sink
                    .emit(ClientEvent::TextDelta {
                        text: "STALE-FIRST-EVENT".into(),
                    })
                    .await;
                self.permission_sink
                    .emit_request(PermissionRequest {
                        request_id: 41,
                        kind: PermissionKindDto::ToolUseConfirm {
                            tool_name: "StaleTool".into(),
                            tool_input_json: "{}".into(),
                            default_allow: false,
                        },
                        worker: None,
                        owner: None,
                        suppress_always_allow_rule: false,
                        auto_mode_prompt: None,
                    })
                    .await;
            }
            "second fresh turn" => {
                self.event_sink
                    .emit(ClientEvent::TextDelta {
                        text: "fresh-second-event".into(),
                    })
                    .await;
            }
            other => panic!("unexpected reconnect test prompt: {other}"),
        }
    }
}

/// Build a [`BridgeConnection`] whose orchestrator is wired to the SAME
/// connection-scoped `AdapterOutputStream` + `AdapterPermissionGate` the server
/// drives. The `tool_use` in turn 1 routes through the orchestrator's permission
/// gate (`orch.perms.check`) — the live source of the permission round-trip.
fn build_connection() -> BridgeConnection {
    let tu_id = ToolUseId::new();

    // Turn 1: assistant text, then a tool_use whose `content_block_stop` triggers
    // dispatch → orchestrator permission `check()`.
    let turn1 = scripted![
        message_start("m1", "claude-sonnet-4-20250514"),
        content_block_start_text(0),
        text_delta(0, "let me run a tool"),
        content_block_stop(0),
        content_block_start_tool_use(1, tu_id, "AlwaysOk"),
        input_json_delta(1, "{}"),
        content_block_stop(1),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    // Turn 2: after the tool result the model ends the turn.
    let turn2 = scripted![
        message_start("m2", "claude-sonnet-4-20250514"),
        content_block_start_text(0),
        text_delta(0, "done"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];

    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
    let batched = Arc::new(MockApiClient::new(Vec::new())); // unused on streaming path

    // The server hands the connection a freshly-built event sink + permission
    // sink; we wire them into the orchestrator so the SAME outbound channel feeds
    // streamed events AND permission requests.
    let connection = BridgeConnection::new();
    let output: Arc<dyn lingxi_core::host::OutputStream> =
        Arc::new(AdapterOutputStream::new(connection.event_sink()));
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));

    let mut registry = tool_api::registry::ToolRegistry::new();
    registry.register_builtin(Arc::new(AlwaysOkTool));
    let tools = Arc::new(registry);

    let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        streaming,
        tools,
        orchestrator::test_support::noop_hook_executor(),
        gate.clone() as Arc<dyn permission::gate::PermissionGate>,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    ));

    let driver: Arc<dyn TurnDriver> = Arc::new(OrchestratorTurnDriver { orchestrator });
    connection.bind(gate, driver)
}

/// Open an authenticated WS connection to `port`.
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
        .header("x-lingxi-ide-authorization", TEST_TOKEN)
        .body(())
        .unwrap();
    let (mut ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    let hello = Frame::Request(BridgeRequest {
        id: 0,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "permission-test".into(),
            capabilities: Capabilities::default(),
        })
        .unwrap(),
    });
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .expect("send hello");
    match next_frame(&mut ws).await {
        Frame::Response(response) => assert!(response.error.is_none()),
        other => panic!("expected ServerHello response, got {other:?}"),
    }
    ws
}

/// Read the next `Frame` from the socket, bounded so a deadlock can't hang.
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
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).expect("decode Frame"),
            // tungstenite handles Ping/Pong; ignore anything non-text.
            _ => continue,
        }
    }
}

async fn next_frame_with_timeout<S>(ws: &mut S, timeout: Duration) -> Option<Frame>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let maybe = tokio::time::timeout(timeout, ws.next()).await.ok()?;
        let msg = maybe?.expect("message must not be a ws error");
        match msg {
            Message::Text(t) => return Some(serde_json::from_str(&t).expect("decode Frame")),
            _ => continue,
        }
    }
}

fn send_command<'a, S>(
    ws: &'a mut S,
    command: &ClientCommand,
) -> impl std::future::Future<Output = ()> + 'a
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(bridge::BridgeRequest {
        id: 1,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    async move {
        ws.send(Message::Text(text)).await.expect("send command");
    }
}

#[tokio::test]
async fn permission_request_event_then_approve_resolves_check() {
    let connection = build_connection();
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    // Drive a turn. The first stream round emits a tool_use → the orchestrator's
    // permission `check()` parks on the gate.
    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "use a tool".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;

    // Collect frames until the permission request arrives. Along the way we
    // expect the streamed `TextDelta` + `ToolUseStarted` events.
    let mut saw_text = false;
    let mut saw_tool_started = false;
    let request_id = loop {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::TextDelta { text }) => {
                assert_eq!(text, "let me run a tool");
                saw_text = true;
            }
            Frame::Event(ClientEvent::ToolUseStarted { tool, .. }) => {
                assert_eq!(tool, "AlwaysOk");
                saw_tool_started = true;
            }
            Frame::PermissionRequest(req) => {
                match &req.kind {
                    PermissionKindDto::ToolUseConfirm { tool_name, .. } => {
                        assert_eq!(tool_name, "AlwaysOk");
                    }
                    other => panic!("unexpected permission kind: {other:?}"),
                }
                break req.request_id;
            }
            // CostUpdate / UsageUpdate / ThinkingDelta may interleave with the
            // stream (the §0.7 light-up now feeds usage live) — keep scanning.
            Frame::Event(_) => {}
            other => panic!("unexpected frame before permission request: {other:?}"),
        }
    };
    assert!(saw_text, "the streamed assistant text must arrive first");
    assert!(saw_tool_started, "ToolUseStarted must precede the request");

    // Approve the parked request (the WS read task resolves the engine's oneshot).
    send_command(
        &mut ws,
        &ClientCommand::ApprovePermission {
            request_id,
            response: PermissionResponseDto::AllowOnce,
        },
    )
    .await;

    // The tool now proceeds; collect through to `TurnEnded`.
    let mut saw_tool_result = false;
    let mut saw_turn_ended = false;
    for _ in 0..50 {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::ToolUseResult { tool, is_error, .. }) => {
                assert_eq!(tool, "AlwaysOk");
                assert!(!is_error, "approved tool must not report an error result");
                saw_tool_result = true;
            }
            Frame::Event(ClientEvent::TurnEnded { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                saw_turn_ended = true;
                break;
            }
            // CostUpdate / other events are fine; keep scanning.
            Frame::Event(_) => {}
            other => panic!("unexpected frame after approve: {other:?}"),
        }
    }
    assert!(
        saw_tool_result,
        "the approved tool must have produced a result"
    );
    assert!(
        saw_turn_ended,
        "the turn must end after the tool dispatches"
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn disconnect_mid_permission_denies() {
    let connection = build_connection();
    // Keep a handle to the gate so we can prove the drain happened deterministically.
    let gate = connection.gate_handle();
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "use a tool".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;

    // Wait for the permission request, then DROP the connection mid-permission.
    loop {
        match next_frame(&mut ws).await {
            Frame::PermissionRequest(_) => break,
            Frame::Event(_) => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    // A request is parked on the gate. Closing the socket tears down the
    // connection pump, which DRAINS the gate (fail-closed) ⇒ the parked
    // `check()` resolves `Deny`.
    drop(ws);

    // The gate's parked map must empty out (drain on disconnect). Bounded poll so
    // a regression can't hang the suite.
    let mut drained = false;
    for _ in 0..2000 {
        if gate.pending_count().await == 0 {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        drained,
        "the parked permission request must be drained when the connection drops (fail-closed)"
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn reconnect_does_not_receive_stale_frames_from_aborted_first_turn() {
    let connection = BridgeConnection::new();
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    let driver: Arc<dyn TurnDriver> = Arc::new(ReconnectIsolationDriver {
        event_sink: connection.event_sink(),
        permission_sink: connection.permission_sink(),
        first_started: first_started.clone(),
        release_first: release_first.clone(),
    });
    let endpoint =
        McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection.bind(gate, driver)))
            .await
            .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());

    let mut first = connect(endpoint.port()).await;
    send_command(
        &mut first,
        &ClientCommand::SendPrompt {
            text: "first blocked turn".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;
    first_started.notified().await;

    drop(first);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut second = connect(endpoint.port()).await;
    send_command(
        &mut second,
        &ClientCommand::SendPrompt {
            text: "second fresh turn".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;

    match next_frame_with_timeout(&mut second, Duration::from_secs(2)).await {
        Some(Frame::Event(ClientEvent::TextDelta { text })) => {
            assert_eq!(text, "fresh-second-event");
        }
        other => panic!("expected only the second turn's event after reconnect, got {other:?}"),
    }

    release_first.notify_waiters();

    match next_frame_with_timeout(&mut second, Duration::from_millis(300)).await {
        None => {}
        Some(Frame::Event(ClientEvent::TextDelta { text })) => {
            panic!("stale first-turn event leaked into the reconnect: {text}");
        }
        Some(Frame::PermissionRequest(req)) => {
            panic!("stale permission request leaked into the reconnect: {req:?}");
        }
        Some(other) => panic!("unexpected extra frame after reconnect: {other:?}"),
    }

    endpoint.shutdown().await;
}
