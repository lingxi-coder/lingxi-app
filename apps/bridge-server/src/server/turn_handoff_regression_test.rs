//! Ownership, queued lifecycle, and session-transition admission regressions.
use super::connection_regression_test::{
    captured_sink, connect_authenticated, hello, images, next_frame, Socket,
};
use super::*;
use futures_util::SinkExt;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn idle_coordinator_transitions_remain_visible_without_a_foreground_turn() {
    use lingxi_core::host::OutputStream;
    let (endpoint, mut socket, sink) = captured_sink().await;
    let connection = BridgeConnection::new();
    assert!(connection.claim_outbound(&sink).await);
    connection.handshaken.store(true, Ordering::SeqCst);
    let output = client::adapter::AdapterOutputStream::new(connection.event_sink());
    output
        .emit_coordinator_worker(&lingxi_core::host::team_registry::WorkerInfo {
            agent_id: "background-worker".into(),
            name: "worker".into(),
            agent_type: "explorer".into(),
            status: "completed".into(),
            ..Default::default()
        })
        .await;
    output.emit_coordinator_status(0, Some("team")).await;
    assert!(matches!(next_frame(&mut socket).await,
        Frame::Event(ClientEvent::CoordinatorWorker { worker }) if worker.agent_id == "background-worker" && worker.status == "completed"));
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Event(ClientEvent::CoordinatorStatus {
            active_workers: 0,
            ..
        })
    ));
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}

struct GateDrainBarrier {
    published: Notify,
    draining: Notify,
    release: Notify,
}

#[async_trait]
impl PermissionRequestSink for GateDrainBarrier {
    async fn emit_request(&self, _: PermissionRequest) {
        self.published.notify_one();
    }
}

#[async_trait]
impl ClientEventSink for GateDrainBarrier {
    async fn emit(&self, event: ClientEvent) {
        if matches!(event, ClientEvent::PermissionRequestResolved { .. }) {
            self.draining.notify_one();
            self.release.notified().await;
        }
    }
}

struct CancelBoundaryDriver {
    entries: mpsc::UnboundedSender<(String, CancellationToken)>,
    seed_returned: Notify,
    question_tx: mpsc::Sender<AskUserQuestionExchange>,
}

#[async_trait]
impl TurnDriver for CancelBoundaryDriver {
    async fn run_turn(&self, _: String) {
        panic!("connection must use cancellation-aware input");
    }

    async fn run_turn_with_images_and_cancel(
        &self,
        text: String,
        _: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        self.entries.send((text.clone(), cancel.clone())).unwrap();
        if text == "seed" {
            cancel.cancelled().await;
            self.seed_returned.notify_one();
        } else {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
            self.question_tx
                .send(AskUserQuestionExchange {
                    questions: Vec::new(),
                    timeout_secs: None,
                    resp_tx,
                })
                .await
                .unwrap();
            let _ = resp_rx.await;
        }
    }
}

#[tokio::test]
async fn cancellation_cleanup_fences_successor_question_admission() {
    use lingxi_core::host::PermissionGate;
    let connection = BridgeConnection::new();
    let barrier = Arc::new(GateDrainBarrier {
        published: Notify::new(),
        draining: Notify::new(),
        release: Notify::new(),
    });
    let gate =
        Arc::new(AdapterPermissionGate::new(barrier.clone()).with_event_sink(barrier.clone()));
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (question_tx, question_rx) = mpsc::channel(2);
    let (entries, mut receiver) = mpsc::unbounded_channel();
    let driver = Arc::new(CancelBoundaryDriver {
        entries,
        seed_returned: Notify::new(),
        question_tx,
    });
    let connection = Arc::new(
        connection
            .bind(gate.clone(), driver.clone())
            .bind_ask_user_question(broker.clone(), question_rx),
    );
    connection
        .handle_send_prompt("seed".into(), Vec::new(), Some(1))
        .await;
    receiver.recv().await.unwrap();
    connection
        .handle_send_prompt("successor".into(), Vec::new(), Some(2))
        .await;
    let checking = tokio::spawn(async move {
        gate.check(
            "Write",
            &serde_json::json!({"file_path":"/tmp/bridge-handoff"}),
        )
        .await
    });
    barrier.published.notified().await;
    let cancelling = connection.clone();
    let cancellation = tokio::spawn(async move { cancelling.cancel_active_turn(Some(1)).await });
    barrier.draining.notified().await;
    driver.seed_returned.notified().await;
    assert!(
        connection.turn_handoff.try_lock().is_err(),
        "cleanup must own the handoff through awaited resolution emission"
    );
    assert_eq!(connection.active_turn.turn_id(), Some(1));
    assert!(
        receiver.try_recv().is_err(),
        "the successor cannot start during its predecessor's cleanup"
    );
    barrier.release.notify_one();
    cancellation.await.unwrap();
    let (text, successor_cancel) = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(text, "successor");
    tokio::time::timeout(Duration::from_secs(2), async {
        while broker.pending_count().await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(connection.active_turn.turn_id(), Some(2));
    assert!(!successor_cancel.is_cancelled());
    assert_eq!(broker.pending_count().await, 1);
    connection.cancel_active_turn(Some(1)).await;
    assert_eq!(
        broker.pending_count().await,
        1,
        "stale cancel must preserve the successor's live question"
    );
    connection.cancel_active_turn(Some(2)).await;
    let task = connection.active_turn_task.lock().unwrap().take().unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    checking.await.unwrap();
}

struct FirstTextBarrier {
    inner: Arc<dyn ClientEventSink>,
    entered: Notify,
    release: Notify,
    seen: AtomicBool,
}

#[async_trait]
impl ClientEventSink for FirstTextBarrier {
    async fn emit(&self, event: ClientEvent) {
        if matches!(event, ClientEvent::TextDelta { .. }) && !self.seen.swap(true, Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.emit(event).await;
    }
}

#[tokio::test]
async fn queued_complete_inputs_emit_one_correlated_start_before_output() {
    use client::adapter::AdapterOutputStream;
    use orchestrator::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, noop_hook_executor, text_delta, MockApiClient, MockStreamingApiClient,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
    for (attachments, turn_id) in [
        (images(), None),
        (Vec::new(), Some(22)),
        (images(), Some(23)),
    ] {
        let connection = BridgeConnection::new();
        let barrier = Arc::new(FirstTextBarrier {
            inner: connection.event_sink(),
            entered: Notify::new(),
            release: Notify::new(),
            seen: AtomicBool::new(false),
        });
        let output = AdapterOutputStream::new(barrier.clone());
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![
            scripted![
                message_start("first", "test-model"),
                content_block_start_text(0),
                text_delta(0, "seed output"),
                content_block_stop(0),
                message_delta_stop("end_turn"),
                message_stop()
            ],
            scripted![
                message_start("second", "test-model"),
                content_block_start_text(0),
                text_delta(0, "queued output"),
                content_block_stop(0),
                message_delta_stop("end_turn"),
                message_stop()
            ],
        ]));
        let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(Vec::new())),
            streaming.clone(),
            Arc::new(tool_api::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(output.clone()),
            Arc::new(StaticMemoryProvider::empty()),
            "/tmp".into(),
        ));
        let driver = Arc::new(
            crate::driver::OrchestratorTurnDriver::new(orchestrator).with_message_output(output),
        );
        let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
        let connection = connection.bind(gate, driver);
        let (endpoint, mut socket, sink) = captured_sink().await;
        assert!(connection.claim_outbound(&sink).await);
        connection.handshaken.store(true, Ordering::SeqCst);
        connection
            .handle_send_prompt("seed".into(), Vec::new(), Some(1))
            .await;
        tokio::time::timeout(Duration::from_secs(2), barrier.entered.notified())
            .await
            .unwrap();
        connection
            .handle_send_prompt("queued complete input".into(), attachments.clone(), turn_id)
            .await;
        barrier.release.notify_one();
        let task = connection.active_turn_task.lock().unwrap().take().unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let mut events = Vec::new();
        while events
            .iter()
            .filter(|event| matches!(event, ClientEvent::TurnEnded { .. }))
            .count()
            < 2
        {
            if let Frame::Event(event) = next_frame(&mut socket).await {
                events.push(event);
            }
        }
        let starts = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                ClientEvent::TurnStarted { turn_id } => Some((index, *turn_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts.len(),
            1,
            "queued complete input must announce once: {events:?}"
        );
        assert_eq!(starts[0].1, turn_id);
        let queued_output = events
            .iter()
            .position(
                |event| matches!(event, ClientEvent::TextDelta { text } if text == "queued output"),
            )
            .unwrap();
        assert!(starts[0].0 < queued_output);
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 2);
        let attached = calls[1]
            .messages
            .iter()
            .flat_map(|message| match message {
                lingxi_core::types::ConversationMessage::User { content, .. } => content.clone(),
                _ => Vec::new(),
            })
            .filter(|block| matches!(block, lingxi_core::types::ContentBlock::Image { .. }))
            .count();
        assert_eq!(attached, attachments.len());
        connection.close_connection(Some(&sink)).await;
        drop(socket);
        endpoint.shutdown().await;
    }
}

#[derive(Default)]
struct ReplacementRouter {
    calls: AtomicUsize,
}

#[async_trait]
impl CommandRouter for ReplacementRouter {
    async fn route(&self, _: ClientCommand, _: Arc<dyn ClientEventSink>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
    }
    async fn dispatch_slash(&self, _: &str) -> Option<crate::router::SlashDispatchOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

struct WaitingQuestionDriver {
    question_tx: mpsc::Sender<AskUserQuestionExchange>,
    stopped_loop: AtomicUsize,
}

#[async_trait]
impl TurnDriver for WaitingQuestionDriver {
    async fn run_turn(&self, _: String) {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        self.question_tx
            .send(AskUserQuestionExchange {
                questions: Vec::new(),
                timeout_secs: None,
                resp_tx,
            })
            .await
            .unwrap();
        let _ = resp_rx.await;
    }
    async fn stop_dynamic_loop(&self) {
        self.stopped_loop.fetch_add(1, Ordering::SeqCst);
    }
}

async fn send_request(socket: &mut Socket, id: u64, method: &str, params: serde_json::Value) {
    socket
        .send(Message::Text(
            serde_json::to_string(&Frame::Request(BridgeRequest {
                id,
                method: method.into(),
                params,
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn active_session_replacement_rejects_before_router_and_socket_close_drains_question() {
    let connection = BridgeConnection::new();
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (question_tx, question_rx) = mpsc::channel(2);
    let driver = Arc::new(WaitingQuestionDriver {
        question_tx,
        stopped_loop: AtomicUsize::new(0),
    });
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    let router = Arc::new(ReplacementRouter::default());
    let connection = Arc::new(
        connection
            .bind(gate, driver.clone())
            .bind_ask_user_question(broker.clone(), question_rx)
            .bind_router(router.clone()),
    );
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection.clone())
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    send_request(
        &mut socket,
        1,
        "hello",
        serde_json::to_value(hello()).unwrap(),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse { error: None, .. })
    ));
    send_request(
        &mut socket,
        2,
        "send_prompt",
        serde_json::to_value(ClientCommand::SendPrompt {
            text: "ask".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: Some(1),
            visualization_context: None,
        })
        .unwrap(),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Event(ClientEvent::AskUserQuestion { .. })
    ));
    let commands = [
        ClientCommand::ClearSession,
        ClientCommand::NewSession {
            cwd: None,
            model: None,
        },
        ClientCommand::ResumeSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            cwd: None,
        },
        ClientCommand::RunSlashCommand {
            raw: "/clear".into(),
            turn_id: Some(11),
        },
        ClientCommand::RunSlashCommand {
            raw: "/NEW title".into(),
            turn_id: Some(12),
        },
        ClientCommand::RunSlashCommand {
            raw: "/reset".into(),
            turn_id: Some(13),
        },
    ];
    for (index, command) in commands.into_iter().enumerate() {
        send_request(
            &mut socket,
            index as u64 + 3,
            "command",
            serde_json::to_value(command).unwrap(),
        )
        .await;
        assert!(matches!(
            next_frame(&mut socket).await,
            Frame::Event(ClientEvent::Error {
                kind: ErrorKindDto::Protocol,
                ..
            }) | Frame::Event(ClientEvent::SlashCommandResult { is_error: true, .. })
        ));
        assert_eq!(broker.pending_count().await, 1);
    }
    assert_eq!(router.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        driver.stopped_loop.load(Ordering::SeqCst),
        0,
        "rejected replacements must preserve the current loop"
    );
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while connection.handshaken.load(Ordering::SeqCst) || broker.pending_count().await != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("socket close must reach abort/drain after rejected controls");
    let mut reconnected = connect_authenticated(&endpoint).await;
    send_request(
        &mut reconnected,
        100,
        "hello",
        serde_json::to_value(hello()).unwrap(),
    )
    .await;
    assert!(matches!(
        next_frame(&mut reconnected).await,
        Frame::Response(BridgeResponse { error: None, .. })
    ));
    reconnected.close(None).await.unwrap();
    endpoint.shutdown().await;
}

#[tokio::test]
async fn session_replacement_waits_for_settled_turn_handoff() {
    use std::{future::Future, task::Poll};
    for command in [
        ClientCommand::ClearSession,
        ClientCommand::NewSession {
            cwd: None,
            model: None,
        },
        ClientCommand::ResumeSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            cwd: None,
        },
        ClientCommand::RunSlashCommand {
            raw: "/reset".into(),
            turn_id: None,
        },
    ] {
        let router = Arc::new(ReplacementRouter::default());
        let connection = BridgeConnection::new().bind_router(router.clone());
        let handoff = connection.turn_handoff.lock().await;
        // Drain ownership is temporarily false while queued work is inspected.
        connection.turn_running.store(false, Ordering::SeqCst);
        let mut dispatch = Box::pin(connection.dispatch(command));
        std::future::poll_fn(|cx| {
            assert!(dispatch.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        connection.turn_running.store(true, Ordering::SeqCst);
        drop(handoff);
        tokio::time::timeout(Duration::from_secs(2), dispatch)
            .await
            .unwrap();
        assert_eq!(router.calls.load(Ordering::SeqCst), 0);
    }
}

struct CorrelatedSnapshotRouter {
    calls: AtomicUsize,
}

fn snapshot_events() -> Vec<ClientEvent> {
    vec![
        ClientEvent::SessionAgentList {
            session_id: "snapshot-session".into(),
            agents: Vec::new(),
        },
        ClientEvent::CoordinatorWorker {
            worker: client::protocol::listings::CoordinatorWorkerDto {
                agent_id: "live-worker".into(),
                name: "live".into(),
                agent_type: "explorer".into(),
                status: "idle".into(),
            },
        },
        ClientEvent::CoordinatorStatus {
            active_workers: 1,
            team: Some("snapshot-team".into()),
        },
    ]
}

#[async_trait]
impl CommandRouter for CorrelatedSnapshotRouter {
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        assert!(matches!(command, ClientCommand::ListSessionAgents));
        sink.emit(snapshot_events().remove(0)).await;
    }
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(snapshot_events())
    }
}

#[tokio::test]
async fn runtime_snapshot_requires_hello_and_replies_as_one_correlated_frame() {
    let router = Arc::new(CorrelatedSnapshotRouter {
        calls: AtomicUsize::new(0),
    });
    let connection = Arc::new(BridgeConnection::new().bind_router(router.clone()));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    let params = serde_json::json!({"type":"list_session_agents"});
    send_request(&mut socket, 41, "desktop_runtime_snapshot", params.clone()).await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse {
            id: 41,
            error: Some(_),
            ..
        })
    ));
    assert_eq!(router.calls.load(Ordering::SeqCst), 0);
    send_request(
        &mut socket,
        42,
        "hello",
        serde_json::to_value(hello()).unwrap(),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse {
            id: 42,
            error: None,
            ..
        })
    ));
    send_request(&mut socket, 43, "desktop_runtime_snapshot", params).await;
    let Frame::Response(BridgeResponse {
        id: 43,
        result: Some(result),
        error: None,
    }) = next_frame(&mut socket).await
    else {
        panic!("snapshot must be one correlated response, never incremental push frames");
    };
    let events: Vec<ClientEvent> = serde_json::from_value(result["events"].clone()).unwrap();
    assert_eq!(events, snapshot_events());
    assert_eq!(router.calls.load(Ordering::SeqCst), 1);
    send_request(
        &mut socket,
        44,
        "desktop_runtime_snapshot",
        serde_json::json!({"type":"clear_session"}),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse {
            id: 44,
            error: Some(_),
            ..
        })
    ));
    assert_eq!(router.calls.load(Ordering::SeqCst), 1);
    // Ordinary public roster commands retain their existing event semantics.
    send_request(
        &mut socket,
        45,
        "list_session_agents",
        serde_json::json!({"type":"list_session_agents"}),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Event(ClientEvent::SessionAgentList { .. })
    ));
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}

struct BarrierSnapshotRouter {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl CommandRouter for BarrierSnapshotRouter {
    async fn route(&self, _: ClientCommand, _: Arc<dyn ClientEventSink>) {
        unreachable!()
    }
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        let events = snapshot_events();
        self.entered.notify_one();
        self.release.notified().await;
        Ok(events)
    }
}

#[tokio::test]
async fn runtime_snapshot_read_fences_live_worker_delivery_until_after_reply() {
    use std::{future::Future, task::Poll};
    let router = Arc::new(BarrierSnapshotRouter {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let connection = Arc::new(BridgeConnection::new().bind_router(router.clone()));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection.clone())
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    send_request(
        &mut socket,
        1,
        "hello",
        serde_json::to_value(hello()).unwrap(),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse { error: None, .. })
    ));
    connection.active_turn.begin(Some(99));
    send_request(
        &mut socket,
        2,
        "desktop_runtime_snapshot",
        serde_json::json!({"type":"list_session_agents"}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), router.entered.notified())
        .await
        .unwrap();
    let output = connection.event_sink();
    let live = ClientEvent::CoordinatorStatus {
        active_workers: 0,
        team: Some("snapshot-team".into()),
    };
    let mut delivery = Box::pin(output.emit(live.clone()));
    std::future::poll_fn(|cx| {
        assert!(
            delivery.as_mut().poll(cx).is_pending(),
            "a live completion must wait for the captured snapshot reply"
        );
        Poll::Ready(())
    })
    .await;
    router.release.notify_one();
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse {
            id: 2,
            error: None,
            ..
        })
    ));
    tokio::time::timeout(Duration::from_secs(2), delivery)
        .await
        .expect("snapshot must release delivery fence");
    assert!(matches!(next_frame(&mut socket).await, Frame::Event(event) if event == live));
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}
