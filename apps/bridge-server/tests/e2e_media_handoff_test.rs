//! Desktop same-turn realtime media continuation over the authenticated bridge.
//!
//! The ordinary SDK stream invokes the real `media_session_start` builtin. The
//! production turn driver then waits on the same connection-owned handoff queue
//! while the fake native realtime transport dispatches another tool through the
//! same orchestrator and permission owner.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::driver::OrchestratorTurnDriver;
use bridge_server::server::BridgeConnection;
use client::adapter::{AdapterOutputStream, AdapterPermissionGate};
use client::protocol::commands::ClientCommand;
use client::protocol::events::{ClientEvent, TurnOutcomeDto};
use client::protocol::permission::{PermissionKindDto, PermissionOwnerDto, PermissionResponseDto};
use futures_util::{stream, SinkExt, StreamExt};
use harness_runtime::desktop::{desktop_tool_registry, DesktopMediaAgentHandoffQueue};
use lingxi_core::host::{OrchestratorHandle, OutputStream};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use lingxi_llm_client::realtime::{
    RealtimeCapabilities, RealtimeClose, RealtimeCodec, RealtimeConnectRequest, RealtimeConnection,
    RealtimeError, RealtimeEvent, RealtimeFrame, RealtimeInput, RealtimeLimits, RealtimeSession,
    RealtimeSink, RealtimeTransport,
};
use media_api::{
    MediaAudioChannel, MediaCapabilities, MediaContent, MediaError, MediaErrorKind, MediaKind,
    MediaOperation, MediaOperationContext, MediaOperationId, MediaOperationKind,
    MediaOperationReadiness, MediaOwner, MediaReadinessState, MediaRef, MediaRenderReceipt,
    MediaResult, MediaService, MediaSessionMode, MediaSessionRef, MediaSessionRequest,
    MediaSessionState, MediaSessionStatus,
};
use orchestrator::test_support::{
    content_block_start_tool_use, content_block_stop, input_json_delta, message_delta_stop,
    message_start, message_stop, MockApiClient, MockStreamingApiClient, StaticMemoryProvider,
};
use orchestrator::{
    scripted, ConversationOrchestrator, OrchestratorConfig, RealtimeAgentInput,
    RealtimeAgentLimits, RealtimeAgentSessionIo,
};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;
use tool_api::test_support::shell_test_ctx;
use tool_fixture::AlwaysOkTool;

mod tool_fixture;

const TEST_TOKEN: &str = "media-handoff-e2e-token-000000000";
const TURN_ID: u64 = 71;
const SESSION_REF: &str = "session-media-handoff";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct FakeAudioChannel;

#[async_trait]
impl MediaAudioChannel for FakeAudioChannel {
    async fn read(&self) -> Result<Option<media_api::MediaFrame>, MediaError> {
        Ok(None)
    }

    async fn write(
        &self,
        frame: media_api::MediaPcmFrame,
    ) -> Result<MediaRenderReceipt, MediaError> {
        let sample_frames = frame.pcm16le.len() / (usize::from(frame.channels) * 2);
        Ok(MediaRenderReceipt {
            accepted_sample_frames: u32::try_from(sample_frames).unwrap_or(u32::MAX),
            played_sample_frames: 0,
            rendered_at_unix_ms: None,
        })
    }

    async fn flush(&self) -> Result<MediaRenderReceipt, MediaError> {
        Ok(empty_receipt())
    }

    async fn interrupt(&self) -> Result<MediaRenderReceipt, MediaError> {
        Ok(empty_receipt())
    }
}

fn empty_receipt() -> MediaRenderReceipt {
    MediaRenderReceipt {
        accepted_sample_frames: 0,
        played_sample_frames: 0,
        rendered_at_unix_ms: None,
    }
}

struct FakeMediaService {
    audio: Arc<dyn MediaAudioChannel>,
    started: Mutex<Option<MediaOperationContext>>,
    started_request: Mutex<Option<MediaSessionRequest>>,
    audio_requests: Mutex<Vec<(MediaOwner, MediaSessionRef)>>,
    stopped_contexts: Mutex<Vec<MediaOperationContext>>,
    stopped: AtomicUsize,
}

impl FakeMediaService {
    fn new() -> Self {
        Self {
            audio: Arc::new(FakeAudioChannel),
            started: Mutex::new(None),
            started_request: Mutex::new(None),
            audio_requests: Mutex::new(Vec::new()),
            stopped_contexts: Mutex::new(Vec::new()),
            stopped: AtomicUsize::new(0),
        }
    }

    fn capability_snapshot() -> MediaCapabilities {
        MediaCapabilities {
            service_epoch: 9,
            support_revision: 1,
            supported_operations: vec![MediaOperationKind::SessionStart, MediaOperationKind::Stop],
            supported_media_kinds: vec![MediaKind::Audio],
            readiness: vec![
                MediaOperationReadiness {
                    operation: MediaOperationKind::SessionStart,
                    state: MediaReadinessState::Ready,
                },
                MediaOperationReadiness {
                    operation: MediaOperationKind::Stop,
                    state: MediaReadinessState::Ready,
                },
            ],
            max_payload_bytes: 1024 * 1024,
        }
    }
}

#[async_trait]
impl MediaService for FakeMediaService {
    fn capabilities(&self) -> MediaCapabilities {
        Self::capability_snapshot()
    }

    async fn execute(
        &self,
        context: MediaOperationContext,
        operation: MediaOperation,
    ) -> Result<MediaResult, MediaError> {
        match operation {
            MediaOperation::SessionStart { request } => {
                *self.started.lock().unwrap() = Some(context);
                *self.started_request.lock().unwrap() = Some(request);
                Ok(MediaResult::SessionStarted {
                    session: MediaSessionRef(SESSION_REF.into()),
                    status: MediaSessionStatus {
                        state: MediaSessionState::Ready,
                        audio_enabled: true,
                        video_enabled: false,
                    },
                })
            }
            MediaOperation::Stop { target } => {
                self.stopped_contexts.lock().unwrap().push(context);
                self.stopped.fetch_add(1, Ordering::SeqCst);
                Ok(MediaResult::Stopped { resource: target })
            }
            other => Err(MediaError::new(
                MediaErrorKind::Unsupported,
                format!("unexpected fixture operation: {:?}", other.kind()),
            )),
        }
    }

    async fn session_audio(
        &self,
        context: MediaOperationContext,
        session: MediaSessionRef,
    ) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        self.audio_requests
            .lock()
            .unwrap()
            .push((context.owner, session));
        Ok(self.audio.clone())
    }

    async fn read_media(
        &self,
        _context: MediaOperationContext,
        _media: MediaRef,
    ) -> Result<MediaContent, MediaError> {
        Err(MediaError::new(
            MediaErrorKind::NotFound,
            "fixture has no assets",
        ))
    }

    async fn cancel(&self, _identity: MediaOperationId) -> Result<(), MediaError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum OpenBehavior {
    Realtime,
    Fail,
}

struct AttachmentState {
    open_started: Notify,
    open_ready: Notify,
    open_count: AtomicUsize,
    opened_request: Mutex<Option<tool_api::MediaAgentHandoffRequest>>,
    prepared_session_id: Mutex<Option<String>>,
    audio_channel_ptr: AtomicUsize,
    peer: Mutex<Option<mpsc::UnboundedSender<Result<RealtimeFrame, RealtimeError>>>>,
    input_tx: Mutex<Option<mpsc::Sender<RealtimeAgentInput>>>,
    output_rx: Mutex<Option<mpsc::Receiver<RealtimeEvent>>>,
    realtime_driver: Mutex<Option<tokio::task::JoinHandle<Result<(), RealtimeError>>>>,
}

impl Default for AttachmentState {
    fn default() -> Self {
        Self {
            open_started: Notify::new(),
            open_ready: Notify::new(),
            open_count: AtomicUsize::new(0),
            opened_request: Mutex::new(None),
            prepared_session_id: Mutex::new(None),
            audio_channel_ptr: AtomicUsize::new(0),
            peer: Mutex::new(None),
            input_tx: Mutex::new(None),
            output_rx: Mutex::new(None),
            realtime_driver: Mutex::new(None),
        }
    }
}

struct FakeAttachment {
    behavior: OpenBehavior,
    state: Arc<AttachmentState>,
}

impl FakeAttachment {
    fn new(behavior: OpenBehavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            state: Arc::new(AttachmentState::default()),
        })
    }
}

#[async_trait]
impl orchestrator::MediaConversationAttachment for FakeAttachment {
    fn supported_modes(&self) -> Vec<MediaSessionMode> {
        vec![MediaSessionMode::Audio]
    }

    fn supports_mode(
        &self,
        _context: &tool_api::MediaAgentCapabilityContext,
        mode: MediaSessionMode,
    ) -> bool {
        mode == MediaSessionMode::Audio
    }

    async fn open(
        &self,
        request: tool_api::MediaAgentHandoffRequest,
        prepared: orchestrator::RealtimeAgentContext,
        audio: Arc<dyn MediaAudioChannel>,
    ) -> Result<RealtimeAgentSessionIo, String> {
        self.state.open_count.fetch_add(1, Ordering::SeqCst);
        *self.state.opened_request.lock().unwrap() = Some(request);
        *self.state.prepared_session_id.lock().unwrap() = Some(prepared.session_id);
        self.state
            .audio_channel_ptr
            .store(Arc::as_ptr(&audio) as *const () as usize, Ordering::SeqCst);
        self.state.open_started.notify_one();
        if matches!(self.behavior, OpenBehavior::Fail) {
            self.state.open_ready.notify_one();
            return Err("native realtime fixture refused to open".into());
        }

        let (peer, incoming) = mpsc::unbounded_channel();
        let transport = TestRealtimeTransport {
            incoming: Mutex::new(Some(incoming)),
        };
        let (session, driver) = RealtimeSession::connect(
            &transport,
            RealtimeConnectRequest {
                endpoint: "wss://fixture.invalid/realtime".into(),
                headers: Vec::new(),
                max_frame_bytes: 4096,
            },
            Arc::new(TestRealtimeCodec),
            RealtimeLimits::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let (control, events) = session.into_parts();
        let realtime_driver = tokio::spawn(driver.run());
        let (input_tx, inputs) = mpsc::channel(8);
        let (output, output_rx) = mpsc::channel(8);
        *self.state.peer.lock().unwrap() = Some(peer.clone());
        *self.state.input_tx.lock().unwrap() = Some(input_tx);
        *self.state.output_rx.lock().unwrap() = Some(output_rx);
        *self.state.realtime_driver.lock().unwrap() = Some(realtime_driver);
        peer.send(Ok(RealtimeFrame::text("ready")))
            .map_err(|error| error.to_string())?;
        self.state.open_ready.notify_one();
        Ok(RealtimeAgentSessionIo {
            control,
            events,
            inputs,
            output,
            limits: RealtimeAgentLimits {
                idle_timeout: Duration::from_secs(30),
                max_duration: Duration::from_secs(60),
                ..RealtimeAgentLimits::default()
            },
            image_input_supported: false,
        })
    }
}

struct TestRealtimeTransport {
    incoming: Mutex<Option<mpsc::UnboundedReceiver<Result<RealtimeFrame, RealtimeError>>>>,
}

#[async_trait]
impl RealtimeTransport for TestRealtimeTransport {
    async fn connect(
        &self,
        _request: RealtimeConnectRequest,
    ) -> Result<RealtimeConnection, RealtimeError> {
        let receiver = self
            .incoming
            .lock()
            .unwrap()
            .take()
            .ok_or(RealtimeError::Closed)?;
        let inbound = stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|frame| (frame, receiver))
        })
        .boxed();
        Ok(RealtimeConnection {
            outbound: Box::new(TestRealtimeSink),
            inbound,
        })
    }
}

struct TestRealtimeSink;

#[async_trait]
impl RealtimeSink for TestRealtimeSink {
    async fn send(&mut self, _frame: RealtimeFrame) -> Result<(), RealtimeError> {
        Ok(())
    }

    async fn ping(&mut self, _payload: bytes::Bytes) -> Result<(), RealtimeError> {
        Err(RealtimeError::InvalidInput {
            message: "fixture transport has no ping support".into(),
        })
    }

    fn abort(&mut self) {}

    async fn close(&mut self, _close: RealtimeClose) -> Result<(), RealtimeError> {
        Ok(())
    }
}

struct TestRealtimeCodec;

impl RealtimeCodec for TestRealtimeCodec {
    fn capabilities(&self) -> RealtimeCapabilities {
        RealtimeCapabilities {
            history_import: true,
            tools: true,
            input_transcription: true,
            output_transcription: true,
            interruption: true,
            ..RealtimeCapabilities::default()
        }
    }

    fn encode(&self, input: &RealtimeInput) -> Result<Vec<RealtimeFrame>, RealtimeError> {
        Ok(vec![RealtimeFrame::text(format!("{input:?}"))])
    }

    fn decode(&self, frame: RealtimeFrame) -> Result<Vec<RealtimeEvent>, RealtimeError> {
        let RealtimeFrame::Text(bytes) = frame else {
            return Err(RealtimeError::InvalidInput {
                message: "fixture accepts text frames only".into(),
            });
        };
        match std::str::from_utf8(&bytes) {
            Ok("ready") => Ok(vec![RealtimeEvent::SessionReady]),
            Ok("permission_tool") => Ok(vec![RealtimeEvent::ToolCall {
                call_id: "realtime-permission-call".into(),
                name: "AlwaysOk".into(),
                arguments: serde_json::json!({}),
            }]),
            Ok(_) => Ok(Vec::new()),
            Err(error) => Err(RealtimeError::InvalidInput {
                message: error.to_string(),
            }),
        }
    }
}

async fn connect(port: u16) -> Socket {
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let request = http::Request::builder()
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
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("WebSocket upgrade");
    assert_eq!(response.status(), 101);
    let hello = Frame::Request(BridgeRequest {
        trace_context: None,
        id: 0,
        method: "hello".into(),
        params: bridge_source(&ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "media-handoff-test".into(),
            capabilities: Capabilities::default(),
        }),
    });
    socket
        .send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .expect("send hello");
    match next_frame(&mut socket).await {
        Frame::Response(response) => assert!(response.error.is_none()),
        other => panic!("expected hello response, got {other:?}"),
    }
    socket
}

async fn next_frame(socket: &mut Socket) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("bridge frame timeout")
            .expect("socket remains open")
            .expect("valid websocket message");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).expect("decode bridge frame");
        }
    }
}

async fn next_frame_before(socket: &mut Socket, timeout: Duration) -> Option<Frame> {
    tokio::time::timeout(timeout, next_frame(socket)).await.ok()
}

async fn failure_frames_before(socket: &mut Socket, timeout: Duration) -> Vec<Frame> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut frames = Vec::new();
    while frames.len() < 16 {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            break;
        };
        let Ok(frame) = tokio::time::timeout(remaining, next_frame(socket)).await else {
            break;
        };
        let terminal = matches!(&frame, Frame::Event(ClientEvent::TurnEnded { .. }));
        frames.push(frame);
        if terminal {
            break;
        }
    }
    frames
}

async fn send_command(socket: &mut Socket, command: &ClientCommand) {
    static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
    let frame = Frame::Request(BridgeRequest {
        trace_context: None,
        id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
        method: "submit".into(),
        params: bridge_source(command),
    });
    socket
        .send(Message::Text(serde_json::to_string(&frame).unwrap()))
        .await
        .expect("send client command");
}

fn bridge_source<T: serde::Serialize>(value: &T) -> Utf16JsonProjection {
    let json = serde_json::to_string(value).expect("serialize bridge payload");
    Utf16JsonProjection::parse(&json).expect("bridge payload projection")
}

async fn approve(socket: &mut Socket, request_id: u64) {
    send_command(
        socket,
        &ClientCommand::ApprovePermission {
            request_id,
            response: PermissionResponseDto::AllowOnce,
        },
    )
    .await;
}

fn owner_of(request: &client::protocol::permission::PermissionRequest) -> PermissionOwnerDto {
    request
        .owner
        .clone()
        .expect("permission request retains its active turn owner")
}

fn build_orchestrator(
    service: Arc<FakeMediaService>,
    handoffs: Arc<DesktopMediaAgentHandoffQueue>,
    output: Arc<dyn OutputStream>,
    gate: Arc<AdapterPermissionGate>,
) -> Arc<ConversationOrchestrator> {
    let mut context = shell_test_ctx(mobile_linux_api::ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    });
    context.media = Some(service);
    context.media_agent_handoff = Some(handoffs);
    let mut tools = desktop_tool_registry(context, None, None);
    tools.register_builtin(Arc::new(AlwaysOkTool));
    let tools = Arc::new(tools);
    let first = scripted![
        message_start("ordinary-permission", "test-model"),
        content_block_start_tool_use(0, lingxi_core::types::ToolUseId::new(), "AlwaysOk"),
        input_json_delta(0, "{}"),
        content_block_stop(0),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    let second = scripted![
        message_start("media-start", "test-model"),
        content_block_start_tool_use(
            0,
            lingxi_core::types::ToolUseId::new(),
            "media_session_start"
        ),
        input_json_delta(
            0,
            r#"{"intent":"conversation","mode":"audio","audio_source_ref":"mic-1"}"#
        ),
        content_block_stop(0),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    let third = scripted![
        message_start("ordinary-finished", "test-model"),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![
        first, second, third,
    ]));
    ConversationOrchestrator::into_shared(ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig {
            interactive_permissions: true,
            ..OrchestratorConfig::default()
        },
        Arc::new(MockApiClient::new(Vec::new())),
        streaming,
        tools,
        orchestrator::test_support::noop_hook_executor(),
        gate as Arc<dyn permission::gate::PermissionGate>,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ))
}

async fn build_rig(
    behavior: OpenBehavior,
    loop_scheduler: Option<Arc<TestLoopScheduler>>,
) -> (
    McpEndpoint,
    Socket,
    Arc<FakeMediaService>,
    Arc<AttachmentState>,
) {
    let connection = BridgeConnection::new();
    let service = Arc::new(FakeMediaService::new());
    let attachment = FakeAttachment::new(behavior);
    let handoffs = Arc::new(DesktopMediaAgentHandoffQueue::default());
    handoffs.set_attachment(Some(attachment.clone()));
    let sink = connection.event_sink();
    let output_stream =
        AdapterOutputStream::new(sink.clone()).with_turn_terminal_deferrer(handoffs.clone());
    let output: Arc<dyn OutputStream> = Arc::new(output_stream.clone());
    let gate = Arc::new(
        AdapterPermissionGate::new(connection.permission_sink())
            .with_timeout(Duration::from_secs(5))
            .with_event_sink(sink.clone()),
    );
    let orchestrator = build_orchestrator(service.clone(), handoffs.clone(), output, gate.clone());
    let session_id = orchestrator
        .current_session_id()
        .await
        .as_uuid()
        .to_string();
    orchestrator.session().lock().await.model_profile = Some("test-profile".into());
    gate.set_session_id(Some(session_id));
    let driver = OrchestratorTurnDriver::with_error_sink(orchestrator, sink)
        .with_media_handoff(handoffs, Some(service.clone()))
        .with_message_output(output_stream);
    let driver = if let Some(scheduler) = loop_scheduler {
        driver.with_wakeup_scheduler(scheduler)
    } else {
        driver
    };
    let driver = Arc::new(driver);
    let endpoint =
        McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection.bind(gate, driver)))
            .await
            .expect("bridge endpoint starts");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let socket = connect(endpoint.port()).await;
    (endpoint, socket, service, attachment.state.clone())
}

struct TestLoopScheduler {
    runtime: Arc<tool_cron::LoopRuntime>,
    scheduled: AtomicUsize,
    cancel_calls: AtomicUsize,
}

impl TestLoopScheduler {
    fn for_in_flight_tick() -> Arc<Self> {
        let runtime = Arc::new(tool_cron::LoopRuntime::default());
        runtime.begin_tick(Utf16JsonProjection::from("<<autonomous-loop-dynamic>>"));
        Arc::new(Self {
            runtime,
            scheduled: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl tool_cron::WakeupScheduler for TestLoopScheduler {
    async fn schedule(&self, _delay: Duration, _prompt: Utf16JsonProjection, _reason: String) {
        self.scheduled.fetch_add(1, Ordering::SeqCst);
    }

    async fn cancel_pending(&self) -> Vec<Utf16JsonProjection> {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }

    fn loop_runtime(&self) -> Option<Arc<tool_cron::LoopRuntime>> {
        Some(self.runtime.clone())
    }
}

async fn submit_prompt(socket: &mut Socket) {
    send_command(
        socket,
        &ClientCommand::SendPrompt {
            session_id: None,
            text: "start a media conversation".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: Some(TURN_ID),
            visualization_context: None,
        },
    )
    .await;
}

async fn approve_ordinary_tools(socket: &mut Socket) -> (PermissionOwnerDto, usize) {
    let mut owner = None;
    let mut turn_starts = 0;
    let mut media_tool_finished = false;
    while !media_tool_finished {
        match next_frame(socket).await {
            Frame::PermissionRequest(request) => {
                match &request.kind {
                    PermissionKindDto::ToolUseConfirm { tool_name, .. } => assert!(
                        tool_name == "AlwaysOk" || tool_name == "media_session_start",
                        "unexpected ordinary permission tool: {tool_name}"
                    ),
                    other => panic!("unexpected permission request: {other:?}"),
                }
                let request_owner = owner_of(&request);
                if let Some(previous) = owner.as_ref() {
                    assert_eq!(previous, &request_owner, "ordinary tools share one owner");
                } else {
                    owner = Some(request_owner);
                }
                approve(socket, request.request_id).await;
            }
            Frame::Event(ClientEvent::TurnStarted { turn_id }) => {
                assert_eq!(turn_id, Some(TURN_ID));
                turn_starts += 1;
            }
            Frame::Event(ClientEvent::ToolUseResult { tool, is_error, .. })
                if tool == "media_session_start" =>
            {
                assert!(!is_error, "the media start tool must succeed");
                media_tool_finished = true;
            }
            Frame::Event(_) | Frame::Response(_) => {}
            other => panic!("unexpected frame during ordinary media turn: {other:?}"),
        }
    }
    assert_eq!(turn_starts, 1, "the ordinary turn starts exactly once");
    (
        owner.expect("the ordinary permission probe must have prompted"),
        turn_starts,
    )
}

async fn next_permission_request(
    socket: &mut Socket,
) -> client::protocol::permission::PermissionRequest {
    loop {
        match next_frame(socket).await {
            Frame::PermissionRequest(request) => return request,
            Frame::Event(ClientEvent::TurnEnded { .. }) => {
                panic!("turn terminal escaped while realtime was active")
            }
            Frame::Event(_) | Frame::Response(_) => {}
            other => panic!("unexpected frame while waiting for realtime permission: {other:?}"),
        }
    }
}

async fn wait_for_terminal(socket: &mut Socket) -> (usize, TurnOutcomeDto) {
    let mut starts = 0;
    let mut terminals = Vec::new();
    while terminals.is_empty() {
        match next_frame(socket).await {
            Frame::Event(ClientEvent::TurnStarted { .. }) => starts += 1,
            Frame::Event(ClientEvent::TurnEnded { outcome, .. }) => terminals.push(outcome),
            Frame::PermissionRequest(request) => {
                panic!("unexpected parked permission request: {request:?}")
            }
            Frame::Event(_) | Frame::Response(_) => {}
            other => panic!("unexpected frame before terminal: {other:?}"),
        }
    }
    assert_eq!(terminals.len(), 1);
    (starts, terminals.remove(0))
}

#[tokio::test]
async fn conversation_continues_in_the_same_turn_until_realtime_cancel() {
    let (endpoint, mut socket, service, attachment) = build_rig(OpenBehavior::Realtime, None).await;
    submit_prompt(&mut socket).await;
    let (ordinary_owner, ordinary_starts) = approve_ordinary_tools(&mut socket).await;
    assert!(matches!(
        service.started_request.lock().unwrap().as_ref(),
        Some(request)
            if request.intent == media_api::MediaSessionIntent::Conversation
                && request.mode == MediaSessionMode::Audio
    ));

    if tokio::time::timeout(Duration::from_secs(5), attachment.open_started.notified())
        .await
        .is_err()
    {
        let failure_frames = failure_frames_before(&mut socket, Duration::from_millis(500)).await;
        panic!(
            "realtime attachment did not open after the committed media tool result; stopped={}, audio_requests={}, failure_frames={failure_frames:?}",
            service.stopped.load(Ordering::SeqCst),
            service.audio_requests.lock().unwrap().len(),
        );
    }
    tokio::time::timeout(Duration::from_secs(5), attachment.open_ready.notified())
        .await
        .expect("realtime IO becomes ready");
    assert_eq!(attachment.open_count.load(Ordering::SeqCst), 1);
    let started = service
        .started
        .lock()
        .unwrap()
        .clone()
        .expect("session started");
    assert!(started.identity.generation > 0);
    assert_eq!(started.identity.service_epoch, 9);
    assert_eq!(
        started.owner,
        MediaOwner::Session {
            session_id: ordinary_owner.session_id.clone().expect("session owner")
        }
    );
    let opened = attachment
        .opened_request
        .lock()
        .unwrap()
        .clone()
        .expect("attachment receives the successful session start");
    assert_eq!(opened.operation_id, started.identity);
    assert_eq!(opened.session.0 .0, SESSION_REF);
    assert_eq!(opened.mode, MediaSessionMode::Audio);
    assert_eq!(
        *attachment.prepared_session_id.lock().unwrap(),
        Some(ordinary_owner.session_id.clone().expect("session owner"))
    );
    let audio_requests = service.audio_requests.lock().unwrap();
    assert_eq!(audio_requests.len(), 1);
    assert_eq!(audio_requests[0].1 .0 .0, SESSION_REF);
    assert_eq!(audio_requests[0].0, started.owner);
    let expected_audio_ptr = Arc::as_ptr(&service.audio) as *const () as usize;
    assert_eq!(
        attachment.audio_channel_ptr.load(Ordering::SeqCst),
        expected_audio_ptr,
        "handoff receives the service's already-open audio channel"
    );
    drop(audio_requests);
    let _realtime_inputs = attachment
        .input_tx
        .lock()
        .unwrap()
        .take()
        .expect("realtime input pump stays connected");
    let _realtime_output = attachment
        .output_rx
        .lock()
        .unwrap()
        .take()
        .expect("realtime output pump stays connected");

    let peer = attachment
        .peer
        .lock()
        .unwrap()
        .clone()
        .expect("live realtime peer");
    peer.send(Ok(RealtimeFrame::text("permission_tool")))
        .expect("deliver realtime tool call");
    let realtime_permission = next_permission_request(&mut socket).await;
    assert!(matches!(
        &realtime_permission.kind,
        PermissionKindDto::ToolUseConfirm { tool_name, .. } if tool_name == "AlwaysOk"
    ));
    let realtime_owner = owner_of(&realtime_permission);
    assert_eq!(
        realtime_owner, ordinary_owner,
        "same ActiveTurn permission owner"
    );
    approve(&mut socket, realtime_permission.request_id).await;
    loop {
        match next_frame(&mut socket).await {
            Frame::Event(ClientEvent::ToolUseResult { tool, is_error, .. })
                if tool == "AlwaysOk" =>
            {
                assert!(!is_error);
                break;
            }
            Frame::Event(ClientEvent::TurnEnded { .. }) => {
                panic!("ordinary turn ended before realtime was cancelled")
            }
            Frame::Event(_) | Frame::Response(_) | Frame::PermissionRequest(_) => {}
            other => panic!("unexpected realtime frame: {other:?}"),
        }
    }
    assert!(
        next_frame_before(&mut socket, Duration::from_millis(50))
            .await
            .is_none(),
        "the active turn stays open after the ordinary SDK stream ends"
    );

    send_command(
        &mut socket,
        &ClientCommand::Cancel {
            session_id: None,
            turn_id: Some(TURN_ID),
        },
    )
    .await;
    let (starts, outcome) = wait_for_terminal(&mut socket).await;
    assert_eq!(
        ordinary_starts + starts,
        1,
        "handoff never announces a second turn"
    );
    assert_eq!(outcome, TurnOutcomeDto::Cancelled);
    assert_eq!(service.stopped.load(Ordering::SeqCst), 1);
    let stopped_contexts = service.stopped_contexts.lock().unwrap();
    assert_eq!(stopped_contexts.len(), 1);
    assert_eq!(
        stopped_contexts[0].owner, started.owner,
        "cancellation cleanup must use the session-start owner"
    );
    assert_eq!(
        stopped_contexts[0].identity.service_epoch,
        started.identity.service_epoch
    );
    drop(stopped_contexts);
    let driver = attachment
        .realtime_driver
        .lock()
        .unwrap()
        .take()
        .expect("realtime transport driver exists");
    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("realtime transport closes after cancellation")
        .expect("driver task joins")
        .expect("transport closes cleanly");
    endpoint.shutdown().await;
}

#[tokio::test]
async fn realtime_open_failure_still_releases_the_deferred_terminal() {
    let (endpoint, mut socket, service, attachment) = build_rig(OpenBehavior::Fail, None).await;
    submit_prompt(&mut socket).await;
    let (_ordinary_owner, ordinary_starts) = approve_ordinary_tools(&mut socket).await;
    assert!(matches!(
        service.started_request.lock().unwrap().as_ref(),
        Some(request)
            if request.intent == media_api::MediaSessionIntent::Conversation
                && request.mode == MediaSessionMode::Audio
    ));
    if tokio::time::timeout(Duration::from_secs(5), attachment.open_started.notified())
        .await
        .is_err()
    {
        let failure_frames = failure_frames_before(&mut socket, Duration::from_millis(500)).await;
        panic!(
            "failed attachment was not attempted; stopped={}, audio_requests={}, failure_frames={failure_frames:?}",
            service.stopped.load(Ordering::SeqCst),
            service.audio_requests.lock().unwrap().len(),
        );
    }
    let (starts, outcome) = wait_for_terminal(&mut socket).await;
    assert_eq!(ordinary_starts + starts, 1);
    assert_eq!(outcome, TurnOutcomeDto::EndTurn);
    assert_eq!(service.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(attachment.open_count.load(Ordering::SeqCst), 1);
    let started = service
        .started
        .lock()
        .unwrap()
        .clone()
        .expect("session started");
    let stopped_contexts = service.stopped_contexts.lock().unwrap();
    assert_eq!(stopped_contexts.len(), 1);
    assert_eq!(
        stopped_contexts[0].owner, started.owner,
        "failed-open cleanup must use the session-start owner"
    );
    assert_eq!(
        stopped_contexts[0].identity.service_epoch,
        started.identity.service_epoch
    );
    endpoint.shutdown().await;
}

#[tokio::test]
async fn cancelling_realtime_continuation_stops_the_in_flight_loop_tick() {
    let scheduler = TestLoopScheduler::for_in_flight_tick();
    let (endpoint, mut socket, _service, attachment) =
        build_rig(OpenBehavior::Realtime, Some(scheduler.clone())).await;
    submit_prompt(&mut socket).await;
    approve_ordinary_tools(&mut socket).await;
    tokio::time::timeout(Duration::from_secs(5), attachment.open_ready.notified())
        .await
        .expect("realtime continuation opens");
    assert!(
        scheduler.runtime.in_flight_prompt().is_some(),
        "the loop tick remains active until its realtime continuation ends"
    );

    send_command(
        &mut socket,
        &ClientCommand::Cancel {
            session_id: None,
            turn_id: Some(TURN_ID),
        },
    )
    .await;
    let (_starts, outcome) = wait_for_terminal(&mut socket).await;
    assert_eq!(outcome, TurnOutcomeDto::Cancelled);
    assert!(scheduler.runtime.loop_ended());
    assert!(scheduler.runtime.in_flight_prompt().is_none());
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 0);
    assert_eq!(scheduler.cancel_calls.load(Ordering::SeqCst), 1);
    endpoint.shutdown().await;
}

#[tokio::test]
async fn realtime_open_failure_stops_the_in_flight_loop_tick() {
    let scheduler = TestLoopScheduler::for_in_flight_tick();
    let (endpoint, mut socket, _service, _attachment) =
        build_rig(OpenBehavior::Fail, Some(scheduler.clone())).await;
    submit_prompt(&mut socket).await;
    approve_ordinary_tools(&mut socket).await;
    let (_starts, outcome) = wait_for_terminal(&mut socket).await;
    assert_eq!(outcome, TurnOutcomeDto::EndTurn);
    assert!(scheduler.runtime.loop_ended());
    assert!(scheduler.runtime.in_flight_prompt().is_none());
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 0);
    assert_eq!(scheduler.cancel_calls.load(Ordering::SeqCst), 1);
    endpoint.shutdown().await;
}
