//! What `boot::assemble` does with audio — the seam Task 2 built but nothing
//! constructed.
//!
//! `tests/e2e_audio_test.rs` proves the `AudioBridge` works when a test wires it
//! by hand. This file proves the PRODUCTION wiring: that `assemble` builds a
//! bridge over the connection it is assembling, hands it to the engine through
//! `DesktopConfig::audio`, and binds the responder half to that same connection.
//!
//! Three properties, each of which was unprovable while nothing constructed a
//! bridge in production:
//!
//! 1. **It is wired at all** — an assembled runtime carries the capability, and
//!    the tools that depend on it are in the registry the orchestrator
//!    dispatches through.
//! 2. **It is per-connection** — a request parked by one connection's engine can
//!    NOT be resolved by a response arriving on a different connection, even
//!    with the same `request_id`. The bridge and the responder are created as a
//!    pair, per `assemble`; ids are per-bridge counters into per-bridge tables.
//!    This is the property that stops a second Electron window from answering
//!    (or stealing the answer to) the first window's microphone request.
//! 3. **A disconnect drains** — dropping the client fails the parked call
//!    immediately instead of leaving the engine to wait out a 180s deadline.
//!
//! No orchestrator is needed: unlike the computer-access prompt, the audio
//! traits are called directly, so the test IS the engine task. It calls
//! `BoundServer::audio()` — the request half `assemble` built, the same object
//! it put on `DesktopConfig::audio`. That the engine got THAT object is what
//! property 1 asserts, through the registry: the two tools are registered only
//! when the tool context carries the capability.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::boot;
use client::protocol::audio::{
    AudioCapabilitySnapshotDto, AudioOperationDto, AudioOperationKindDto,
    AudioOperationReadinessDto, AudioOperationRequestDto, AudioOperationResultDto,
    AudioReadinessStateDto,
};
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use harness_runtime::desktop::DesktopConfig;
use platform_api::audio::{
    AudioError, AudioErrorKind, AudioOperation, AudioOperationContext, AudioOperationId,
    AudioOperationSuccess, AudioOwner, AudioService,
};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

const TEST_TOKEN: &str = "audio-asm-token-32chars000000000";

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

fn audio_context() -> AudioOperationContext {
    AudioOperationContext {
        identity: AudioOperationId::new(1, 7),
        owner: AudioOwner::Session {
            session_id: "audio-assembly-session".into(),
        },
        initiator: None,
        timeout_budget_ms: Some(5_000),
        max_payload_bytes: 1024,
    }
}

/// Serializes the assembles in this binary. `boot::assemble` builds a real
/// `DesktopRuntime`, which touches process-global runtime state (the same
/// hazard `driver::LOOP_KA_TEST_SERIAL` guards for the in-crate tests, which an
/// integration test cannot reach). Async-aware, because the guard is held
/// across the `assemble` await.
static ASSEMBLE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A deterministic, env-free config rooted at a temp dir.
///
/// `isolated_credential_storage` keeps the developer's login keychain out of the
/// build (otherwise a machine that has ever logged in assembles a different
/// runtime than one that has not), and `session_persistence: false` keeps it off
/// the real transcript store. No turn is ever driven here.
///
/// The managed (policy) root is the fourth thing that has to be isolated and the
/// one `cwd` / `lingxi_home` / `isolated_credential_storage` do not cover:
/// `boot::assemble` folds `harness_runtime::desktop::managed_settings_overlay()` over the
/// file layers, and that falls back to the machine's REAL policy directory
/// (`/Library/Application Support/LingXi/…` on macOS) unless `LINGXI_MANAGED_DIR`
/// is set. Nothing this suite asserts reads a settings VALUE, so an installed
/// policy is unlikely to flip a result today — but "unlikely" is not a property
/// of the harness, and a policy that made `assemble` behave differently would
/// make these tests pass or fail depending on whose laptop ran them.
///
/// `set_var` is process-global. What makes it safe is that every test binds
/// `ASSEMBLE_SERIAL` to a NAMED guard before calling this, so the guard lives
/// until the whole async fn returns and no other test can run between the write
/// and `assemble`'s synchronous read of it. Bound to a bare `_` instead, the
/// guard would drop immediately and serialize nothing while looking identical.
fn sandbox_config() -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().to_path_buf();
    let managed = tmp.path().join("managed");
    std::fs::create_dir_all(&managed).expect("create sandbox managed dir");
    std::env::set_var(
        harness_runtime::desktop::settings_watch::MANAGED_DIR_ENV,
        &managed,
    );
    let cfg = DesktopConfig {
        cwd: cwd.clone(),
        lingxi_home: cwd.join(".lingxi"),
        // `assemble` requires the real adapter gate (it binds the gate handle).
        use_noop_permission_gate: false,
        // Keep the developer's login keychain out of the build: with a real key
        // in it the runtime resolves a different model/provider set than on a
        // machine that has never logged in.
        isolated_credential_storage: true,
        // …and for the same reason, do not let a connected provider re-pick the
        // default model.
        default_model_explicit: true,
        session_persistence: false,
        ..DesktopConfig::default()
    };
    (tmp, cfg)
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
            client_name: "audio-assembly-test".into(),
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
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).expect("decode Frame");
        }
    }
}

/// Wait for the next `AudioRequest` event, skipping any unrelated event.
async fn next_audio_request<S>(ws: &mut S) -> AudioOperationRequestDto
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match next_frame(ws).await {
            Frame::Event(ClientEvent::AudioRequest { request }) => return request,
            Frame::Event(_) => {}
            other => panic!("unexpected frame while awaiting an AudioRequest: {other:?}"),
        }
    }
}

async fn send_command<S>(ws: &mut S, command: &ClientCommand)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(BridgeRequest {
        id: 1,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    ws.send(Message::Text(
        serde_json::to_string(&frame).expect("serialize frame"),
    ))
    .await
    .expect("send command");
}

/// Round-trip a frame the connection always answers, so everything sent BEFORE
/// it has necessarily been dispatched by the time this returns.
async fn barrier(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let hello = Frame::Request(BridgeRequest {
        id: 99,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "audio-assembly-test".into(),
            capabilities: Capabilities {
                audio: Some(audio_capabilities()),
                ..Capabilities::default()
            },
        })
        .unwrap(),
    });
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .expect("send barrier hello");
    loop {
        match next_frame(ws).await {
            Frame::Response(response) => {
                assert!(response.error.is_none(), "barrier hello must be accepted");
                return;
            }
            Frame::Event(_) => {}
            other => panic!("unexpected frame while awaiting the barrier reply: {other:?}"),
        }
    }
}

/// Property 1: assembly wires the capability AND the tools that need it.
///
/// The registry assertion is what proves the bridge reached the ENGINE:
/// `register_desktop_tools` registers these two only when the tool context
/// carries the capability, so their presence means `DesktopConfig::audio`
/// survived the whole way from `assemble` into the tool context.
#[tokio::test]
async fn assemble_gives_the_engine_an_audio_capability_and_its_two_tools() {
    let _serial = ASSEMBLE_SERIAL.lock().await;
    let (_tmp, cfg) = sandbox_config();
    assert!(
        cfg.audio.is_none(),
        "the capability must come from assembly, not from the caller's config"
    );
    let bound = Box::pin(boot::assemble(cfg))
        .await
        .expect("assemble must succeed");

    // The registry is the assertion that matters: registration is gated on the
    // tool context carrying the capability, so these two names are present only
    // if the bridge `assemble` built travelled config → `build` → tool context.
    let names = bound.registered_tool_names();
    assert!(
        names.contains(&"voice".to_string()),
        "the injected capability must register the voice tool; registered: {names:?}"
    );
    assert!(
        names.contains(&"speech".to_string()),
        "the injected capability must register the speech tool; registered: {names:?}"
    );
}

/// Property 2 — the per-connection lifecycle.
///
/// Two independently assembled servers. A `transcribe` on server A's engine
/// parks and pushes its `AudioRequest` to A's client. Answering it on B's socket
/// with the SAME `request_id` must not resolve it: B has its own bridge and its
/// own pending table, so to B that id is simply unknown (a logged no-op). The
/// same answer on A's socket then resolves it.
///
/// Without per-connection pairing — one shared bridge, or a responder bound to
/// the wrong connection — the call would complete after the response on B, and
/// the "still parked" assertion below fails by name.
#[tokio::test]
async fn a_response_on_another_connection_cannot_resolve_this_connections_request() {
    let _serial = ASSEMBLE_SERIAL.lock().await;
    // Assembled one at a time: `sandbox_config` writes the process-global
    // `LINGXI_MANAGED_DIR`, so building both configs up front would leave A
    // reading B's managed root.
    let (_tmp_a, cfg_a) = sandbox_config();
    let bound_a = Box::pin(boot::assemble(cfg_a)).await.expect("assemble A");
    let (_tmp_b, cfg_b) = sandbox_config();
    let bound_b = Box::pin(boot::assemble(cfg_b)).await.expect("assemble B");

    let audio_a = bound_a.audio().clone();
    let endpoint_a = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound_a.connection))
        .await
        .expect("endpoint A must start");
    endpoint_a.set_auth_token(TEST_TOKEN.to_string());
    let endpoint_b = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound_b.connection))
        .await
        .expect("endpoint B must start");
    endpoint_b.set_auth_token(TEST_TOKEN.to_string());
    let mut ws_a = connect(endpoint_a.port()).await;
    let mut ws_b = connect(endpoint_b.port()).await;

    // The engine task: one `transcribe` on A's capability.
    let call: tokio::task::JoinHandle<Result<AudioOperationSuccess, AudioError>> =
        tokio::spawn(async move {
            audio_a
                .execute(audio_context(), AudioOperation::Listen { language: None })
                .await
        });

    let request = next_audio_request(&mut ws_a).await;
    assert!(
        matches!(request.operation, AudioOperationDto::Listen { .. }),
        "A's client must be asked to listen, got {:?}",
        request.operation
    );

    // The wrong connection answers first, with the right id.
    let answer = ClientCommand::AudioResponse {
        identity: request.identity.clone(),
        result: AudioOperationResultDto::Transcript {
            text: "answered by the wrong connection".to_string(),
            language: None,
            confidence: None,
        },
    };
    send_command(&mut ws_b, &answer).await;

    // Ordering barrier, so "still parked" is a fact and not a race: a
    // connection routes inbound frames one at a time, so a REPLY to a frame sent
    // after the answer proves the answer was already dispatched. A second
    // `hello` is the only inbound frame that is always answered, and repeating
    // it is idempotent (compatible versions re-send `ServerHello`).
    barrier(&mut ws_b).await;
    assert!(
        !call.is_finished(),
        "a response on ANOTHER connection must not resolve this connection's \
         parked audio request"
    );

    // The right connection answers the same id.
    let answer = ClientCommand::AudioResponse {
        identity: request.identity.clone(),
        result: AudioOperationResultDto::Transcript {
            text: "answered by the right connection".to_string(),
            language: Some("en-US".to_string()),
            confidence: Some(0.9),
        },
    };
    send_command(&mut ws_a, &answer).await;

    let transcript = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the parked call must resolve once ITS OWN connection answers")
        .expect("the call task must not panic")
        .expect("the answer is a success result");
    assert!(
        matches!(
            transcript,
            AudioOperationSuccess::Transcript { transcript }
                if transcript.text == "answered by the right connection"
        ),
        "the resolved value must be the one its OWN connection sent — had the \
     other connection's response resolved the call, this would carry its text"
    );
}

/// Property 3 — a disconnect drains what the assembled connection parked.
///
/// Dropping the socket must fail the parked call right away. Without the
/// responder bound to this connection, `on_close` has nothing to drain and the
/// call would sit until its 180s deadline — far past the 10s bound below, so
/// this assertion is load-bearing on `bind_audio` having been called with the
/// bridge's OWN responder.
#[tokio::test]
async fn dropping_the_client_fails_an_assembled_connections_parked_call() {
    let _serial = ASSEMBLE_SERIAL.lock().await;
    let (_tmp, cfg) = sandbox_config();
    let bound = Box::pin(boot::assemble(cfg))
        .await
        .expect("assemble must succeed");
    let audio = bound.audio().clone();
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    let call: tokio::task::JoinHandle<Result<AudioOperationSuccess, AudioError>> =
        tokio::spawn(async move {
            audio
                .execute(audio_context(), AudioOperation::Listen { language: None })
                .await
        });
    let _request = next_audio_request(&mut ws).await;

    drop(ws);

    let error = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("a disconnect must fail the parked call, not leave it to its deadline")
        .expect("the call task must not panic")
        .expect_err("a drained request cannot produce a transcript");
    assert_eq!(error.kind, AudioErrorKind::NativeFailure);
}
