//! Exercise connection teardown through the real Bridge pump and socket.
//!
//! Each fixture owns its attachment host, so an intentionally Unknown
//! attachment cannot poison another test's process-wide app host.
use super::*;
use crate::audio_bridge::new_audio_bridge;
use crate::server::BridgeConnection;
use bridge::{
    BridgeRequest, Capabilities, ClientHello, Frame, McpEndpoint, BRIDGE_PROTOCOL_VERSION,
};
use client::protocol::audio::{
    AudioCapabilitySnapshotDto, AudioOperationDto, AudioOperationKindDto, AudioOperationResultDto,
    AudioOwnerDto,
};
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use lingxi_core::host::audio::{
    AudioOperation, AudioOperationContext, AudioOperationId, AudioOperationSuccess, AudioService,
};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use tokio_tungstenite::tungstenite::{handshake::client::generate_key, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Fixture {
    endpoint: McpEndpoint,
    socket: Socket,
    rpc: Arc<MediaServiceRpc>,
    attachment: MediaAttachmentHandle,
    audio: Arc<dyn AudioService>,
    service: Arc<dyn MediaService>,
    owner: MediaOwner,
    connector: Arc<remote_control_client::host_media::RemoteMediaConnector>,
}

fn wire_request<T: serde::Serialize>(id: u64, method: &str, params: &T) -> Frame {
    Frame::Request(BridgeRequest {
        trace_context: None,
        id,
        method: method.into(),
        params: Utf16JsonProjection::parse(&serde_json::to_string(params).unwrap()).unwrap(),
    })
}

async fn send(socket: &mut Socket, frame: Frame) {
    socket
        .send(Message::Text(serde_json::to_string(&frame).unwrap()))
        .await
        .unwrap();
}

async fn next_frame(socket: &mut Socket) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .expect("Bridge peer frame deadline")
            .expect("Bridge socket ended")
            .expect("Bridge socket read failed");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn fixture() -> Fixture {
    let connection = BridgeConnection::new();
    let (device, responder) = new_audio_bridge(connection.audio_sink());
    let resources = Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default());
    let service: Arc<dyn MediaService> = media_runtime::MediaRuntimeBuilder::new(resources.clone())
        .audio(device.clone())
        .service_epoch(7)
        .build();
    let owner = MediaOwner::Session {
        session_id: "teardown-peer-session".into(),
    };
    let mut rpc = MediaServiceRpc::new(service.clone(), owner.clone()).unwrap();
    let connector = remote_control_client::host_media::RemoteMediaConnector::new(
        Arc::new(crate::media_bridge::DesktopRemoteMediaConfigProvider::from_process_environment()),
        resources,
    );
    let (connection_channel, connection_responder) = connection.media_connection_channel();
    let (control_channel, control_responder) = connection.media_control_channel();
    let rpc_mut = Arc::get_mut(&mut rpc).unwrap();
    rpc_mut.remote_connector = Some(connector.clone());
    rpc_mut.media_connection_channel = Some(connection_channel);
    rpc_mut.media_control_channel = Some(control_channel);
    let attachment = rpc.current_attachment().unwrap();
    let audio = attachment.audio_service(device).unwrap();
    let connection = Arc::new(
        connection
            .bind_audio(responder)
            .bind_media_connection(connection_responder)
            .bind_media_control(control_responder)
            .bind_media_service(rpc.clone()),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("teardown-test-token".into());
    let socket = connect(&endpoint).await;
    Fixture {
        endpoint,
        socket,
        rpc,
        attachment,
        audio,
        service,
        owner,
        connector,
    }
}

async fn connect(endpoint: &McpEndpoint) -> Socket {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("ws://127.0.0.1:{}/mcp", endpoint.port()))
        .header("host", format!("127.0.0.1:{}", endpoint.port()))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header(bridge::AUTH_HEADER_NAME, "teardown-test-token")
        .body(())
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let capabilities = Capabilities {
        audio: Some(AudioCapabilitySnapshotDto {
            service_epoch: 7,
            support_revision: 1,
            supported_operations: vec![AudioOperationKindDto::Record],
            readiness: Vec::new(),
            max_payload_bytes: 1024,
        }),
        ..Capabilities::default()
    };
    send(
        &mut socket,
        wire_request(
            1,
            "hello",
            &ClientHello {
                protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
                client_name: "attachment-teardown-test".into(),
                capabilities,
            },
        ),
    )
    .await;
    match next_frame(&mut socket).await {
        Frame::Response(response) => assert!(response.error.is_none(), "{response:?}"),
        other => panic!("expected hello response, got {other:?}"),
    }
    socket
}

async fn disconnect(fixture: &mut Fixture) -> MediaAttachmentPhase {
    fixture.socket.close(None).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.attachment.wait_for_admission_closed(),
    )
    .await
    .expect("socket disconnect must close attachment admission");
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.attachment.wait_for_drain_completion(),
    )
    .await
    .expect("attachment drain must settle")
}

#[tokio::test]
async fn registered_idle_owner_disconnects_without_device_status() {
    let mut fixture = fixture().await;
    let permit = fixture.attachment.authorize_owner(&fixture.owner).unwrap();
    assert_eq!(permit.logical_owner(), &fixture.owner);
    drop(permit);

    // The peer never receives a native operation and therefore owes no device
    // cleanup receipt. Closing a registration-only attachment must succeed.
    let phase = disconnect(&mut fixture).await;
    fixture.endpoint.shutdown().await;
    assert_eq!(phase, MediaAttachmentPhase::Revoked);
    assert!(fixture
        .rpc
        .attachment_host
        .attachment(fixture.attachment.id())
        .is_none());
    assert!(fixture
        .rpc
        .attachment_host
        .attach(fixture.service.clone())
        .is_ok());
}

async fn register_route(socket: &mut Socket, request_id: &str) {
    send(
        socket,
        wire_request(
            20,
            "command",
            &ClientCommand::MediaServiceRequest {
                request: dto::MediaServiceRequestDto {
                    request_id: request_id.into(),
                    action: dto::MediaServiceActionDto::RegisterConnection {
                        connection_id: "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61".into(),
                        label: "socket route".into(),
                        supported_modes: vec![dto::MediaSessionModeDto::Audio],
                        source_kinds: vec![dto::MediaSourceKindDto::Microphone],
                    },
                },
            },
        ),
    )
    .await;
    match next_frame(socket).await {
        Frame::Event(ClientEvent::MediaServiceResponse { response }) => {
            assert_eq!(response.request_id, request_id);
            assert!(
                matches!(
                    &response.result,
                    Some(dto::MediaServiceResultDto::ConnectionUpdated)
                ),
                "{response:?}"
            );
        }
        other => panic!("expected route registration response, got {other:?}"),
    }
}

#[tokio::test]
async fn real_socket_reconnect_removes_old_route_and_rejects_old_epoch_work() {
    let mut fixture = fixture().await;
    let old_token = fixture.rpc.connection_token();
    let old_epoch = fixture.rpc.request_attachment_epoch();
    let old_owner = fixture
        .owner
        .clone()
        .in_attachment(fixture.attachment.id().clone());
    register_route(&mut fixture.socket, "old-register").await;
    assert_eq!(
        fixture.connector.supported_modes(&old_owner).await,
        vec![MediaSessionMode::Audio]
    );
    assert_eq!(
        disconnect(&mut fixture).await,
        MediaAttachmentPhase::Revoked
    );
    assert!(fixture
        .connector
        .supported_modes(&old_owner)
        .await
        .is_empty());

    fixture.socket = connect(&fixture.endpoint).await;
    let replacement = fixture.rpc.current_attachment().unwrap();
    assert_ne!(replacement.id(), fixture.attachment.id());
    assert_eq!(replacement.phase(), MediaAttachmentPhase::Active);
    let stale = fixture
        .rpc
        .handle_for_connection(
            dto::MediaServiceRequestDto {
                request_id: "old-epoch-status".into(),
                action: dto::MediaServiceActionDto::Execute {
                    operation: dto::MediaOperationDto::Status { target: None },
                    timeout_budget_ms: Some(1_000),
                    max_payload_bytes: None,
                },
            },
            old_token,
            old_epoch,
        )
        .await;
    assert_eq!(
        stale.error.map(|error| error.kind),
        Some(dto::MediaErrorKindDto::Unavailable)
    );
    register_route(&mut fixture.socket, "replacement-register").await;
    let replacement_owner = fixture
        .owner
        .clone()
        .in_attachment(replacement.id().clone());
    assert_eq!(
        fixture.connector.supported_modes(&replacement_owner).await,
        vec![MediaSessionMode::Audio]
    );
    fixture.attachment = replacement;
    assert_eq!(
        disconnect(&mut fixture).await,
        MediaAttachmentPhase::Revoked
    );
    fixture.endpoint.shutdown().await;
}

#[tokio::test]
async fn socket_close_does_not_release_an_independent_unconfirmed_host_cleanup() {
    let mut fixture = fixture().await;
    register_route(&mut fixture.socket, "fenced-register").await;
    let mut external = fixture.attachment.retain_for_host_cleanup().unwrap();
    fixture.socket.close(None).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.attachment.wait_for_admission_closed(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let routes_empty = fixture.rpc.registered_routes.lock().await.is_empty();
            let rpc_cleanup_done = !fixture
                .rpc
                .cleanup_fences
                .lock()
                .unwrap()
                .contains_key(fixture.attachment.id());
            if fixture.attachment.media_drain_confirmed() && routes_empty && rpc_cleanup_done {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the media and RPC route cleanup should settle");
    assert_eq!(fixture.attachment.phase(), MediaAttachmentPhase::Revoking);
    assert!(
        fixture
            .rpc
            .attachment_host
            .attach(fixture.service.clone())
            .is_err(),
        "another connection must not claim an unconfirmed host scope"
    );
    external.confirm().unwrap();
    assert_eq!(fixture.attachment.phase(), MediaAttachmentPhase::Revoked);
    fixture.socket = connect(&fixture.endpoint).await;
    fixture.attachment = fixture.rpc.current_attachment().unwrap();
    assert_eq!(
        disconnect(&mut fixture).await,
        MediaAttachmentPhase::Revoked
    );
    fixture.endpoint.shutdown().await;
}

#[tokio::test]
async fn disconnected_recording_without_native_receipt_retains_unknown_attachment() {
    let mut fixture = fixture().await;
    let audio = fixture.audio.clone();
    let owner = fixture.owner.clone();
    let start = tokio::spawn(async move {
        audio
            .execute(
                AudioOperationContext::new(AudioOperationId::new(1, 7), owner),
                AudioOperation::StartRecording {
                    sample_rate_hz: 16_000,
                    format: "wav".into(),
                },
            )
            .await
    });
    let request = match next_frame(&mut fixture.socket).await {
        Frame::Event(ClientEvent::AudioRequest { request }) => request,
        other => panic!("expected a native start request, got {other:?}"),
    };
    assert!(matches!(
        request.operation,
        AudioOperationDto::StartRecording { .. }
    ));
    assert_eq!(
        request.owner,
        AudioOwnerDto::Attached {
            attachment_host_id: fixture.attachment.id().host_id(),
            attachment_generation: fixture.attachment.id().generation(),
            logical_kind: dto::MediaOwnerKindDto::Session,
            logical_owner_id: "teardown-peer-session".into(),
        }
    );
    assert_eq!(request.identity.service_epoch, 7);
    send(
        &mut fixture.socket,
        wire_request(
            2,
            "command",
            &ClientCommand::AudioResponse {
                identity: request.identity,
                result: AudioOperationResultDto::RecordingStarted {
                    handle: "peer-recording-1".into(),
                },
            },
        ),
    )
    .await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), start).await.unwrap().unwrap(),
        Ok(AudioOperationSuccess::RecordingStarted { handle }) if handle.0.0 == "peer-recording-1"
    ));

    // The fake peer has acknowledged a live recording but sends no EndOwner
    // or inactive-status receipt before its socket disappears.
    let phase = disconnect(&mut fixture).await;
    fixture.endpoint.shutdown().await;
    assert_eq!(phase, MediaAttachmentPhase::Unknown);
    assert!(fixture.attachment.authorize_owner(&fixture.owner).is_err());
    assert!(fixture
        .rpc
        .attachment_host
        .attachment(fixture.attachment.id())
        .is_some());
    assert_eq!(
        fixture
            .rpc
            .attachment_host
            .attach(fixture.service.clone())
            .unwrap_err()
            .kind,
        MediaErrorKind::Unavailable
    );
}

#[tokio::test]
async fn same_bridge_socket_reconnect_keeps_shared_app_host_usable() {
    // AppMediaHost intentionally has a process-wide attachment registry.
    // Share the host-test lock until both socket drains really complete.
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let connection = BridgeConnection::new();
    let (audio, audio_responder) = new_audio_bridge(connection.audio_sink());
    let (callback, media_responder) =
        crate::media_bridge::new_media_device_callback(connection.audio_sink());
    let host = audio_provider::media_host::AppMediaHost::new(
        Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default()),
        audio,
        None,
        Arc::new(audio_provider::media_native::CallbackMediaDevicePorts::new(
            callback,
        )),
        Vec::new(),
        Arc::new(audio_provider::media_cloud::AudioProviderCloudRegistry::new("wav")),
        Vec::new(),
        media_runtime::AudioRoutingSettings::default(),
        7,
        1024,
    )
    .unwrap();
    let first = host.create_media_attachment().unwrap();
    host.set_current_media_attachment(Some(first.clone()))
        .unwrap();
    let owner = MediaOwner::Session {
        session_id: "teardown-peer-session".into(),
    };
    let rpc = MediaServiceRpc::for_app_host(host.clone(), owner.clone()).unwrap();
    let connection = Arc::new(
        connection
            .bind_audio(audio_responder)
            .bind_media_device(media_responder)
            .bind_media_service(rpc.clone()),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("teardown-test-token".into());
    let mut socket = connect(&endpoint).await;
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), first.wait_for_admission_closed())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first.wait_for_drain_completion())
            .await
            .unwrap(),
        MediaAttachmentPhase::Revoked
    );

    socket = connect(&endpoint).await;
    let second = host
        .current_media_attachment()
        .expect("reconnect publishes a new host attachment");
    assert_ne!(first.id(), second.id());
    assert_eq!(second.phase(), MediaAttachmentPhase::Active);
    assert!(
        !host
            .media_service()
            .capabilities_for(&owner)
            .supported_operations
            .is_empty(),
        "shared AppMediaHost remains usable"
    );
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), second.wait_for_admission_closed())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), second.wait_for_drain_completion())
            .await
            .unwrap(),
        MediaAttachmentPhase::Revoked
    );
    endpoint.shutdown().await;
}

struct TransitionDriver {
    session: Arc<tokio::sync::Mutex<String>>,
}

#[async_trait::async_trait]
impl crate::server::TurnDriver for TransitionDriver {
    async fn run_turn(&self, _: Utf16JsonProjection) {}
    async fn current_session_id(&self) -> Option<String> {
        Some(self.session.lock().await.clone())
    }
}

struct TransitionRouter {
    session: Arc<tokio::sync::Mutex<String>>,
    rpc: Arc<MediaServiceRpc>,
    prepared: std::sync::Mutex<Vec<(MediaOwner, MediaAttachmentId)>>,
}

#[async_trait::async_trait]
impl crate::router::CommandRouter for TransitionRouter {
    async fn route(&self, command: ClientCommand, _: Arc<dyn client::adapter::ClientEventSink>) {
        assert_eq!(
            self.rpc.current_attachment().unwrap().phase(),
            MediaAttachmentPhase::Revoking
        );
        match command {
            ClientCommand::NewSession { .. } => *self.session.lock().await = "new-session".into(),
            ClientCommand::ResumeSession { session_id, .. } if session_id == "resume-session" => {
                *self.session.lock().await = session_id;
            }
            ClientCommand::ResumeSession { .. } => {} // Simulate a failed resume.
            _ => panic!("unexpected session command"),
        }
    }

    async fn audio_session_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            Some(self.session.lock().await.clone()),
            Some("profile".into()),
            Some("profile:profile".into()),
        )
    }

    async fn prepare_media_attachment(
        &self,
        attachment: &MediaAttachmentHandle,
        owner: &MediaOwner,
    ) -> Result<(), String> {
        assert!(self.rpc.transition_pending.load(Ordering::Acquire));
        assert_eq!(
            self.rpc.current_attachment().unwrap().phase(),
            MediaAttachmentPhase::Revoked
        );
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Active);
        self.prepared
            .lock()
            .unwrap()
            .push((owner.clone(), attachment.id().clone()));
        Ok(())
    }
}

async fn wait_for_owner_replacement(
    rpc: &MediaServiceRpc,
    previous: &MediaAttachmentId,
    expected: &str,
) -> MediaAttachmentHandle {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let owner = rpc.owner.read().await.clone();
            if let Ok(current) = rpc.current_attachment() {
                if current.id() != previous
                    && current.phase() == MediaAttachmentPhase::Active
                    && !rpc.transition_pending.load(Ordering::Acquire)
                    && owner
                        == (MediaOwner::Session {
                            session_id: expected.into(),
                        })
                {
                    break current;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session transition should publish its prepared attachment")
}

async fn enqueue_context(socket: &mut Socket, request_id: &str) {
    send(
        socket,
        wire_request(
            25,
            "command",
            &ClientCommand::GetAudioSessionContext {
                request_id: request_id.into(),
            },
        ),
    )
    .await;
}

async fn assert_context_binding(
    socket: &mut Socket,
    request_id: &str,
    session_id: &str,
    attachment: &MediaAttachmentHandle,
) {
    match next_frame(socket).await {
        Frame::Event(ClientEvent::AudioSessionContext {
            request_id: got,
            session_id: Some(session),
            media_owner:
                Some(dto::MediaOwnerDto::Attached {
                    attachment_host_id,
                    attachment_generation,
                    logical_kind: dto::MediaOwnerKindDto::Session,
                    logical_owner_id,
                }),
            ..
        }) => {
            assert_eq!(got, request_id);
            assert_eq!(session, session_id);
            assert_eq!(logical_owner_id, session_id);
            assert_eq!(attachment_host_id, attachment.id().host_id());
            assert_eq!(attachment_generation, attachment.id().generation());
        }
        other => panic!("expected the committed attachment context, got {other:?}"),
    }
}

#[tokio::test]
async fn socket_context_without_session_cannot_install_binding() {
    let mut fixture = fixture().await;
    enqueue_context(&mut fixture.socket, "no-session").await;
    match next_frame(&mut fixture.socket).await {
        Frame::Event(ClientEvent::AudioSessionContext {
            request_id,
            session_id: None,
            profile_id: None,
            account_scope: None,
            media_owner: None,
        }) => assert_eq!(request_id, "no-session"),
        other => panic!("expected an unavailable context, got {other:?}"),
    }
    assert_eq!(
        disconnect(&mut fixture).await,
        MediaAttachmentPhase::Revoked
    );
    fixture.endpoint.shutdown().await;
}

#[tokio::test]
async fn socket_new_and_resume_prepare_attachment_before_publishing_it() {
    let service: Arc<dyn MediaService> = media_runtime::MediaRuntimeBuilder::new(Arc::new(
        audio_provider::media_store::BoundedMediaResourceStore::default(),
    ))
    .service_epoch(7)
    .build();
    let rpc = MediaServiceRpc::new(
        service,
        MediaOwner::Session {
            session_id: "old-session".into(),
        },
    )
    .unwrap();
    let session = Arc::new(tokio::sync::Mutex::new("old-session".to_owned()));
    let router = Arc::new(TransitionRouter {
        session: session.clone(),
        rpc: rpc.clone(),
        prepared: std::sync::Mutex::new(Vec::new()),
    });
    let connection = BridgeConnection::new();
    let gate = Arc::new(client::adapter::AdapterPermissionGate::new(
        connection.permission_sink(),
    ));
    let connection = Arc::new(
        connection
            .bind(gate, Arc::new(TransitionDriver { session }))
            .bind_router(router.clone())
            .bind_media_service(rpc.clone()),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("teardown-test-token".into());
    let mut socket = connect(&endpoint).await;
    let old = rpc.current_attachment().unwrap();
    enqueue_context(&mut socket, "initial-context").await;
    assert_context_binding(&mut socket, "initial-context", "old-session", &old).await;
    send(
        &mut socket,
        wire_request(
            21,
            "command",
            &ClientCommand::NewSession {
                cwd: None,
                model: None,
            },
        ),
    )
    .await;
    enqueue_context(&mut socket, "new-context").await;
    let new = wait_for_owner_replacement(&rpc, old.id(), "new-session").await;
    assert_context_binding(&mut socket, "new-context", "new-session", &new).await;
    send(
        &mut socket,
        wire_request(
            22,
            "command",
            &ClientCommand::ResumeSession {
                session_id: "resume-session".into(),
                cwd: None,
            },
        ),
    )
    .await;
    enqueue_context(&mut socket, "resume-context").await;
    let resumed = wait_for_owner_replacement(&rpc, new.id(), "resume-session").await;
    assert_context_binding(&mut socket, "resume-context", "resume-session", &resumed).await;
    send(
        &mut socket,
        wire_request(
            23,
            "command",
            &ClientCommand::ResumeSession {
                session_id: "missing-session".into(),
                cwd: None,
            },
        ),
    )
    .await;
    enqueue_context(&mut socket, "failed-resume-context").await;
    let recovered = wait_for_owner_replacement(&rpc, resumed.id(), "resume-session").await;
    assert_context_binding(
        &mut socket,
        "failed-resume-context",
        "resume-session",
        &recovered,
    )
    .await;
    let prepared = router.prepared.lock().unwrap();
    assert_eq!(prepared.len(), 3);
    assert_eq!(
        prepared[0],
        (
            MediaOwner::Session {
                session_id: "new-session".into()
            },
            new.id().clone()
        )
    );
    assert_eq!(
        prepared[1],
        (
            MediaOwner::Session {
                session_id: "resume-session".into()
            },
            resumed.id().clone()
        )
    );
    assert_eq!(
        prepared[2],
        (
            MediaOwner::Session {
                session_id: "resume-session".into()
            },
            recovered.id().clone()
        )
    );
    drop(prepared);
    socket.close(None).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        recovered.wait_for_admission_closed(),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            recovered.wait_for_drain_completion()
        )
        .await
        .unwrap(),
        MediaAttachmentPhase::Revoked
    );
    endpoint.shutdown().await;
}
// An older client registration is received while the priority lane is busy.
// It must never be installed into a later session's attachment.
struct BlockedPriorityRouter {
    session_router: TransitionRouter,
    input_started: tokio::sync::Notify,
    release_input: CancellationToken,
}

#[async_trait::async_trait]
impl crate::router::CommandRouter for BlockedPriorityRouter {
    async fn route(&self, command: ClientCommand, sink: Arc<dyn client::adapter::ClientEventSink>) {
        if matches!(&command, ClientCommand::RealtimeAudioInput { .. }) {
            self.input_started.notify_one();
            self.release_input.cancelled().await;
        } else {
            crate::router::CommandRouter::route(&self.session_router, command, sink).await;
        }
    }

    async fn audio_session_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        crate::router::CommandRouter::audio_session_context(&self.session_router).await
    }

    async fn prepare_media_attachment(
        &self,
        attachment: &MediaAttachmentHandle,
        owner: &MediaOwner,
    ) -> Result<(), String> {
        crate::router::CommandRouter::prepare_media_attachment(
            &self.session_router,
            attachment,
            owner,
        )
        .await
    }
}

#[tokio::test]
async fn socket_queued_old_route_cannot_register_in_new_session() {
    let resources = Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default());
    let service: Arc<dyn MediaService> = media_runtime::MediaRuntimeBuilder::new(resources.clone())
        .service_epoch(7)
        .build();
    let old_owner = MediaOwner::Session {
        session_id: "old-session".into(),
    };
    let mut rpc = MediaServiceRpc::new(service, old_owner).unwrap();
    let connector = remote_control_client::host_media::RemoteMediaConnector::new(
        Arc::new(crate::media_bridge::DesktopRemoteMediaConfigProvider::from_process_environment()),
        resources,
    );
    let connection = BridgeConnection::new();
    let (connection_channel, connection_responder) = connection.media_connection_channel();
    let (control_channel, control_responder) = connection.media_control_channel();
    let rpc_mut = Arc::get_mut(&mut rpc).unwrap();
    rpc_mut.remote_connector = Some(connector.clone());
    rpc_mut.media_connection_channel = Some(connection_channel);
    rpc_mut.media_control_channel = Some(control_channel);
    let session = Arc::new(tokio::sync::Mutex::new("old-session".to_owned()));
    let router = Arc::new(BlockedPriorityRouter {
        session_router: TransitionRouter {
            session: session.clone(),
            rpc: rpc.clone(),
            prepared: std::sync::Mutex::new(Vec::new()),
        },
        input_started: tokio::sync::Notify::new(),
        release_input: CancellationToken::new(),
    });
    let gate = Arc::new(client::adapter::AdapterPermissionGate::new(
        connection.permission_sink(),
    ));
    let connection = Arc::new(
        connection
            .bind(gate, Arc::new(TransitionDriver { session }))
            .bind_router(router.clone())
            .bind_media_connection(connection_responder)
            .bind_media_control(control_responder)
            .bind_media_service(rpc.clone()),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("teardown-test-token".into());
    let mut socket = connect(&endpoint).await;
    let old = rpc.current_attachment().unwrap();
    send(
        &mut socket,
        wire_request(
            20,
            "command",
            &ClientCommand::RealtimeAudioInput {
                input_json: "{}".into(),
            },
        ),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), router.input_started.notified())
        .await
        .expect("priority input callback must be parked");
    send(
        &mut socket,
        wire_request(
            21,
            "command",
            &ClientCommand::MediaServiceRequest {
                request: dto::MediaServiceRequestDto {
                    request_id: "queued-before-new-session".into(),
                    action: dto::MediaServiceActionDto::RegisterConnection {
                        connection_id: "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61".into(),
                        label: "old session route".into(),
                        supported_modes: vec![dto::MediaSessionModeDto::Audio],
                        source_kinds: vec![dto::MediaSourceKindDto::Microphone],
                    },
                },
            },
        ),
    )
    .await;
    send(
        &mut socket,
        wire_request(
            22,
            "command",
            &ClientCommand::NewSession {
                cwd: None,
                model: None,
            },
        ),
    )
    .await;
    let replacement = wait_for_owner_replacement(&rpc, old.id(), "new-session").await;
    router.release_input.cancel();
    match next_frame(&mut socket).await {
        Frame::Event(ClientEvent::MediaServiceResponse { response }) => {
            assert_eq!(response.request_id, "queued-before-new-session");
            assert!(
                matches!(
                    response.result,
                    Some(dto::MediaServiceResultDto::ConnectionUpdated)
                ) || matches!(
                    response.error,
                    Some(dto::MediaErrorDto {
                        kind: dto::MediaErrorKindDto::Unavailable,
                        ..
                    })
                ),
                "old registration must either finish under its original owner or be rejected"
            );
        }
        other => panic!("expected the old registration response, got {other:?}"),
    }
    let new_owner = MediaOwner::Session {
        session_id: "new-session".into(),
    }
    .in_attachment(replacement.id().clone());
    assert!(
        connector.supported_modes(&new_owner).await.is_empty(),
        "a queued old registration must not install a route in the replacement attachment"
    );
    socket.close(None).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        replacement.wait_for_admission_closed(),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            replacement.wait_for_drain_completion()
        )
        .await
        .unwrap(),
        MediaAttachmentPhase::Revoked
    );
    endpoint.shutdown().await;
}

struct BlockedSessionRouter {
    session_router: TransitionRouter,
    session_started: tokio::sync::Notify,
    release_session: CancellationToken,
}

#[async_trait::async_trait]
impl crate::router::CommandRouter for BlockedSessionRouter {
    async fn route(&self, command: ClientCommand, sink: Arc<dyn client::adapter::ClientEventSink>) {
        self.session_started.notify_one();
        self.release_session.cancelled().await;
        crate::router::CommandRouter::route(&self.session_router, command, sink).await;
    }

    async fn prepare_media_attachment(
        &self,
        attachment: &MediaAttachmentHandle,
        owner: &MediaOwner,
    ) -> Result<(), String> {
        crate::router::CommandRouter::prepare_media_attachment(
            &self.session_router,
            attachment,
            owner,
        )
        .await
    }
}

#[tokio::test]
async fn socket_audio_receipt_bypasses_a_blocked_session_command() {
    let service: Arc<dyn MediaService> = media_runtime::MediaRuntimeBuilder::new(Arc::new(
        audio_provider::media_store::BoundedMediaResourceStore::default(),
    ))
    .service_epoch(7)
    .build();
    let old_owner = MediaOwner::Session {
        session_id: "old-session".into(),
    };
    let rpc = MediaServiceRpc::new(service, old_owner.clone()).unwrap();
    let session = Arc::new(tokio::sync::Mutex::new("old-session".to_owned()));
    let router = Arc::new(BlockedSessionRouter {
        session_router: TransitionRouter {
            session: session.clone(),
            rpc: rpc.clone(),
            prepared: std::sync::Mutex::new(Vec::new()),
        },
        session_started: tokio::sync::Notify::new(),
        release_session: CancellationToken::new(),
    });
    let connection = BridgeConnection::new();
    let (audio, responder) = new_audio_bridge(connection.audio_sink());
    let gate = Arc::new(client::adapter::AdapterPermissionGate::new(
        connection.permission_sink(),
    ));
    let connection = Arc::new(
        connection
            .bind(gate, Arc::new(TransitionDriver { session }))
            .bind_router(router.clone())
            .bind_audio(responder)
            .bind_media_service(rpc.clone()),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("teardown-test-token".into());
    let mut socket = connect(&endpoint).await;
    let old = rpc.current_attachment().unwrap();
    let status = tokio::spawn(async move {
        audio
            .execute(
                AudioOperationContext::new(AudioOperationId::new(19, 7), old_owner),
                AudioOperation::Status { handle: None },
            )
            .await
    });
    let request = match next_frame(&mut socket).await {
        Frame::Event(ClientEvent::AudioRequest { request }) => request,
        other => panic!("expected an audio status request, got {other:?}"),
    };
    send(
        &mut socket,
        wire_request(
            21,
            "command",
            &ClientCommand::NewSession {
                cwd: None,
                model: None,
            },
        ),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), router.session_started.notified())
        .await
        .expect("ordinary session command must be parked");
    send(
        &mut socket,
        wire_request(
            22,
            "command",
            &ClientCommand::AudioResponse {
                identity: request.identity,
                result: AudioOperationResultDto::Status {
                    status: client::protocol::audio::AudioStatusDto {
                        recording: false,
                        playing: false,
                    },
                },
            },
        ),
    )
    .await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), status)
            .await
            .expect("audio receipt must bypass the parked session command")
            .unwrap(),
        Ok(AudioOperationSuccess::Status { .. })
    ));
    router.release_session.cancel();
    let replacement = wait_for_owner_replacement(&rpc, old.id(), "new-session").await;
    socket.close(None).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        replacement.wait_for_admission_closed(),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            replacement.wait_for_drain_completion()
        )
        .await
        .unwrap(),
        MediaAttachmentPhase::Revoked
    );
    endpoint.shutdown().await;
}
