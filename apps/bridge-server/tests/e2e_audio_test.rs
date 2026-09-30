//! Electron-facing audio round-trip over WebSocket.
//!
//! The mirror of `tests/e2e_computer_access_test.rs` for the `AudioBridge`
//! seam: an engine-side trait call parks on a oneshot while a SEPARATE WS read
//! task resolves it from an inbound `AudioResponse` command.
//!
//! ```text
//! engine task ── AudioService::execute(Listen) ──▶ AudioBridge parks the reply
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
use client::protocol::audio::{
    AudioCapabilitySnapshotDto, AudioOperationDto, AudioOperationIdDto, AudioOperationKindDto,
    AudioOperationReadinessDto, AudioOperationRequestDto, AudioOperationResultDto,
    AudioReadinessStateDto,
};
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use platform_api::audio::{
    AudioOperation, AudioOperationContext, AudioOperationId, AudioOperationSuccess, AudioOwner,
    AudioRecordingHandle, AudioService,
};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

const TEST_TOKEN: &str = "audio-e2e-token-32chars000000000";

fn audio_capabilities() -> AudioCapabilitySnapshotDto {
    AudioCapabilitySnapshotDto {
        service_epoch: 7,
        support_revision: 1,
        supported_operations: vec![
            AudioOperationKindDto::Record,
            AudioOperationKindDto::Listen,
            AudioOperationKindDto::Synthesize,
            AudioOperationKindDto::Speak,
        ],
        readiness: vec![AudioOperationReadinessDto {
            operation: AudioOperationKindDto::Listen,
            state: AudioReadinessStateDto::Ready,
        }],
        max_payload_bytes: 1024,
    }
}

fn context() -> AudioOperationContext {
    AudioOperationContext {
        identity: AudioOperationId::new(1, 7),
        owner: AudioOwner::Session {
            session_id: "session-audio-test".into(),
        },
        initiator: None,
        timeout_budget_ms: Some(5_000),
        max_payload_bytes: 1024,
    }
}

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
            capabilities: Capabilities {
                audio: Some(audio_capabilities()),
                ..Capabilities::default()
            },
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
async fn next_audio_request<S>(ws: &mut S) -> AudioOperationRequestDto
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match next_frame(ws).await {
            Frame::Event(ClientEvent::AudioRequest { request }) => {
                return request;
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
            .execute(
                context(),
                AudioOperation::Listen {
                    language: Some("zh-CN".to_string()),
                },
            )
            .await
    });

    let request = next_audio_request(&mut ws).await;
    assert_eq!(
        request.operation,
        AudioOperationDto::Listen {
            language: Some("zh-CN".to_string())
        },
        "the operation's options must reach the client intact"
    );

    // An answer for an id nobody parked must be ignored, not break the
    // connection — the SAME unknown-id tolerance the computer-access broker has.
    send_command(
        &mut ws,
        &ClientCommand::AudioResponse {
            identity: AudioOperationIdDto {
                id: "unknown-operation".into(),
                ..request.identity.clone()
            },
            result: AudioOperationResultDto::Transcript {
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
            identity: request.identity,
            result: AudioOperationResultDto::Transcript {
                text: "你好".to_string(),
                language: Some("zh-CN".to_string()),
                confidence: None,
            },
        },
    )
    .await;

    let result = task
        .await
        .expect("the listen task must not panic")
        .expect("the client answered with a transcript");
    assert!(
        matches!(result, AudioOperationSuccess::Transcript { transcript } if transcript.text == "你好" && transcript.language.as_deref() == Some("zh-CN"))
    );

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

    let task = tokio::spawn(async move {
        bridge
            .execute(
                context(),
                AudioOperation::StopRecording {
                    handle: AudioRecordingHandle("recording-1".into()),
                },
            )
            .await
    });

    let request = next_audio_request(&mut ws).await;
    assert_eq!(
        request.operation,
        AudioOperationDto::StopRecording {
            handle: "recording-1".into()
        }
    );

    drop(ws);

    // The call must fail on the DRAIN, not on its 30s deadline: the message
    // names the disconnect, which a timed-out request never would.
    let error = task
        .await
        .expect("the stop_recording task must not panic")
        .expect_err("a dropped connection cannot answer");
    assert_eq!(
        error.kind,
        platform_api::audio::AudioErrorKind::NativeFailure
    );
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
    let (bridge, responder) = new_audio_bridge(connection.audio_sink());
    responder.update_capabilities(Some(audio_capabilities()));

    let error = bridge
        .execute(
            context(),
            AudioOperation::StopRecording {
                handle: AudioRecordingHandle("recording-1".into()),
            },
        )
        .await
        .expect_err("there is no client to record anything");
    assert_eq!(error.kind, platform_api::audio::AudioErrorKind::Unavailable);
}

#[tokio::test]
async fn capability_updates_replace_the_live_snapshot_and_notify_the_client() {
    let connection = BridgeConnection::new();
    let (bridge, responder) = new_audio_bridge(connection.audio_sink());
    let endpoint =
        McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection.bind_audio(responder)))
            .await
            .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;
    let mut update = audio_capabilities();
    update.support_revision = 2;
    update.supported_operations = vec![AudioOperationKindDto::Record];

    send_command(
        &mut ws,
        &ClientCommand::UpdateAudioCapabilities {
            capabilities: update.clone(),
        },
    )
    .await;
    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::AudioCapabilitiesChanged { capabilities }) => {
            assert_eq!(capabilities, update);
        }
        other => panic!("expected capability-change event, got {other:?}"),
    }
    let current = bridge.capabilities();
    assert_eq!(current.support_revision, 2);
    assert_eq!(
        current.supported_operations,
        vec![platform_api::audio::AudioOperationKind::Record]
    );

    endpoint.shutdown().await;
}
