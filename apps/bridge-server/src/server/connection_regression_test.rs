//! Regression coverage for reconnect fencing and complete queued input.
use super::*;
use crate::cron_host::HostCronFirer;
use cron::CronJobFirer;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::{handshake::client::generate_key, Message};

pub(super) type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct CaptureSink(mpsc::UnboundedSender<FrameSink>);
#[async_trait]
impl FramePump for CaptureSink {
    async fn on_frame(&self, _: Frame, out: FrameSink) {
        let _ = self.0.send(out);
    }
    async fn on_close(&self) {}
}

/// Obtain a real SDK FrameSink without manufacturing its private channel.
pub(super) async fn captured_sink() -> (bridge::McpEndpoint, Socket, FrameSink) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let endpoint =
        bridge::McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(CaptureSink(tx)))
            .await
            .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    let frame = Frame::Request(BridgeRequest {
        id: 0,
        method: "capture".into(),
        params: serde_json::json!({}),
    });
    socket
        .send(Message::Text(serde_json::to_string(&frame).unwrap()))
        .await
        .unwrap();
    let sink = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    (endpoint, socket, sink)
}

pub(super) async fn connect_authenticated(endpoint: &bridge::McpEndpoint) -> Socket {
    let port = endpoint.port();
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("ws://127.0.0.1:{port}/mcp"))
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header(bridge::AUTH_HEADER_NAME, "connection-regression-token")
        .body(())
        .unwrap();
    let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
}

pub(super) async fn next_frame(socket: &mut Socket) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

pub(super) fn hello() -> ClientHello {
    ClientHello {
        protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
        client_name: "connection-regression".into(),
        capabilities: Capabilities::default(),
    }
}

struct BlockingDrop {
    entered: Arc<Notify>,
    release: std::sync::mpsc::Receiver<()>,
}
impl Drop for BlockingDrop {
    fn drop(&mut self) {
        self.entered.notify_one();
        self.release.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_cannot_claim_until_old_cleanup_has_finished() {
    let (first_endpoint, mut first_socket, first_sink) = captured_sink().await;
    let (second_endpoint, second_socket, second_sink) = captured_sink().await;
    let connection = BridgeConnection::new();
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (question_tx, question_rx) = mpsc::channel(1);
    let connection = Arc::new(connection.bind_ask_user_question(broker.clone(), question_rx));
    assert!(connection.claim_outbound(&first_sink).await);
    connection.handshaken.store(true, Ordering::SeqCst);
    connection.active_turn.begin(Some(1));
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    question_tx
        .send(AskUserQuestionExchange {
            questions: Vec::new(),
            timeout_secs: None,
            resp_tx: response_tx,
        })
        .await
        .unwrap();
    assert!(matches!(
        next_frame(&mut first_socket).await,
        Frame::Event(ClientEvent::AskUserQuestion { .. })
    ));
    connection.turn_running.store(true, Ordering::SeqCst);
    connection
        .queue
        .enqueue(prompt_command("old input".into()))
        .await;

    let ready = Arc::new(Notify::new());
    let dropping = Arc::new(Notify::new());
    let (release, receiver) = std::sync::mpsc::channel();
    let task_ready = ready.clone();
    let task_dropping = dropping.clone();
    let task = tokio::spawn(async move {
        let _guard = BlockingDrop {
            entered: task_dropping,
            release: receiver,
        };
        task_ready.notify_one();
        std::future::pending::<()>().await;
    });
    *connection.active_turn_task.lock().unwrap() = Some(task);
    ready.notified().await;
    let old = connection.clone();
    let closing_sink = first_sink.clone();
    let closing = tokio::spawn(async move { old.close_connection(Some(&closing_sink)).await });
    dropping.notified().await;

    let claiming = connection.clone();
    let sink = second_sink.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let mut claim = tokio::spawn(async move {
        let _ = entered.send(());
        claiming.claim_outbound(&sink).await
    });
    waiting.await.unwrap();
    let blocked = tokio::time::timeout(Duration::from_millis(30), &mut claim)
        .await
        .is_err();
    // Always release the Drop barrier, including when the regression fails.
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();
    assert!(
        response_rx.await.is_err(),
        "close must drain the published question"
    );
    assert_eq!(broker.pending_count().await, 0);
    assert!(
        blocked,
        "new input must not be admitted during old abort/join"
    );
    assert!(claim.await.unwrap());
    assert!(connection.queue.is_empty().await);
    connection
        .queue
        .enqueue(prompt_command("new input".into()))
        .await;
    connection.close_connection(Some(&first_sink)).await;
    assert_eq!(
        connection.queue.len().await,
        1,
        "a stale peer close has no effect on the new queue"
    );
    connection.close_connection(Some(&second_sink)).await;
    drop(first_socket);
    drop(second_socket);
    first_endpoint.shutdown().await;
    second_endpoint.shutdown().await;
}

struct ObservedSink {
    inner: Arc<dyn ClientEventSink>,
    emitted: Arc<Notify>,
}
#[async_trait]
impl ClientEventSink for ObservedSink {
    async fn emit(&self, event: ClientEvent) {
        self.inner.emit(event).await;
        self.emitted.notify_one();
    }
}

fn automation_request(id: &str) -> cron::automation::AutomationRunRequest {
    cron::automation::AutomationRunRequest {
        run_id: id.into(),
        claim_generation: 1,
        task: serde_json::from_value(serde_json::json!({
            "id":"d00000001", "cron":"* * * * *", "prompt":"scheduled prompt", "createdAt":0,
            "automation": { "version":2, "status":"active", "model":"openai/test",
                "reasoning":{"type":"automatic"}, "runMode":"new_session",
                "notificationPolicy":"none", "runs":[] }
        }))
        .unwrap(),
    }
}

#[tokio::test]
async fn cron_request_survives_pre_hello_and_reconnect_without_duplicate_live_delivery() {
    let connection = BridgeConnection::new();
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let request = automation_request("pending-occurrence");
    let wire_id = crate::cron_host::transport_run_id(&request);
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await; // the real firer emitted before any client attached

    let (first_endpoint, mut first_socket, first_sink) = captured_sink().await;
    assert!(connection.claim_outbound(&first_sink).await);
    connection.handle_hello(1, hello()).await;
    assert!(matches!(
        next_frame(&mut first_socket).await,
        Frame::Response(_)
    ));
    assert!(matches!(next_frame(&mut first_socket).await,
        Frame::Event(ClientEvent::CronRunRequested { run_id, .. }) if run_id == wire_id));
    connection.handle_hello(2, hello()).await;
    assert!(matches!(
        next_frame(&mut first_socket).await,
        Frame::Response(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), first_socket.next())
            .await
            .is_err(),
        "a repeated hello must not duplicate an already delivered request"
    );

    connection.close_connection(Some(&first_sink)).await;
    let (second_endpoint, mut second_socket, second_sink) = captured_sink().await;
    assert!(connection.claim_outbound(&second_sink).await);
    connection.handle_hello(3, hello()).await;
    assert!(matches!(
        next_frame(&mut second_socket).await,
        Frame::Response(_)
    ));
    assert!(matches!(next_frame(&mut second_socket).await,
        Frame::Event(ClientEvent::CronRunRequested { run_id, .. }) if run_id == wire_id));
    firer
        .complete(
            &wire_id,
            Some("execution-session".into()),
            Some("done".into()),
            None,
        )
        .await;
    assert_eq!(job.await.unwrap().unwrap().summary, "done");
    connection.close_connection(Some(&second_sink)).await;
    assert!(connection.claim_outbound(&second_sink).await);
    connection.handle_hello(4, hello()).await;
    assert!(matches!(
        next_frame(&mut second_socket).await,
        Frame::Response(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), second_socket.next())
            .await
            .is_err(),
        "completed requests must never replay"
    );
    connection.close_connection(Some(&second_sink)).await;
    drop(first_socket);
    drop(second_socket);
    first_endpoint.shutdown().await;
    second_endpoint.shutdown().await;
}

#[tokio::test]
async fn cancelled_cron_request_is_removed_from_the_replay_buffer() {
    let connection = BridgeConnection::new();
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let request = automation_request("cancelled-occurrence");
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await;
    firer.cancel_run("cancelled-occurrence").await.unwrap();
    assert!(job.await.unwrap().is_err());
    let mut replayed = Vec::new();
    connection
        .cron_requests
        .replay(|event| {
            replayed.push(event);
            true
        })
        .await;
    assert!(replayed.is_empty());
}

struct InputDriver {
    entries: mpsc::UnboundedSender<(String, Vec<ImageRefDto>, CancellationToken)>,
    release_seed: Notify,
}
#[async_trait]
impl TurnDriver for InputDriver {
    async fn run_turn(&self, _: String) {
        panic!("metadata input must keep its complete entrypoint");
    }
    async fn run_turn_with_images_and_cancel(
        &self,
        text: String,
        images: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        self.entries
            .send((text.clone(), images, cancel.clone()))
            .unwrap();
        if text == "seed" {
            self.release_seed.notified().await;
        } else {
            cancel.cancelled().await;
        }
    }
}

fn input_connection() -> (
    BridgeConnection,
    Arc<InputDriver>,
    mpsc::UnboundedReceiver<(String, Vec<ImageRefDto>, CancellationToken)>,
) {
    let (entries, receiver) = mpsc::unbounded_channel();
    let driver = Arc::new(InputDriver {
        entries,
        release_seed: Notify::new(),
    });
    let mut connection = BridgeConnection::new();
    connection.driver = Some(driver.clone());
    (connection, driver, receiver)
}
async fn input_started(
    receiver: &mut mpsc::UnboundedReceiver<(String, Vec<ImageRefDto>, CancellationToken)>,
) -> (String, Vec<ImageRefDto>, CancellationToken) {
    tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn join_turn(connection: &BridgeConnection) {
    let task = connection.active_turn_task.lock().unwrap().take().unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}
pub(super) fn images() -> Vec<ImageRefDto> {
    vec![ImageRefDto {
        media_type: "image/png".into(),
        base64: "aW1hZ2U=".into(),
    }]
}

#[tokio::test]
async fn queued_images_and_turn_id_survive_mid_turn_drain_and_targeted_cancel() {
    let (connection, driver, mut entries) = input_connection();
    connection
        .handle_send_prompt("seed".into(), Vec::new(), Some(1))
        .await;
    let (_, _, seed_cancel) = input_started(&mut entries).await;
    connection.loop_runtime.begin_tick("old loop tick".into());
    connection
        .handle_send_prompt("follow-up".into(), images(), Some(2))
        .await;
    assert!(
        connection.queue.take_mid_turn_prompt().await.is_none(),
        "the text-only SDK adapter must not consume a prompt carrying attachments or identity"
    );
    driver.release_seed.notify_one();
    let (text, attachments, follow_cancel) = input_started(&mut entries).await;
    assert_eq!(text, "follow-up");
    assert_eq!(attachments, images());
    assert_eq!(connection.active_turn.turn_id(), Some(2));
    assert!(
        connection.loop_runtime.in_flight_prompt().is_none(),
        "a queued human input must not inherit a prior loop tick"
    );
    connection.cancel_active_turn(Some(1)).await;
    assert!(
        !follow_cancel.is_cancelled(),
        "a stale seed cancel cannot cancel its successor"
    );
    connection.cancel_active_turn(Some(2)).await;
    assert!(follow_cancel.is_cancelled());
    assert!(!seed_cancel.is_cancelled());
    join_turn(&connection).await;
    assert!(connection.queued_prompt_payloads.lock().await.is_empty());
}

#[tokio::test]
async fn cancelling_queued_identity_removes_only_that_prompt() {
    let (connection, driver, mut entries) = input_connection();
    connection
        .handle_send_prompt("seed".into(), Vec::new(), Some(1))
        .await;
    let (_, _, seed_cancel) = input_started(&mut entries).await;
    connection
        .handle_send_prompt("cancel before start".into(), images(), Some(2))
        .await;
    connection.cancel_active_turn(Some(2)).await;
    assert!(!seed_cancel.is_cancelled());
    assert!(connection.queue.is_empty().await);
    assert!(connection.queued_prompt_payloads.lock().await.is_empty());
    driver.release_seed.notify_one();
    join_turn(&connection).await;
    assert!(
        entries.try_recv().is_err(),
        "cancelled queued input must never reach the model driver"
    );
}

#[tokio::test]
async fn uncorrelated_text_keeps_legacy_mid_turn_delivery() {
    let (connection, driver, mut entries) = input_connection();
    connection
        .handle_send_prompt("seed".into(), Vec::new(), Some(1))
        .await;
    input_started(&mut entries).await;
    connection
        .handle_send_prompt("folded text".into(), Vec::new(), None)
        .await;
    assert_eq!(
        connection.queue.take_mid_turn_prompt().await.as_deref(),
        Some("folded text")
    );
    assert!(connection.queued_prompt_payloads.lock().await.is_empty());
    driver.release_seed.notify_one();
    join_turn(&connection).await;
    assert!(entries.try_recv().is_err());
}

#[tokio::test]
async fn batch_selection_serializes_complete_prompt_publication() {
    use std::{future::Future, task::Poll};
    let (connection, _, _) = input_connection();
    connection.turn_running.store(true, Ordering::SeqCst);
    let plain = prompt_command("old text".into());
    let plain_id = plain.uuid.clone();
    connection.queue.enqueue(plain).await;
    let payloads = connection.queued_prompt_payloads.lock().await;
    let mut selection = Box::pin(batchable_prompt_snapshot(
        &connection.queue,
        &connection.queued_prompt_payloads,
        &connection.turn_handoff,
    ));
    // Park exactly between acquiring admission and reading metadata. A prompt
    // publication must remain blocked throughout both metadata and queue reads.
    std::future::poll_fn(|cx| {
        assert!(selection.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(connection.turn_handoff.try_lock().is_err());
    let mut publication =
        Box::pin(connection.handle_send_prompt("new attachment".into(), images(), Some(2)));
    std::future::poll_fn(|cx| {
        assert!(publication.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(payloads);
    let batch = selection.await;
    assert_eq!(
        batch.iter().map(|c| c.uuid.as_str()).collect::<Vec<_>>(),
        vec![plain_id.as_str()]
    );
    publication.await;
    let queued = connection.queue.snapshot().await;
    let complete = queued
        .iter()
        .find(|c| c.text() == Some("new attachment"))
        .unwrap();
    assert!(connection
        .queued_prompt_payloads
        .lock()
        .await
        .contains_key(&complete.uuid));
    assert_eq!(complete.priority, QueuePriority::Later);
    connection.cancel_active_turn(Some(2)).await;
    assert_eq!(connection.queue.len().await, 1);
}

#[tokio::test]
async fn cron_claim_identity_and_metadata_fence_stale_failure_completion() {
    let connection = BridgeConnection::new();
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let first = automation_request("retried-occurrence");
    assert!(
        first.task.automation.as_ref().unwrap().runs.is_empty(),
        "use the actual SDK claimed snapshot contract"
    );
    let first_id = crate::cron_host::transport_run_id(&first);
    let first_running = firer.clone();
    let first_job = tokio::spawn(async move { first_running.fire_automation(&first).await });
    emitted.notified().await;
    let mut produced = Vec::new();
    connection
        .cron_requests
        .replay(|event| {
            produced.push(event);
            true
        })
        .await;
    firer
        .complete(
            &first_id,
            None,
            None,
            Some("busy: target is running".into()),
        )
        .await;
    assert!(first_job.await.unwrap().unwrap_err().starts_with("busy:"));

    let mut second = automation_request("retried-occurrence");
    second.claim_generation = 2;
    let second_id = crate::cron_host::transport_run_id(&second);
    assert_ne!(first_id, second_id);
    let second_running = firer.clone();
    let second_job = tokio::spawn(async move { second_running.fire_automation(&second).await });
    emitted.notified().await;
    connection
        .cron_requests
        .replay(|event| {
            produced.push(event);
            true
        })
        .await;
    // Failure replies have no session identity; only the claim token fences them.
    firer
        .complete(&first_id, None, None, Some("late old failure".into()))
        .await;
    assert!(!second_job.is_finished());
    assert!(firer.pending.lock().await.contains_key(&second_id));
    for (index, event) in produced.iter().enumerate() {
        let ClientEvent::CronRunRequested { run_id, task } = event else {
            panic!("expected execution request")
        };
        let run = &task.automation.as_ref().unwrap().runs[0];
        assert_eq!(run.id, "retried-occurrence");
        assert_eq!(run.claim_generation, Some(index as u64 + 1));
        assert_eq!(
            run_id,
            &format!(
                "lingxi-cron-claim-v1:[\"retried-occurrence\",{}]",
                index + 1
            )
        );
    }
    // Rust and Electron consume one wire fixture. Ignore only process/time
    // values; all protocol fields and claim/admission metadata must agree.
    fn normalize(mut events: serde_json::Value) -> serde_json::Value {
        for event in events.as_array_mut().unwrap() {
            let task = event.get_mut("task").unwrap();
            task.as_object_mut().unwrap().remove("next_run_at");
            for run in task["automation"]["runs"].as_array_mut().unwrap() {
                run.as_object_mut().unwrap().remove("ownerPid");
            }
        }
        events
    }
    let shared_fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/test/fixtures/cron-claim-replay.json"
    ))
    .unwrap();
    assert_eq!(
        normalize(serde_json::to_value(&produced).unwrap()),
        normalize(shared_fixture)
    );
    firer
        .complete(
            &second_id,
            Some("new-session".into()),
            Some("success".into()),
            None,
        )
        .await;
    assert_eq!(second_job.await.unwrap().unwrap().summary, "success");
}

#[tokio::test]
async fn cancelling_delivered_cron_replays_only_recovery_until_termination_confirmed() {
    let connection = BridgeConnection::new();
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let request = automation_request("possibly-running");
    let wire_id = crate::cron_host::transport_run_id(&request);
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await;
    connection.cron_requests.replay(|_| true).await;
    connection.cron_requests.disconnected().await;
    assert!(firer
        .cancel_run("possibly-running")
        .await
        .unwrap_err()
        .contains("not confirmed termination"));
    let mut replayed = Vec::new();
    connection
        .cron_requests
        .replay(|event| {
            replayed.push(event);
            true
        })
        .await;
    assert_eq!(replayed.len(), 1);
    let ClientEvent::CronRunRequested { run_id, task } = &replayed[0] else {
        panic!("expected recovery request")
    };
    assert_eq!(run_id, &wire_id);
    assert_eq!(
        task.automation.as_ref().unwrap().runs[0].error.as_deref(),
        Some(crate::cron_host::HOST_CANCEL_REQUESTED)
    );
    assert!(!job.is_finished());
    firer
        .complete(&wire_id, None, None, Some("cancelled: host stopped".into()))
        .await;
    assert!(job.await.unwrap().is_err());
    firer.cancel_run("possibly-running").await.unwrap();
}

#[tokio::test]
async fn dropped_cron_producer_rejects_late_binding_until_real_host_completion() {
    let connection = BridgeConnection::new();
    let (first_endpoint, mut socket, sink) = captured_sink().await;
    assert!(connection.claim_outbound(&sink).await);
    connection.handle_hello(1, hello()).await;
    assert!(matches!(next_frame(&mut socket).await, Frame::Response(_)));
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let request = automation_request("producer-dropped");
    let wire_id = crate::cron_host::transport_run_id(&request);
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Event(ClientEvent::CronRunRequested { .. })
    ));
    // This is the scheduler's actual shutdown order: drop the outer firing
    // future/receiver, then ask the firer to cancel and join its owned resources.
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    assert!(firer.cancel_run("producer-dropped").await.is_err());
    firer.started(&wire_id, "late-session").await;
    assert!(matches!(next_frame(&mut socket).await,
        Frame::Event(ClientEvent::CronRunBound { run_id, error: Some(error) })
        if run_id == wire_id && error.starts_with("cancelled:")));
    // The old host handler can lose its result on disconnect. Reconnect must
    // recover that same cached result without admitting another execution.
    connection.close_connection(Some(&sink)).await;
    let (second_endpoint, mut reconnected, second_sink) = captured_sink().await;
    assert!(connection.claim_outbound(&second_sink).await);
    connection.handle_hello(2, hello()).await;
    assert!(matches!(
        next_frame(&mut reconnected).await,
        Frame::Response(_)
    ));
    let Frame::Event(ClientEvent::CronRunRequested { run_id, task }) =
        next_frame(&mut reconnected).await
    else {
        panic!("expected cancellation result recovery")
    };
    assert_eq!(run_id, wire_id);
    let run = &task.automation.unwrap().runs[0];
    assert_eq!(
        run.error.as_deref(),
        Some(crate::cron_host::HOST_CANCEL_REQUESTED)
    );
    assert!(
        run.started_at.is_none(),
        "cancelled before binding must stay unstarted"
    );
    // The cached host completion removes ownership even though the original
    // firing receiver is gone. The next scheduler settlement can release it.
    firer
        .complete(
            &wire_id,
            None,
            None,
            Some("cancelled: Host refused the revoked bind".into()),
        )
        .await;
    firer.cancel_run("producer-dropped").await.unwrap();
    assert!(firer.pending.lock().await.is_empty());
    connection.close_connection(Some(&second_sink)).await;
    drop(socket);
    drop(reconnected);
    first_endpoint.shutdown().await;
    second_endpoint.shutdown().await;
}

#[tokio::test]
async fn cron_binding_and_cancellation_are_serialized() {
    use std::{future::Future, task::Poll};
    let connection = BridgeConnection::new();
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let request = automation_request("binding-race");
    let wire_id = crate::cron_host::transport_run_id(&request);
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await;
    // The actual SDK bind blocks on its durable store lock. While it is parked,
    // cancellation must not get past the claim lock and revoke a cloned input.
    let durable_store = cron::lock_cron_file().await;
    let mut binding = Box::pin(firer.started(&wire_id, "execution-session"));
    std::future::poll_fn(|cx| {
        assert!(binding.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(firer.pending.try_lock().is_err());
    let mut cancellation = Box::pin(firer.cancel_run("binding-race"));
    std::future::poll_fn(|cx| {
        assert!(cancellation.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(durable_store);
    binding.await;
    assert!(cancellation.await.is_err());
    firer
        .complete(
            &wire_id,
            None,
            None,
            Some("cancelled: Host acknowledged stop".into()),
        )
        .await;
    assert!(job.await.unwrap().is_err());
    firer.cancel_run("binding-race").await.unwrap();
}

#[tokio::test]
async fn bound_cron_claim_replays_for_result_recovery_then_retires_on_completion() {
    let connection = BridgeConnection::new();
    let (first_endpoint, mut first_socket, first_sink) = captured_sink().await;
    assert!(connection.claim_outbound(&first_sink).await);
    connection.handle_hello(1, hello()).await;
    assert!(matches!(
        next_frame(&mut first_socket).await,
        Frame::Response(_)
    ));
    let emitted = Arc::new(Notify::new());
    let temp = tempfile::tempdir().unwrap();
    let fs = crate::HostFileSystem::new(temp.path().into());
    let request = automation_request("bound-occurrence");
    let wire_id = crate::cron_host::transport_run_id(&request);
    // Restore the same running claim in the real SDK durable task store.
    let mut task = request.task.clone();
    let dto = crate::cron_host::requested_task(&request);
    task.automation.as_mut().unwrap().runs =
        serde_json::from_value(serde_json::to_value(dto.automation.unwrap().runs).unwrap())
            .unwrap();
    cron::tasks_file::write_automation_tasks_body(
        &fs,
        temp.path(),
        &serde_json::to_string(&serde_json::json!({"tasks":[task]})).unwrap(),
    )
    .await
    .unwrap();
    let firer = Arc::new(HostCronFirer::new(
        Arc::new(ObservedSink {
            inner: connection.event_sink(),
            emitted: emitted.clone(),
        }),
        temp.path().into(),
        connection.cron_requests_handle(),
    ));
    let running = firer.clone();
    let job = tokio::spawn(async move { running.fire_automation(&request).await });
    emitted.notified().await;
    assert!(matches!(
        next_frame(&mut first_socket).await,
        Frame::Event(ClientEvent::CronRunRequested { .. })
    ));
    firer.started(&wire_id, "bound-session").await;
    let bound = next_frame(&mut first_socket).await;
    assert!(
        matches!(&bound,
        Frame::Event(ClientEvent::CronRunBound { run_id, error: None }) if run_id == &wire_id),
        "actual bind acknowledgement: {bound:?}"
    );
    connection.close_connection(Some(&first_sink)).await;

    let (second_endpoint, mut second_socket, second_sink) = captured_sink().await;
    assert!(connection.claim_outbound(&second_sink).await);
    connection.handle_hello(2, hello()).await;
    assert!(matches!(
        next_frame(&mut second_socket).await,
        Frame::Response(_)
    ));
    let Frame::Event(ClientEvent::CronRunRequested { run_id, task }) =
        next_frame(&mut second_socket).await
    else {
        panic!("bound claim must replay for result recovery")
    };
    assert_eq!(run_id, wire_id);
    let run = &task.automation.unwrap().runs[0];
    assert_eq!(run.id, "bound-occurrence");
    assert_eq!(run.session_id.as_deref(), Some("bound-session"));
    assert!(run.started_at.is_some());
    firer
        .complete(
            &wire_id,
            Some("bound-session".into()),
            Some("original result".into()),
            None,
        )
        .await;
    assert_eq!(job.await.unwrap().unwrap().summary, "original result");
    connection.close_connection(Some(&second_sink)).await;
    assert!(connection.claim_outbound(&second_sink).await);
    connection.handle_hello(3, hello()).await;
    assert!(matches!(
        next_frame(&mut second_socket).await,
        Frame::Response(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), second_socket.next())
            .await
            .is_err()
    );
    connection.close_connection(Some(&second_sink)).await;
    drop(first_socket);
    drop(second_socket);
    first_endpoint.shutdown().await;
    second_endpoint.shutdown().await;
}

#[tokio::test]
async fn unowned_question_publication_during_close_finishes_and_is_joined() {
    use std::{future::Future, task::Poll};
    let connection = BridgeConnection::new();
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (question_tx, question_rx) = mpsc::channel(1);
    let connection = connection.bind_ask_user_question(broker.clone(), question_rx);
    let (endpoint, socket, sink) = captured_sink().await;
    // Park the actual broker publisher in FrameEventSink after it marks the
    // request PUBLICATION_IN_FLIGHT. There is deliberately no turn owner.
    let outbound = connection.out.lock().await;
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    question_tx
        .send(AskUserQuestionExchange {
            questions: Vec::new(),
            timeout_secs: None,
            resp_tx: response_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while broker.pending_count().await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut closing = Box::pin(connection.close_connection(None));
    std::future::poll_fn(|cx| {
        assert!(closing.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(connection.connection_admission.try_lock().is_err());
    let mut reconnect = Box::pin(connection.claim_outbound(&sink));
    std::future::poll_fn(|cx| {
        assert!(reconnect.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(outbound);
    tokio::time::timeout(Duration::from_secs(2), closing)
        .await
        .unwrap();
    assert!(response_rx.await.is_err());
    assert_eq!(broker.pending_count().await, 0);
    assert!(connection.question_rejections.lock().unwrap().is_empty());
    assert!(reconnect.await);
    connection.close_connection(Some(&sink)).await;
    drop(question_tx);
    drop(socket);
    endpoint.shutdown().await;
}
