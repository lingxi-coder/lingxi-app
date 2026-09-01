//! Electron-facing audio round-trip over WebSocket.
//!
//! The mirror of `tests/e2e_computer_access_test.rs` for the `AudioBridge`
//! seam: an engine-side trait call parks on a oneshot while a SEPARATE WS read
//! task resolves it from an inbound `AudioResponse` command.
//!
//! ```text
//! engine task ── SpeechToText::transcribe() ──▶ AudioBridge parks the reply
//!                                            └▶ connection's audio sink emits
//!            ◀──Frame::Event(AudioRequest{request_id, op})── pushed to client
//! WS client ──Frame::Request(AudioResponse{request_id, result})──▶ read task
//!                                            └▶ AudioResponder::resolve()
//!                                               → oneshot fires → transcript
//! ```
//!
//! It needs no orchestrator: unlike the computer-access prompt (which is only
//! reachable through a tool dispatch), the audio traits are called directly, so
//! the test IS the engine task. What it proves is the wiring the unit tests in
//! `src/audio_bridge.rs` cannot: that the connection's real sink puts an
//! `AudioRequest` on the wire, that an inbound `AudioResponse` reaches the
//! responder, and that dropping the connection drains a parked request instead
//! of leaving the caller to wait out its deadline.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::audio_bridge::new_audio_bridge;
use bridge_server::server::BridgeConnection;
use client_protocol::commands::{AudioResultDto, ClientCommand};
use client_protocol::events::{AudioOpDto, ClientEvent};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;
use platform_api::stt::{SpeechToText, SttOpts};
use platform_api::voice::{VoiceError, VoiceRecorder};

const TEST_TOKEN: &str = "audio-e2e-token-32chars000000000";

/// Open an authenticated WS connection to `port` and complete the handshake.
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
            client_name: "audio-test".into(),
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
    let frame = Frame::Request(BridgeRequest {
        id: 1,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    async move {
        ws.send(Message::Text(text)).await.expect("send command");
    }
}

/// Wait for the next `AudioRequest` event, skipping any unrelated event.
async fn next_audio_request<S>(ws: &mut S) -> (u64, AudioOpDto)
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match next_frame(ws).await {
            Frame::Event(ClientEvent::AudioRequest { request_id, op }) => {
                return (request_id, op);
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame while awaiting an AudioRequest: {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_audio_request_event_is_answered_by_an_inbound_audio_response() {
    let connection = BridgeConnection::new();
    let (bridge, responder) = new_audio_bridge(connection.audio_sink());
    let endpoint =
        McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection.bind_audio(responder)))
            .await
            .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    let task = tokio::spawn(async move {
        bridge
            .transcribe(SttOpts {
                language: Some("zh-CN".to_string()),
            })
            .await
    });

    let (request_id, op) = next_audio_request(&mut ws).await;
    assert_eq!(
        op,
        AudioOpDto::Transcribe {
            language: Some("zh-CN".to_string())
        },
        "the trait's options must reach the client intact"
    );

    // An answer for an id nobody parked must be ignored, not break the
    // connection — the SAME unknown-id tolerance the computer-access broker has.
    send_command(
        &mut ws,
        &ClientCommand::AudioResponse {
            request_id: request_id + 4096,
            result: AudioResultDto::Transcript {
                text: "for nobody".to_string(),
                language: None,
                confidence: None,
            },
        },
    )
    .await;

    send_command(
        &mut ws,
        &ClientCommand::AudioResponse {
            request_id,
            result: AudioResultDto::Transcript {
                text: "你好".to_string(),
                language: Some("zh-CN".to_string()),
                confidence: None,
            },
        },
    )
    .await;

    let transcript = task
        .await
        .expect("the transcribe task must not panic")
        .expect("the client answered with a transcript");
    assert_eq!(transcript.text, "你好");
    assert_eq!(transcript.language, Some("zh-CN".to_string()));

    endpoint.shutdown().await;
}

#[tokio::test]
async fn disconnect_mid_audio_request_fails_the_parked_call() {
    let connection = BridgeConnection::new();
    let (bridge, responder) = new_audio_bridge(connection.audio_sink());
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(
        connection.bind_audio(responder.clone()),
    ))
    .await
    .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    let task = tokio::spawn(async move { bridge.stop_recording().await });

    let (_request_id, op) = next_audio_request(&mut ws).await;
    assert_eq!(op, AudioOpDto::StopRecording);

    drop(ws);

    // The call must fail on the DRAIN, not on its 30s deadline: the message
    // names the disconnect, which a timed-out request never would.
    let error = task
        .await
        .expect("the stop_recording task must not panic")
        .expect_err("a dropped connection cannot answer");
    match &error {
        VoiceError::Other(message) => assert!(
            message.contains("disconnected"),
            "a dropped connection must drain the parked request rather than let it \
             wait out its deadline, got: {message}"
        ),
        other => panic!("expected VoiceError::Other after a disconnect, got {other:?}"),
    }
    assert_eq!(
        responder.pending_count().await,
        0,
        "the parked request must be removed from the pending table"
    );

    endpoint.shutdown().await;
}

/// With no client attached the sink reports that nobody is listening, so the
/// call fails immediately instead of parking until its deadline.
#[tokio::test]
async fn an_audio_request_with_no_client_connected_fails_immediately() {
    let connection = BridgeConnection::new();
    let (bridge, _responder) = new_audio_bridge(connection.audio_sink());

    let error = bridge
        .stop_recording()
        .await
        .expect_err("there is no client to record anything");
    match &error {
        VoiceError::Other(message) => assert!(
            message.contains("no desktop client"),
            "the error must name the missing client, got: {message}"
        ),
        other => panic!("expected VoiceError::Other with no client, got {other:?}"),
    }
}
