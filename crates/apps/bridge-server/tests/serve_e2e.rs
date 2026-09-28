//! S3 — headless serve-path integration test (transport, keyless).
//!
//! This proves the binary's FULL transport end to end without a key or a
//! network: it starts the REAL serve path the production binary uses —
//! [`McpEndpoint::start_on_ephemeral_port_with_pump`] driving a
//! [`bridge_server::server::BridgeConnection`], with the F2-04 discovery
//! lockfile published + token-enforced via [`bridge_server::boot::publish_lockfile`]
//! (the same library helper `main.rs` calls) — but wires a DETERMINISTIC fake
//! echo [`TurnDriver`] instead of the real orchestrator. No `ANTHROPIC_API_KEY`,
//! no api-client, no live HTTP.
//!
//! End-to-end, it asserts:
//!
//! - the discovery lockfile is written to disk with the SAME `authToken` the
//!   endpoint enforces (a client presenting it upgrades; the test reads the
//!   lockfile back to recover that token, exactly as the Electron app would);
//! - the opening `hello`/`ServerHello` handshake succeeds, with compatible
//!   versions AND a matching `client_protocol_version` (governing decision §0.10
//!   exchanges BOTH numbers);
//! - a `SendPrompt` `Frame::Request` produces the streamed event sequence
//!   `TextDelta`… then a terminal `TurnEnded` — proving the connection's
//!   `ClientEventSink` reaches the wire in order;
//! - the lockfile is reaped on shutdown (the Drop-guard runs).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge::lockfile::IdeLockfile;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::boot;
use bridge_server::server::{BridgeConnection, TurnDriver};
use client_adapter::{AdapterPermissionGate, ClientEventSink};
use client_protocol::commands::ClientCommand;
use client_protocol::events::{ClientEvent, CostDto, TurnOutcomeDto};
use client_protocol::version::CLIENT_PROTOCOL_VERSION;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

/// A deterministic, network-free [`TurnDriver`]: every turn echoes the prompt
/// back as a single `TextDelta`, then ends the turn with a clean `EndTurn`.
///
/// It holds a clone of the connection's [`ClientEventSink`] (the SAME sink the
/// real orchestrator's output stream would feed) and emits straight through it,
/// so the events ride the one connection-scoped outbound channel exactly as a
/// real turn's events do — no orchestrator, no api-client, no key.
struct EchoTurnDriver {
    sink: Arc<dyn ClientEventSink>,
}

#[async_trait]
impl TurnDriver for EchoTurnDriver {
    async fn run_turn(&self, prompt: String) {
        self.sink
            .emit(ClientEvent::TextDelta {
                text: format!("echo: {prompt}"),
            })
            .await;
        self.sink
            .emit(ClientEvent::TurnEnded {
                outcome: TurnOutcomeDto::EndTurn,
                stop_reason: Some("end_turn".to_string()),
                cost: CostDto {
                    total_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    api_calls: 0,
                    session_duration_secs: 0,
                    formatted: "$0.0000".to_string(),
                },
            })
            .await;
    }
}

/// Assemble a fully-bound connection wired to the [`EchoTurnDriver`] — the same
/// `BridgeConnection` shape `boot::assemble` produces, but with a fake driver +
/// a no-op-construction permission gate (no engine, no key).
fn build_echo_connection() -> BridgeConnection {
    let connection = BridgeConnection::new();
    let event_sink = connection.event_sink();
    // The connection's `bind` requires a gate; a fresh `AdapterPermissionGate`
    // wired to the connection's permission sink is sufficient — this test never
    // exercises a tool, so the gate stays empty.
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    let driver: Arc<dyn TurnDriver> = Arc::new(EchoTurnDriver { sink: event_sink });
    connection.bind(gate, driver)
}

/// Open a WS connection to `port` presenting `token` in the IDE auth header.
async fn connect(
    port: u16,
    token: &str,
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
        .header("x-lingxi-ide-authorization", token)
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed with the published token");
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

async fn send_frame<S>(ws: &mut S, frame: &Frame)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = serde_json::to_string(frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send frame");
}

/// A `hello` request carrying our local versions (so the handshake is compatible).
fn hello_frame() -> Frame {
    Frame::Request(BridgeRequest {
        id: 1,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
            client_name: "serve-e2e/0.0.0".into(),
            capabilities: Capabilities::default(),
        })
        .unwrap(),
    })
}

fn submit(command: &ClientCommand) -> Frame {
    Frame::Request(BridgeRequest {
        id: 2,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    })
}

/// The full headless transport: start the real serve path with an echo driver,
/// publish + read back the discovery lockfile, connect with its token, do the
/// handshake, send a prompt, and assert the streamed `TextDelta`…`TurnEnded`
/// sequence — then prove the lockfile is reaped on shutdown.
#[tokio::test]
async fn serve_path_handshakes_streams_turn_and_reaps_lockfile() {
    // (1) Start the REAL endpoint over the echo connection.
    let endpoint =
        McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(build_echo_connection()))
            .await
            .expect("endpoint must bind a loopback port");

    // (2) Publish the discovery lockfile into a sandbox dir (no real $HOME) via
    //     the SAME library helper `main.rs` calls. This writes the file, enforces
    //     its token on the endpoint, and arms the Drop-guard.
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let served =
        boot::publish_lockfile(endpoint, tmp.path().join("bridge"), vec![workspace.clone()])
            .expect("publish private lockfile");
    // Destructure so we own each piece independently: the endpoint is consumed by
    // `shutdown()` (by value), and the guard is dropped on its own to reap the
    // file at a precisely-asserted moment.
    let boot::ServedEndpoint {
        endpoint,
        lockfile_path,
        lock_guard,
    } = served;
    let port = endpoint.port();

    // (3) Read the lockfile back exactly as the Electron app would: the port is
    //     in the filename, the authToken in the body. The recovered token is what
    //     a client must present.
    assert!(lockfile_path.exists(), "lockfile must be written to disk");
    let (body, lock_port) = IdeLockfile::read(&lockfile_path).expect("lockfile parses");
    assert_eq!(lock_port, port, "lockfile filename encodes the bound port");
    assert_eq!(body.transport, "ws");
    assert!(
        !body.auth_token.is_empty(),
        "lockfile must publish a non-empty auth token"
    );
    let token = body.auth_token.clone();

    // (4) Connect with the published token and perform the handshake.
    let mut ws = connect(port, &token).await;
    send_frame(&mut ws, &hello_frame()).await;
    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert_eq!(resp.id, 1, "ServerHello echoes the hello request id");
            assert!(resp.error.is_none(), "compatible hello must not error");
            let server_hello: bridge::ServerHello =
                serde_json::from_value(resp.result.expect("ServerHello result"))
                    .expect("decode ServerHello");
            // BOTH independently-versioned numbers must be compatible / match
            // (governing decision §0.10).
            assert!(
                bridge::version_compatible(BRIDGE_PROTOCOL_VERSION, &server_hello.protocol_version),
                "bridge protocol versions must be compatible"
            );
            assert_eq!(
                server_hello.capabilities.client_protocol_version, CLIENT_PROTOCOL_VERSION,
                "ServerHello must carry the matching client-protocol version"
            );
        }
        other => panic!("expected ServerHello response, got {other:?}"),
    }

    // (5) Send a prompt and assert the streamed event sequence: TextDelta(echo)
    //     then a terminal TurnEnded(end_turn), TextDelta strictly first.
    send_frame(
        &mut ws,
        &submit(&ClientCommand::SendPrompt {
            text: "ping".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        }),
    )
    .await;

    let mut saw_text_at: Option<usize> = None;
    let mut saw_end_at: Option<usize> = None;
    for i in 0..50 {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::TextDelta { text }) => {
                assert_eq!(
                    text, "echo: ping",
                    "echo driver returns the prompt verbatim"
                );
                saw_text_at.get_or_insert(i);
            }
            Frame::Event(ClientEvent::TurnEnded {
                outcome,
                stop_reason,
                ..
            }) => {
                assert_eq!(outcome, TurnOutcomeDto::EndTurn);
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                saw_end_at = Some(i);
                break;
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame after SendPrompt: {other:?}"),
        }
    }
    let text_idx = saw_text_at.expect("a TextDelta must stream from the turn");
    let end_idx = saw_end_at.expect("the turn must terminate with TurnEnded, not hang");
    assert!(
        text_idx < end_idx,
        "TextDelta must arrive before TurnEnded (got text@{text_idx}, end@{end_idx})"
    );

    // (6) Shut down and prove the lockfile is reaped. The Drop-guard removes the
    //     file when it drops; assert it is gone immediately after.
    endpoint.shutdown().await;
    drop(ws);
    assert!(
        lockfile_path.exists(),
        "lockfile present until the guard drops"
    );
    drop(lock_guard);
    assert!(
        !lockfile_path.exists(),
        "the discovery lockfile must be removed on shutdown (Drop-guard)"
    );
}
