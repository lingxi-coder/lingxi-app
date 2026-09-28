//! F2-07 — Version-mismatch refusal (CLIENT + BRIDGE, independently).
//!
//! The opening handshake exchanges TWO independently-versioned numbers
//! (governing decision §0.10):
//!
//! - `BRIDGE_PROTOCOL_VERSION` — the wire ENVELOPE version (`ClientHello.protocol_version`).
//! - `CLIENT_PROTOCOL_VERSION` — the DTO CONTRACT version carried inside
//!   `Capabilities.client_protocol_version`.
//!
//! A MAJOR-version mismatch in EITHER number refuses the connection: the server
//! replies with a `Frame::Response` error (no `ServerHello`) and never marks the
//! connection handshaken, so no command is routed. This file proves the refusal
//! fires INDEPENDENTLY for each version (one test each) and that a fully-matching
//! handshake still succeeds (the control).
//!
//! The handshake rides the already-frozen request/response envelope (no new
//! `Frame` arm): the client's first frame is
//! `Frame::Request{ method: "hello", params: ClientHello }`; the server answers
//! `Frame::Response{ result: ServerHello }` (accept) or
//! `Frame::Response{ error: BridgeWireError }` (refuse).

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::server::{BridgeConnection, TurnDriver};
use client_adapter::AdapterPermissionGate;
use client_protocol::commands::ClientCommand;
use client_protocol::permission::PermissionRequest;
use client_protocol::version::CLIENT_PROTOCOL_VERSION;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

const TEST_TOKEN: &str = "ver-e2e-token-32chars00000000000";

/// A [`TurnDriver`] that records whether `run_turn` was ever invoked, so a test
/// can assert that a version-REFUSED connection never reaches the engine.
struct RecordingTurnDriver {
    ran: Arc<AtomicBool>,
}

#[async_trait]
impl TurnDriver for RecordingTurnDriver {
    async fn run_turn(&self, _prompt: String) {
        self.ran.store(true, Ordering::SeqCst);
    }
}

/// A [`client_adapter::PermissionRequestSink`] that drops every request — the
/// version-refusal path never emits one, but `BridgeConnection::bind` requires a
/// gate, so we hand it a gate wired to this no-op sink.
struct NoopPermissionSink;

#[async_trait]
impl client_adapter::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: PermissionRequest) {}
}

/// Build a connection BOUND to a recording driver + a real (but unused) gate, so
/// we can observe whether an inbound command reached the engine after a refusal.
fn bound_connection() -> (BridgeConnection, Arc<AtomicBool>) {
    let ran = Arc::new(AtomicBool::new(false));
    let connection = BridgeConnection::new();
    let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
    let driver: Arc<dyn TurnDriver> = Arc::new(RecordingTurnDriver { ran: ran.clone() });
    (connection.bind(gate, driver), ran)
}

/// Send an arbitrary `ClientCommand` as a `Frame::Request` (the post-handshake
/// command envelope — distinct from the `hello` method handshake).
async fn send_command<S>(ws: &mut S, command: &ClientCommand)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(BridgeRequest {
        id: 2,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send command");
}

/// Open an authenticated WS connection to `port` (the WS auth upgrade always
/// succeeds — version refusal is an APPLICATION-LAYER decision that happens
/// AFTER the upgrade, on the first `hello` frame).
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
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).expect("decode Frame"),
            _ => continue,
        }
    }
}

/// Send a `hello` handshake frame carrying the supplied `ClientHello`.
async fn send_hello<S>(ws: &mut S, hello: &ClientHello)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(BridgeRequest {
        id: 1,
        method: "hello".into(),
        params: serde_json::to_value(hello).expect("serialize ClientHello"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send hello");
}

/// Start a bridge-server endpoint with a bare (unbound) connection pump — the
/// version handshake is decided BEFORE any engine wiring, so no orchestrator /
/// gate / driver is needed.
async fn start_endpoint(connection: BridgeConnection) -> McpEndpoint {
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    endpoint
}

/// CONTROL: a fully-matching handshake is ACCEPTED — the server replies with a
/// `ServerHello` (a `Frame::Response` carrying `result`, no `error`).
#[tokio::test]
async fn matching_versions_accept_handshake() {
    let endpoint = start_endpoint(BridgeConnection::new()).await;
    let mut ws = connect(endpoint.port()).await;

    send_hello(
        &mut ws,
        &ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
            client_name: "lingxi-test/0.0.0".into(),
            capabilities: Capabilities::default(),
        },
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert!(
                resp.error.is_none(),
                "matching versions must NOT be refused"
            );
            let result = resp.result.expect("accept must carry a ServerHello result");
            let server_hello: bridge::ServerHello =
                serde_json::from_value(result).expect("ServerHello in result");
            assert_eq!(server_hello.protocol_version, BRIDGE_PROTOCOL_VERSION);
            assert_eq!(
                server_hello.capabilities.client_protocol_version,
                CLIENT_PROTOCOL_VERSION
            );
        }
        other => panic!("expected an accept Frame::Response, got {other:?}"),
    }

    endpoint.shutdown().await;
}

/// BRIDGE version mismatch: the client speaks a DIFFERENT MAJOR wire-envelope
/// version. The handshake is refused (a `Frame::Response` with an `error`, no
/// `ServerHello`).
#[tokio::test]
async fn bridge_version_mismatch_refuses() {
    let endpoint = start_endpoint(BridgeConnection::new()).await;
    let mut ws = connect(endpoint.port()).await;

    // Bump ONLY the wire-envelope major; the client-protocol version stays at the
    // server's, so this isolates the bridge-version check.
    send_hello(
        &mut ws,
        &ClientHello {
            protocol_version: "99.0.0".into(),
            client_name: "lingxi-test/0.0.0".into(),
            capabilities: Capabilities::default(),
        },
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert!(
                resp.result.is_none(),
                "a refused handshake must NOT carry a ServerHello"
            );
            let err = resp
                .error
                .expect("a bridge-version mismatch must be refused with an error");
            assert!(
                err.message.to_lowercase().contains("version"),
                "the refusal message must mention the version mismatch, got: {}",
                err.message
            );
        }
        other => panic!("expected a refusal Frame::Response, got {other:?}"),
    }

    endpoint.shutdown().await;
}

/// CLIENT-PROTOCOL version mismatch: the client speaks the right wire envelope
/// but a DIFFERENT MAJOR DTO-contract version (carried in `Capabilities`). The
/// handshake is refused INDEPENDENTLY of the bridge version.
#[tokio::test]
async fn client_protocol_version_mismatch_refuses() {
    let endpoint = start_endpoint(BridgeConnection::new()).await;
    let mut ws = connect(endpoint.port()).await;

    // Matching wire envelope, but a mismatched MAJOR client-protocol version.
    let caps = Capabilities {
        client_protocol_version: "99.0.0".into(),
        ..Default::default()
    };
    send_hello(
        &mut ws,
        &ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
            client_name: "lingxi-test/0.0.0".into(),
            capabilities: caps,
        },
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert!(
                resp.result.is_none(),
                "a refused handshake must NOT carry a ServerHello"
            );
            let err = resp
                .error
                .expect("a client-protocol-version mismatch must be refused with an error");
            assert!(
                err.message.to_lowercase().contains("version"),
                "the refusal message must mention the version mismatch, got: {}",
                err.message
            );
        }
        other => panic!("expected a refusal Frame::Response, got {other:?}"),
    }

    endpoint.shutdown().await;
}

/// REFUSAL IS LOAD-BEARING: a version-refused connection must NOT route any
/// subsequent command to the engine. This is the security property F2-07 exists
/// to guarantee (governing decision §0.10): "a major mismatch in either refuses
/// the connection" — a refusal is not merely a polite error reply, it MUST stop
/// the peer from driving the engine.
///
/// We bind the connection to a [`RecordingTurnDriver`] that flips a flag the
/// instant `run_turn` is invoked, refuse the handshake on a bad bridge version,
/// then send a `SendPrompt` on the SAME socket. The driver flag must stay false:
/// the refused peer's command is dropped, never reaching the engine.
#[tokio::test]
async fn refused_connection_does_not_route_commands() {
    let (connection, ran) = bound_connection();
    let endpoint = start_endpoint(connection).await;
    let mut ws = connect(endpoint.port()).await;

    // Refuse on a breaking bridge-version mismatch.
    send_hello(
        &mut ws,
        &ClientHello {
            protocol_version: "99.0.0".into(),
            client_name: "lingxi-test/0.0.0".into(),
            capabilities: Capabilities::default(),
        },
    )
    .await;

    // Drain the refusal reply (asserted in detail by the dedicated test above).
    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert!(resp.error.is_some(), "the handshake must be refused");
        }
        other => panic!("expected a refusal Frame::Response, got {other:?}"),
    }

    // Now attempt to drive a turn on the refused connection. A compliant server
    // MUST NOT route it to the engine.
    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "drive a turn".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        },
    )
    .await;

    // Give the server task ample time to (incorrectly) spawn a turn if the guard
    // is missing. The flag must remain false.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !ran.load(Ordering::SeqCst),
        "a version-refused connection must NOT route a command to the engine"
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn command_before_hello_is_rejected() {
    let (connection, ran) = bound_connection();
    let endpoint = start_endpoint(connection).await;
    let mut ws = connect(endpoint.port()).await;

    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            text: "too early".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        },
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Response(response) => {
            let error = response.error.expect("pre-hello command must be rejected");
            assert!(error.message.contains("hello"));
        }
        other => panic!("expected pre-hello error response, got {other:?}"),
    }
    assert!(!ran.load(Ordering::SeqCst));
    endpoint.shutdown().await;
}

#[tokio::test]
async fn second_client_is_rejected_while_first_remains_active() {
    let (connection, ran) = bound_connection();
    let endpoint = start_endpoint(connection).await;
    let mut first = connect(endpoint.port()).await;
    send_hello(
        &mut first,
        &ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "first".into(),
            capabilities: Capabilities::default(),
        },
    )
    .await;
    match next_frame(&mut first).await {
        Frame::Response(response) => assert!(response.error.is_none()),
        other => panic!("expected first ServerHello, got {other:?}"),
    }

    let mut second = connect(endpoint.port()).await;
    send_hello(
        &mut second,
        &ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "second".into(),
            capabilities: Capabilities::default(),
        },
    )
    .await;
    match next_frame(&mut second).await {
        Frame::Response(response) => {
            let error = response.error.expect("second client must be rejected");
            assert!(error.message.contains("active client"));
        }
        other => panic!("expected second-client error response, got {other:?}"),
    }
    drop(second);

    send_command(
        &mut first,
        &ClientCommand::SendPrompt {
            text: "first still owns connection".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ran.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first client command must still route");

    endpoint.shutdown().await;
}
