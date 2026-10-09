//! Electron-facing `computer` tool `request_access` round-trip over WebSocket.
//!
//! Mirrors `tests/e2e_permission_test.rs`'s shape byte-for-byte, but for the
//! SEPARATE `ComputerAccessBroker` seam instead of `AdapterPermissionGate`:
//! a tool dispatch on the ENGINE task blocks inside
//! `tool_computer_use::TuiBridgeResolver::resolve()` on a oneshot (the SAME
//! resolver the TUI host uses — generic despite the name), while a SEPARATE WS
//! read task resolves it from an inbound
//! `ApproveComputerAccess`/`DenyComputerAccess` command — with no deadlock, for
//! the identical reason the permission round-trip has none (the resolver
//! `await`s the oneshot without holding a blocking lock across the await).
//!
//! ```text
//! WS client ──Frame::Request(SendPrompt)──▶ bridge-server frame pump
//!                                            └▶ spawns a turn on the orchestrator
//!     turn 1 stream: text + tool_use ─────────▶ ToolUseStarted (Frame::Event)
//!                                            └▶ resolver.resolve() PARKS on a oneshot
//!                                            └▶ broker.run() lowers the exchange,
//!                                               parks the reply, emits the DTO
//!            ◀──Frame::ComputerAccessRequest{request_id}── emitted to client
//! WS client ──Frame::Request(ApproveComputerAccess{request_id, response})──▶ read task
//!                                            └▶ broker.resolve() → oneshot fires → granted
//!     tool dispatches, turn 2 stream: end_turn ▶ TurnEnded (Frame::Event)
//! ```
//!
//! Two cases:
//! - `computer_access_request_event_then_approve_resolves_resolver` — approve
//!   path: the parked resolver resolves the grant, the tool result reflects
//!   the granted apps, `TurnEnded` arrives.
//! - `disconnect_mid_computer_access_denies` — fail-closed: dropping the
//!   connection drains the broker's parked-request map ⇒ the resolver resolves
//!   to the fully-denied default.
//!
//! The orchestrator is bound to `orchestrator::test_support::NoOpPermissionGate`
//! (always `Allow`) rather than the real `AdapterPermissionGate`: the generic
//! per-tool permission check is orthogonal to what this suite proves (the
//! computer-access broker seam), exactly as `e2e_permission_test.rs`'s
//! `AlwaysOkTool` fixture keeps the reverse concern (the tool's OWN
//! `check_permissions`) irrelevant to ITS gate handshake.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::server::{BridgeConnection, TurnDriver};
use client::adapter::{AdapterOutputStream, AdapterPermissionGate, ComputerAccessBroker};
use client::protocol::commands::ClientCommand;
use client::protocol::computer_access::{AccessTierDto, ComputerAccessResponseDto};
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use lingxi_core::types::ToolUseId;
use orchestrator::test_support::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
    message_delta_stop, message_start, message_stop, text_delta, MockApiClient,
    MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

mod computer_access_fixture;
use async_trait::async_trait;
use computer_access_fixture::AccessRequestingTool;

const TEST_TOKEN: &str = "cua-e2e-token-32chars00000000000";

/// A [`TurnDriver`] backed by a real [`ConversationOrchestrator`] wired with a
/// mock streaming client (two scripted turns) + the connection's
/// `AdapterOutputStream`. `SendPrompt` drives one streaming turn whose first
/// round emits a `tool_use` for `AccessRequestingTool`.
struct OrchestratorTurnDriver {
    orchestrator: Arc<ConversationOrchestrator>,
}

#[async_trait]
impl TurnDriver for OrchestratorTurnDriver {
    async fn run_turn(&self, prompt: String) {
        let _ = self.orchestrator.run_turn_streaming(&prompt).await;
    }
}

/// Build a [`BridgeConnection`] whose orchestrator is wired to the SAME
/// connection-scoped `AdapterOutputStream` the server drives, and whose
/// `AccessRequestingTool` routes `request_access` through a REAL
/// `tool_computer_use::TuiBridgeResolver` feeding the connection's
/// `ComputerAccessBroker` — the exact production wiring
/// `boot::assemble_with_provider_keys` performs, minus the full engine.
fn build_connection() -> BridgeConnection {
    let tu_id = ToolUseId::new();

    // Turn 1: assistant text, then a tool_use whose `content_block_stop`
    // dispatches `AccessRequestingTool`.
    let turn1 = scripted![
        message_start("m1", "claude-sonnet-4-20250514"),
        content_block_start_text(0),
        text_delta(0, "let me get access"),
        content_block_stop(0),
        content_block_start_tool_use(1, tu_id, "AccessRequest"),
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

    let connection = BridgeConnection::new();
    let output: Arc<dyn lingxi_core::host::OutputStream> =
        Arc::new(AdapterOutputStream::new(connection.event_sink()));

    // The SAME channel shape `harness_runtime::desktop::build` wires onto
    // `DesktopConfig::computer_access_tx`: the sender drives the generic
    // `TuiBridgeResolver`, the receiver is drained by the connection's broker.
    let (computer_access_tx, computer_access_rx) = tokio::sync::mpsc::channel(8);
    let resolver: Arc<dyn tool_computer_use::ComputerAccessResolver> = Arc::new(
        tool_computer_use::TuiBridgeResolver::new(computer_access_tx),
    );
    let broker = Arc::new(ComputerAccessBroker::new(connection.computer_access_sink()));

    let mut registry = tool_api::registry::ToolRegistry::new();
    registry.register_builtin(Arc::new(AccessRequestingTool {
        access_resolver: resolver,
    }));
    let tools = Arc::new(registry);

    // `NoOpPermissionGate`: the generic per-tool permission check is orthogonal
    // to this suite (see module doc) — always `Allow` so it never interleaves
    // an unrelated `Frame::PermissionRequest` into the frames this test scans.
    let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        streaming,
        tools,
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate) as Arc<dyn permission::gate::PermissionGate>,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    ));

    let driver: Arc<dyn TurnDriver> = Arc::new(OrchestratorTurnDriver { orchestrator });
    // A dummy `AdapterPermissionGate` is still required by `bind()` (the
    // connection's generic permission-resolution seam), but it is never
    // exercised: the orchestrator above is bound to `NoOpPermissionGate`
    // directly, so no `Frame::PermissionRequest` is ever emitted through it.
    let dummy_gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    connection
        .bind(dummy_gate, driver)
        .bind_computer_access(broker, computer_access_rx)
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
            client_name: "computer-access-test".into(),
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
async fn computer_access_request_event_then_approve_resolves_resolver() {
    let connection = build_connection();
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "grant access".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;

    let mut saw_text = false;
    let mut saw_tool_started = false;
    let request_id = loop {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::TextDelta { text }) => {
                assert_eq!(text, "let me get access");
                saw_text = true;
            }
            Frame::Event(ClientEvent::ToolUseStarted { tool, .. }) => {
                assert_eq!(tool, "AccessRequest");
                saw_tool_started = true;
            }
            Frame::ComputerAccessRequest(req) => {
                assert_eq!(req.reason, "automate chat");
                assert_eq!(req.apps.len(), 2);
                assert_eq!(req.apps[0].label, "Slack");
                assert_eq!(req.apps[1].label, "Chrome");
                assert_eq!(req.tier, AccessTierDto::Full);
                assert!(req.tcc_state.is_none());
                break req.request_id;
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame before computer-access request: {other:?}"),
        }
    };
    assert!(saw_text, "the streamed assistant text must arrive first");
    assert!(saw_tool_started, "ToolUseStarted must precede the request");

    // Approve, granting only "Slack" (a partial grant — proves the response
    // round-trips faithfully, not just an allow/deny bit).
    send_command(
        &mut ws,
        &ClientCommand::ApproveComputerAccess {
            request_id,
            response: ComputerAccessResponseDto {
                granted_apps: vec!["Slack".to_string()],
                clipboard_read: false,
                clipboard_write: false,
                system_key_combos: false,
            },
        },
    )
    .await;

    let mut saw_tool_result = false;
    let mut saw_turn_ended = false;
    for _ in 0..50 {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::ToolUseResult {
                tool,
                is_error,
                result_json,
                ..
            }) => {
                assert_eq!(tool, "AccessRequest");
                assert!(!is_error, "the tool call itself must not error");
                assert!(
                    result_json.contains("Slack"),
                    "the tool result must reflect the granted apps: {result_json}"
                );
                saw_tool_result = true;
            }
            Frame::Event(ClientEvent::TurnEnded { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                saw_turn_ended = true;
                break;
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame after approve: {other:?}"),
        }
    }
    assert!(
        saw_tool_result,
        "the approved computer-access grant must have produced a tool result"
    );
    assert!(
        saw_turn_ended,
        "the turn must end after the tool dispatches"
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn disconnect_mid_computer_access_denies() {
    let connection = build_connection();
    let broker = connection.computer_access_broker_handle();
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "grant access".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
            visualization_context: None,
        },
    )
    .await;

    // Wait for the computer-access request, then DROP the connection mid-flight.
    loop {
        match next_frame(&mut ws).await {
            Frame::ComputerAccessRequest(_) => break,
            Frame::Event(_) => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    drop(ws);

    // The broker's parked map must empty out (drain on disconnect). Bounded
    // poll so a regression can't hang the suite.
    let mut drained = false;
    for _ in 0..2000 {
        if broker.pending_count().await == 0 {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        drained,
        "the parked computer-access request must be drained when the connection drops \
         (fail-closed — the awaiting TuiBridgeResolver::resolve resolves to the fully-denied \
         default)"
    );

    endpoint.shutdown().await;
}
