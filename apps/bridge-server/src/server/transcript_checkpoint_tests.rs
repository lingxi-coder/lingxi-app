//! Read-only transcript capture must not own the command lane or a new socket.
use super::connection_regression_test::{
    captured_sink, connect_authenticated, hello, next_frame, Socket,
};
use super::*;
use client::protocol::listings::SessionModeDto;
use futures_util::SinkExt;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

const SESSION: &str = "11111111-2222-4333-8444-555555555555";

#[derive(Default)]
struct HeldCheckpointRouter {
    checkpoint_calls: AtomicUsize,
    ordinary_calls: AtomicUsize,
    snapshot_calls: AtomicUsize,
    entered: Notify,
    release: Notify,
    returned: Notify,
}

fn roster() -> ClientEvent {
    ClientEvent::SessionAgentList {
        session_id: SESSION.into(),
        agents: Vec::new(),
    }
}

#[async_trait]
impl CommandRouter for HeldCheckpointRouter {
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        assert!(matches!(command, ClientCommand::ListSessionAgents));
        self.ordinary_calls.fetch_add(1, Ordering::SeqCst);
        sink.emit(roster()).await;
    }

    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        self.snapshot_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![roster()])
    }

    async fn transcript_checkpoint(&self, session_id: uuid::Uuid) -> Result<ClientEvent, String> {
        assert_eq!(session_id.to_string(), SESSION);
        self.checkpoint_calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        self.returned.notify_one();
        Ok(ClientEvent::SessionResumed {
            session_id: session_id.to_string(),
            mode: SessionModeDto::Code,
            messages: Vec::new(),
        })
    }
}

fn request<P: serde::Serialize>(id: u64, method: &str, params: P) -> Frame {
    Frame::Request(BridgeRequest {
        trace_context: None,
        id,
        method: method.into(),
        params: serialize_source(&params).expect("test request serializes as source JSON"),
    })
}

fn checkpoint_request(id: u64) -> Frame {
    request(
        id,
        "desktop_transcript_checkpoint",
        serde_json::json!({"session_id":SESSION}),
    )
}

async fn send(socket: &mut Socket, frame: Frame) {
    socket
        .send(Message::Text(serde_json::to_string(&frame).unwrap()))
        .await
        .unwrap();
}

async fn response(socket: &mut Socket, id: u64) -> BridgeResponse {
    let Frame::Response(response) = next_frame(socket).await else {
        panic!("checkpoint controls must reply as one correlated response");
    };
    assert_eq!(response.id, id);
    response
}

fn assert_checkpoint(response: BridgeResponse) {
    assert!(response.error.is_none());
    let event = deserialize_source::<ClientEvent>(&response.result.unwrap()).unwrap();
    assert!(
        matches!(event, ClientEvent::SessionResumed { session_id, mode: SessionModeDto::Code, messages }
        if session_id == SESSION && messages.is_empty())
    );
}

async fn wait_for_capture(router: &HeldCheckpointRouter) {
    tokio::time::timeout(Duration::from_secs(2), router.entered.notified())
        .await
        .expect("the admitted checkpoint must reach its read-only router");
}

#[tokio::test]
async fn held_checkpoint_leaves_ordinary_commands_and_runtime_snapshots_responsive() {
    let router = Arc::new(HeldCheckpointRouter::default());
    let connection = Arc::new(BridgeConnection::new().bind_router(router.clone()));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    send(&mut socket, request(1, "hello", hello())).await;
    assert!(response(&mut socket, 1).await.error.is_none());

    send(&mut socket, checkpoint_request(2)).await;
    wait_for_capture(&router).await;
    send(
        &mut socket,
        request(
            3,
            "list_session_agents",
            serde_json::json!({"type":"list_session_agents"}),
        ),
    )
    .await;
    assert!(matches!(next_frame(&mut socket).await, Frame::Event(event) if event == roster()));
    send(
        &mut socket,
        request(
            4,
            "desktop_runtime_snapshot",
            serde_json::json!({"type":"list_session_agents"}),
        ),
    )
    .await;
    let snapshot = response(&mut socket, 4).await;
    assert!(snapshot.error.is_none());
    #[derive(serde::Deserialize)]
    struct Snapshot {
        events: Vec<ClientEvent>,
    }
    let snapshot = deserialize_source::<Snapshot>(&snapshot.result.unwrap()).unwrap();
    assert_eq!(snapshot.events, vec![roster()]);
    assert_eq!(router.ordinary_calls.load(Ordering::SeqCst), 1);
    assert_eq!(router.snapshot_calls.load(Ordering::SeqCst), 1);
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 1);

    // Both replies arrived while no release permit had been supplied.
    router.release.notify_one();
    assert_checkpoint(response(&mut socket, 2).await);
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_reply_cannot_overtake_an_already_queued_live_delivery() {
    use std::{future::Future, task::Poll};
    let (endpoint, mut socket, sink) = captured_sink().await;
    let router = Arc::new(HeldCheckpointRouter::default());
    let connection = BridgeConnection::new().bind_router(router.clone());
    connection
        .on_frame(request(1, "hello", hello()), sink.clone())
        .await;
    assert!(response(&mut socket, 1).await.error.is_none());
    connection
        .on_frame(checkpoint_request(2), sink.clone())
        .await;
    wait_for_capture(&router).await;

    let delivery_gate = connection.out.lock().await;
    let output = connection.event_sink();
    let live = ClientEvent::CoordinatorStatus {
        active_workers: 0,
        team: Some("checkpoint-response-order".into()),
    };
    let mut delivery = Box::pin(output.emit(live.clone()));
    std::future::poll_fn(|cx| {
        assert!(
            delivery.as_mut().poll(cx).is_pending(),
            "the live event must be queued on the held outbound lock first"
        );
        Poll::Ready(())
    })
    .await;
    router.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), router.returned.notified())
        .await
        .expect("capture must finish without the outbound lock");
    // On this current-thread runtime the capture task continues from its
    // notification until it queues the response lock. Yield explicitly before
    // opening the gate; the live sender already owns the earlier FIFO place.
    tokio::task::yield_now().await;
    drop(delivery_gate);
    tokio::time::timeout(Duration::from_secs(2), delivery)
        .await
        .expect("opening the outbound gate must release the live sender");
    assert!(matches!(next_frame(&mut socket).await, Frame::Event(event) if event == live));
    assert_checkpoint(response(&mut socket, 2).await);
    connection.close_connection(Some(&sink)).await;
    drop(socket);
    endpoint.shutdown().await;
}

#[tokio::test]
async fn a_second_checkpoint_is_refused_without_queueing_and_the_permit_is_reusable() {
    let router = Arc::new(HeldCheckpointRouter::default());
    let connection = Arc::new(BridgeConnection::new().bind_router(router.clone()));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    send(&mut socket, request(1, "hello", hello())).await;
    assert!(response(&mut socket, 1).await.error.is_none());
    send(&mut socket, checkpoint_request(2)).await;
    wait_for_capture(&router).await;
    send(&mut socket, checkpoint_request(3)).await;
    let refused = response(&mut socket, 3).await;
    let message = String::from_utf16(&refused.error.unwrap().message.code_units).unwrap();
    assert!(message.contains("in progress"));
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 1);

    router.release.notify_one();
    assert_checkpoint(response(&mut socket, 2).await);
    send(&mut socket, checkpoint_request(4)).await;
    wait_for_capture(&router).await;
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 2);
    router.release.notify_one();
    assert_checkpoint(response(&mut socket, 4).await);
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}

#[tokio::test]
async fn checkpoint_requires_hello_and_rejects_invalid_params_before_router_admission() {
    let router = Arc::new(HeldCheckpointRouter::default());
    let connection = Arc::new(BridgeConnection::new().bind_router(router.clone()));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    send(&mut socket, checkpoint_request(1)).await;
    let refused = response(&mut socket, 1).await;
    let message = String::from_utf16(&refused.error.unwrap().message.code_units).unwrap();
    assert!(message.contains("hello"));
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 0);
    send(&mut socket, request(2, "hello", hello())).await;
    assert!(response(&mut socket, 2).await.error.is_none());
    for (index, params) in [
        serde_json::json!({"session_id":"not-a-uuid"}),
        serde_json::json!({"session_id":SESSION,"extra":true}),
        serde_json::json!({}),
    ]
    .into_iter()
    .enumerate()
    {
        let id = index as u64 + 3;
        send(
            &mut socket,
            request(id, "desktop_transcript_checkpoint", params),
        )
        .await;
        assert!(response(&mut socket, id).await.error.is_some());
    }
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 0);
    send(&mut socket, checkpoint_request(6)).await;
    wait_for_capture(&router).await;
    router.release.notify_one();
    assert_checkpoint(response(&mut socket, 6).await);
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}

#[tokio::test]
async fn checkpoint_late_reply_stays_with_original_socket_across_disconnect_and_reconnect() {
    // Keep the old transport readable after closing its BridgeConnection
    // ownership so the destination of the late reply is directly observable.
    let (old_endpoint, mut old_socket, old_sink) = captured_sink().await;
    let (new_endpoint, mut new_socket, new_sink) = captured_sink().await;
    let router = Arc::new(HeldCheckpointRouter::default());
    let connection = BridgeConnection::new().bind_router(router.clone());
    connection
        .on_frame(request(1, "hello", hello()), old_sink.clone())
        .await;
    assert!(response(&mut old_socket, 1).await.error.is_none());
    tokio::time::timeout(
        Duration::from_secs(2),
        connection.on_frame(checkpoint_request(2), old_sink.clone()),
    )
    .await
    .expect("checkpoint admission must return before the held filesystem read");
    wait_for_capture(&router).await;
    tokio::time::timeout(
        Duration::from_secs(2),
        connection.close_connection(Some(&old_sink)),
    )
    .await
    .expect("a read-only checkpoint must not postpone disconnect cleanup");
    connection
        .on_frame(request(3, "hello", hello()), new_sink.clone())
        .await;
    assert!(response(&mut new_socket, 3).await.error.is_none());
    connection
        .on_frame(checkpoint_request(4), new_sink.clone())
        .await;
    assert!(response(&mut new_socket, 4).await.error.is_some());
    assert_eq!(router.checkpoint_calls.load(Ordering::SeqCst), 1);

    router.release.notify_one();
    assert_checkpoint(response(&mut old_socket, 2).await);
    // Waiting for the old reply first establishes that its send has completed;
    // any accidental publication to the replacement would precede this reply.
    connection
        .on_frame(
            request(
                5,
                "desktop_runtime_snapshot",
                serde_json::json!({"type":"list_session_agents"}),
            ),
            new_sink.clone(),
        )
        .await;
    assert!(response(&mut new_socket, 5).await.error.is_none());
    connection.close_connection(Some(&new_sink)).await;
    drop(old_socket);
    drop(new_socket);
    old_endpoint.shutdown().await;
    new_endpoint.shutdown().await;
}
