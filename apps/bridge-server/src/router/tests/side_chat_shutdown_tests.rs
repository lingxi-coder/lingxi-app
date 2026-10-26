#![allow(clippy::unwrap_used)]

use super::EngineCommandRouter;
use crate::server::BridgeConnection;
use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use client::adapter::{
    AdapterPermissionGate, AskUserQuestionBroker, ClientEventSink, PermissionRequestSink,
};
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use client::protocol::text::ClientTextDto;
use command_api::RegistrySlashDispatcher;
use futures_util::{SinkExt, StreamExt};
use harness_runtime::side_chat::{SideChatFactory, SideChatSession};
use lingxi_core::host::auth::{AuthError, AuthHandle, LoginInfo};
use lingxi_core::host::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use lingxi_core::host::{CostSnapshot, OrchestratorHandle, OutputStream};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, SideChatParentSnapshot};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const AUTH_TOKEN: &str = "side-chat-shutdown-token-32chars";

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct StubAuth;

#[async_trait]
impl AuthHandle for StubAuth {
    fn register_account_change_observer(
        &self,
        _observer: Weak<dyn lingxi_core::host::auth::AccountChangeObserver>,
    ) {
    }

    async fn login(&self) -> Result<LoginInfo, AuthError> {
        Ok(LoginInfo {
            email: "side-chat-test@example.invalid".into(),
            org_id: "side-chat-test".into(),
        })
    }

    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }

    async fn current_user(&self) -> Option<LoginInfo> {
        None
    }
}

struct EmptyTasks;

#[async_trait]
impl TaskRegistryHandle for EmptyTasks {
    async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal(
            "unused in side-chat test".into(),
        ))
    }

    async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }

    async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        Ok(Vec::new())
    }

    async fn update(
        &self,
        _id: &str,
        _patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal(
            "unused in side-chat test".into(),
        ))
    }

    async fn set_status(&self, _id: &str, _status: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal(
            "unused in side-chat test".into(),
        ))
    }

    async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal(
            "unused in side-chat test".into(),
        ))
    }

    async fn output(
        &self,
        _id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        Err(TaskRegistryError::Internal(
            "unused in side-chat test".into(),
        ))
    }
}

struct PendingRequestDrop(Arc<Notify>);

impl Drop for PendingRequestDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct BlockingStreaming {
    started: Arc<Notify>,
    dropped: Arc<Notify>,
}

#[async_trait]
impl orchestrator::StreamingApiClient for BlockingStreaming {
    async fn stream(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&lingxi_llm_client::providers::anthropic::system_prompt::SystemPromptInput>,
        _messages: Vec<lingxi_core::types::ConversationMessage>,
        _tools: Vec<llm_runtime::ToolDeclaration>,
        _query_source: &str,
        _skip_global_cache_for_system_prompt: bool,
        _request_dispatch_admission: Option<llm_runtime::RequestDispatchAdmission>,
    ) -> Result<
        futures_util::stream::BoxStream<
            'static,
            Result<llm_runtime::HistoryEvent, llm_runtime::LlmError>,
        >,
        llm_runtime::LlmError,
    > {
        self.started.notify_one();
        let _drop_notice = PendingRequestDrop(self.dropped.clone());
        std::future::pending().await
    }
}

struct TerminalGateOutput {
    inner: MockOutputStream,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl OutputStream for TerminalGateOutput {
    async fn emit_text(&self, text: &Utf16JsonProjection) {
        self.inner.emit_text(text).await;
    }

    async fn emit_tool_call(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        input: &Utf16JsonProjection,
    ) {
        self.inner.emit_tool_call(id, tool, input).await;
    }

    async fn emit_tool_result(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        model_text: &Utf16JsonProjection,
        result: &Utf16JsonProjection,
        mcp_meta: Option<&Utf16JsonProjection>,
    ) {
        self.inner
            .emit_tool_result(id, tool, model_text, result, mcp_meta)
            .await;
    }

    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.emit_end_turn(stop_reason, cost).await;
    }
}

struct BlockingFactory {
    stream_started: Arc<Notify>,
    stream_dropped: Arc<Notify>,
    terminal_entered: Arc<Notify>,
    terminal_release: Arc<Notify>,
    child: Arc<StdMutex<Option<Weak<ConversationOrchestrator>>>>,
}

#[async_trait]
impl SideChatFactory for BlockingFactory {
    async fn build(
        &self,
        snapshot: SideChatParentSnapshot,
        sink: Arc<dyn ClientEventSink>,
        permission: Arc<dyn PermissionRequestSink>,
    ) -> Result<SideChatSession, String> {
        let permission_gate =
            Arc::new(AdapterPermissionGate::new(permission).with_event_sink(sink.clone()));
        let question_broker = Arc::new(AskUserQuestionBroker::new(sink));
        let commands = Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new()));
        let orchestrator =
            ConversationOrchestrator::into_shared(ConversationOrchestrator::new_side_chat(
                snapshot.config.clone(),
                Arc::new(MockApiClient::new(Vec::new())),
                Arc::new(BlockingStreaming {
                    started: self.stream_started.clone(),
                    dropped: self.stream_dropped.clone(),
                }),
                Arc::new(tool_api::ToolRegistry::new()),
                noop_hook_executor(),
                permission_gate.clone(),
                Arc::new(TerminalGateOutput {
                    inner: MockOutputStream::new(),
                    entered: self.terminal_entered.clone(),
                    release: self.terminal_release.clone(),
                }),
                Arc::new(StaticMemoryProvider::empty()),
                snapshot.cwd.clone(),
            ));
        orchestrator.seed_side_chat(&snapshot).await?;
        *self.child.lock().unwrap() = Some(Arc::downgrade(&orchestrator));

        Ok(SideChatSession {
            orchestrator,
            dispatcher: Arc::new(RegistrySlashDispatcher::new(commands)),
            permission_gate,
            question_broker,
            cleanup_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
}

fn parent_orchestrator() -> Arc<ConversationOrchestrator> {
    ConversationOrchestrator::into_shared(ConversationOrchestrator::new(
        orchestrator::OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(Vec::new())),
        Arc::new(tool_api::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    ))
}

async fn connect(port: u16) -> TestSocket {
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
        .header("x-lingxi-ide-authorization", AUTH_TOKEN)
        .body(())
        .unwrap();
    let (socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("loopback websocket upgrade must succeed");
    assert_eq!(response.status(), 101);
    socket
}

fn source<T: serde::Serialize>(value: &T) -> Utf16JsonProjection {
    Utf16JsonProjection::parse(&serde_json::to_string(value).unwrap()).unwrap()
}

async fn next_frame(socket: &mut TestSocket) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("bridge frame must arrive")
            .expect("websocket stream remains open")
            .expect("websocket frame is valid");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).expect("decode bridge frame");
        }
    }
}

async fn send_frame(socket: &mut TestSocket, frame: Frame) {
    socket
        .send(Message::Text(
            serde_json::to_string(&frame).expect("serialize bridge frame"),
        ))
        .await
        .expect("send bridge frame");
}

async fn send_hello(socket: &mut TestSocket) {
    send_frame(
        socket,
        Frame::Request(BridgeRequest {
            trace_context: None,
            id: 1,
            method: "hello".into(),
            params: source(&ClientHello {
                protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
                client_name: "side-chat-shutdown-test".into(),
                capabilities: Capabilities::default(),
            }),
        }),
    )
    .await;
    match next_frame(socket).await {
        Frame::Response(response) => assert!(response.error.is_none()),
        other => panic!("expected hello response, received {other:?}"),
    }
}

async fn send_command(socket: &mut TestSocket, command: &ClientCommand) {
    send_frame(
        socket,
        Frame::Request(BridgeRequest {
            trace_context: None,
            id: 2,
            method: "submit".into(),
            params: source(command),
        }),
    )
    .await;
}

#[tokio::test]
async fn endpoint_shutdown_waits_for_live_side_chat_child_to_stop() {
    let stream_started = Arc::new(Notify::new());
    let stream_dropped = Arc::new(Notify::new());
    let terminal_entered = Arc::new(Notify::new());
    let terminal_release = Arc::new(Notify::new());
    let child = Arc::new(StdMutex::new(None));
    let factory = Arc::new(BlockingFactory {
        stream_started: stream_started.clone(),
        stream_dropped: stream_dropped.clone(),
        terminal_entered: terminal_entered.clone(),
        terminal_release: terminal_release.clone(),
        child: child.clone(),
    });
    let parent = parent_orchestrator();
    let router = Arc::new(
        EngineCommandRouter::new(
            parent.clone() as Arc<dyn OrchestratorHandle>,
            Arc::new(StubAuth),
            Arc::new(EmptyTasks),
            None,
            None,
        )
        .with_side_chat_runtime(parent, factory),
    );
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(
        BridgeConnection::new().bind_router(router.clone()),
    ))
    .await
    .expect("loopback endpoint starts");
    endpoint.set_auth_token(AUTH_TOKEN.into());

    let mut socket = connect(endpoint.port()).await;
    send_hello(&mut socket).await;
    send_command(
        &mut socket,
        &ClientCommand::SideChatCreate {
            request_id: "shutdown-child".into(),
            initial_input: Some(ClientTextDto::from_utf8("hold this child turn")),
        },
    )
    .await;

    loop {
        match next_frame(&mut socket).await {
            Frame::Event(ClientEvent::SideChatCreated { request_id, .. })
                if request_id == "shutdown-child" =>
            {
                break;
            }
            Frame::Event(ClientEvent::SideChatError { message, .. }) => {
                panic!("side chat creation failed: {message}");
            }
            _ => {}
        }
    }
    tokio::time::timeout(Duration::from_secs(5), stream_started.notified())
        .await
        .expect("child must enter the pending provider request");

    let mut shutdown = tokio::spawn(endpoint.shutdown());
    tokio::time::timeout(Duration::from_secs(5), stream_dropped.notified())
        .await
        .expect("connection close must cancel the child provider request");
    tokio::time::timeout(Duration::from_secs(5), terminal_entered.notified())
        .await
        .expect("child must reach its terminal cleanup before shutdown completes");

    let shutdown_waited_for_child = tokio::time::timeout(Duration::from_millis(100), &mut shutdown)
        .await
        .is_err();
    terminal_release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), &mut shutdown)
        .await
        .expect("endpoint shutdown completes after the child terminal boundary is released")
        .expect("endpoint shutdown task succeeds");
    drop(socket);

    assert!(
        shutdown_waited_for_child,
        "endpoint shutdown returned while the side-chat child was still in terminal cleanup"
    );
    assert!(
        child.lock().unwrap().as_ref().unwrap().upgrade().is_none(),
        "the stopped child session must be released before endpoint shutdown returns"
    );
    assert!(
        router.side_chat_manager.lock().await.is_none(),
        "connection close must retire the per-connection manager"
    );
}
