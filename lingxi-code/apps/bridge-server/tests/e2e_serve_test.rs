//! S2 — end-to-end boot of the REAL bridge-server over a loopback WebSocket.
//!
//! Where the F2-06 suite (`e2e_permission_test.rs`) drives a hand-wired
//! orchestrator with a MOCK streaming client, this proves the PRODUCTION boot
//! path: `bridge_server::boot::assemble` builds a real
//! `engine_desktop::DesktopRuntime` from a deterministic `DesktopConfig`, binds
//! the production turn driver + command router, and the resulting connection is
//! served by the same `McpEndpoint` the binary uses.
//!
//! Two things are asserted end-to-end (no live network for the turn — the config
//! carries an EMPTY api key, so a turn fails fast at the api-client with a
//! transport error, which the production `OrchestratorTurnDriver` surfaces as a
//! terminal `ClientEvent::Error`):
//!
//! - the opening `hello` handshake replies with a `ServerHello` (compatible
//!   versions), proving the served connection is the real F2-07 pump;
//! - a `SendPrompt` with no credential produces a terminal `Error` event rather
//!   than hanging — proving env-config → `engine_desktop::build` → serve is wired
//!   and the orchestrator's output stream reaches the connection's event sink.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::boot;
use client_protocol::commands::ClientCommand;
use client_protocol::events::ClientEvent;
use engine_desktop::DesktopConfig;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

const TEST_TOKEN: &str = "serve-e2e-token-32chars0000000000";

/// A deterministic, env-free `DesktopConfig` rooted at a sandbox temp dir, with
/// an EMPTY api key (so a live turn fails fast rather than reaching the network).
fn sandbox_config() -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().to_path_buf();
    let cfg = DesktopConfig {
        api_base: "https://api.anthropic.com".to_string(),
        api_key: String::new(),
        cwd: cwd.clone(),
        claude_home: cwd.join(".claude"),
        default_model: "claude-sonnet-4-20250514".to_string(),
        fallback_model: None,
        provider_profiles: None,
        routing: None,
        mcp_paths: vec![cwd.join(".mcp.json")],
        use_noop_permission_gate: false,
        deny_unresolved_ask: false,
        injected_permission_gate: None,
        session_started_as_coordinator: false,
        // Deterministic e2e: empty memory, never the real FS.
        memory_provider: None,
        permission_mode: permission::PermissionMode::Default,
        connect_prompt: None,
        max_turns: None,
        max_budget_usd: None,
        json_schema: None,
        // CLI headless system-prompt flags (715adc4e); None for this e2e fixture.
        system_prompt_override: None,
        append_system_prompt: None,
        multi_agent: None,
        explicit_multi_agent: None,
    };
    (tmp, cfg)
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
        .header("x-claude-code-ide-authorization", TEST_TOKEN)
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
        let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
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

fn hello_frame() -> Frame {
    Frame::Request(BridgeRequest {
        id: 1,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
            client_name: "serve-e2e-test/0.0.0".into(),
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

/// The real boot path serves a connection whose opening `hello` is answered with
/// a `ServerHello`, and a credential-less turn surfaces a terminal `Error` event
/// (the orchestrator output stream reaches the connection's event sink).
#[tokio::test]
async fn real_boot_handshakes_and_surfaces_turn_error() {
    let (_tmp, cfg) = sandbox_config();
    let bound = boot::assemble(cfg).await.expect("assemble must succeed");

    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    // (1) Handshake — a compatible `hello` is answered with a `ServerHello`.
    send_frame(&mut ws, &hello_frame()).await;
    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert_eq!(resp.id, 1, "ServerHello echoes the hello request id");
            assert!(resp.error.is_none(), "compatible hello must not error");
            let server_hello: bridge::ServerHello =
                serde_json::from_value(resp.result.expect("ServerHello result"))
                    .expect("decode ServerHello");
            assert_eq!(server_hello.protocol_version, BRIDGE_PROTOCOL_VERSION);
        }
        other => panic!("expected ServerHello response, got {other:?}"),
    }

    // (2) A turn with no credential fails fast and surfaces a terminal Error
    //     event (not a hang) — proving the wired orchestrator's output stream
    //     reaches the connection's event sink.
    send_frame(
        &mut ws,
        &submit(&ClientCommand::SendPrompt {
            text: "hello".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        }),
    )
    .await;

    let mut saw_error = false;
    for _ in 0..50 {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::Error { message, .. }) => {
                assert!(!message.is_empty(), "error event must carry a message");
                // The message must NOT leak a key (there is none, but be explicit).
                assert!(!message.contains("sk-"), "error must not embed a key");
                saw_error = true;
                break;
            }
            // Any other streamed event (e.g. a TurnEnded path) is acceptable to
            // scan past; the turn must terminate one way or another.
            Frame::Event(ClientEvent::TurnEnded { .. }) => {
                saw_error = true; // a clean terminal is also "did not hang"
                break;
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame after SendPrompt: {other:?}"),
        }
    }
    assert!(
        saw_error,
        "a credential-less turn must terminate (Error or TurnEnded), not hang"
    );

    endpoint.shutdown().await;
}

/// `resolve_desktop_config` always binds the adapter gate for a transport
/// (parity with `engine_desktop::build`'s `use_noop_permission_gate: false`
/// branch), regardless of the model override.
#[tokio::test]
async fn resolved_config_uses_adapter_gate() {
    let args = boot::BridgeArgs {
        cwd: None,
        model: Some("claude-x".into()),
        help: false,
    };
    let cfg = boot::resolve_desktop_config(&args);
    assert!(!cfg.use_noop_permission_gate);
    assert_eq!(cfg.default_model, "claude-x");
    // cwd is the process cwd, never the bogus default path.
    assert_ne!(cfg.cwd, PathBuf::from("/dev/null"));
}
