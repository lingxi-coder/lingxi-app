//! Session attach / snapshot / live-record handling against a scripted connection.

mod support;

use async_trait::async_trait;
use client::protocol::commands::ImageRefDto;
use remote_control_client::messages::{
    PermissionModeId, RemoteRecord, RemoteSeqRecord, SessionSnapshotParams,
};
use remote_control_client::rpc::{RemoteErrorCode, RpcError};
use remote_control_client::session::{
    OutgoingStatus, RemoteClient, SessionController, SessionIo, SessionStatus,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{advance, settle};

type Handler = Box<dyn FnMut(&str, &Value) -> Result<Value, RpcError> + Send>;

struct FakeIo {
    connected: AtomicBool,
    requests: Mutex<Vec<(String, Value)>>,
    notifications: Mutex<Vec<(String, Value)>>,
    handler: Mutex<Handler>,
}

impl FakeIo {
    fn new(
        handler: impl FnMut(&str, &Value) -> Result<Value, RpcError> + Send + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(true),
            requests: Mutex::default(),
            notifications: Mutex::default(),
            handler: Mutex::new(Box::new(handler)),
        })
    }
    fn calls(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
    fn presence(&self) -> usize {
        self.notifications
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "session.presence")
            .count()
    }
}

#[async_trait]
impl SessionIo for FakeIo {
    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        _t: Option<Duration>,
    ) -> Result<Value, RpcError> {
        let params = params.unwrap_or(Value::Null);
        self.requests
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        (self.handler.lock().unwrap())(method, &params)
    }
    async fn notify(&self, method: &str, params: Option<Value>) {
        self.notifications
            .lock()
            .unwrap()
            .push((method.to_owned(), params.unwrap_or(Value::Null)));
    }
    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

fn record(seq: u64, record: Value) -> RemoteSeqRecord {
    serde_json::from_value(json!({ "seq": seq, "record": record })).unwrap()
}

fn prompt(seq: u64, turn: u64) -> RemoteSeqRecord {
    record(
        seq,
        json!({ "type": "prompt", "turn_id": turn, "text": "hello", "images": [], "origin": "device" }),
    )
}

fn event(seq: u64, event: Value) -> RemoteSeqRecord {
    record(seq, json!({ "type": "event", "event": event }))
}

fn snapshot(parts: Vec<Value>) -> Vec<SessionSnapshotParams> {
    parts
        .into_iter()
        .map(|p| serde_json::from_value(p).unwrap())
        .collect()
}

fn full_snapshot(epoch: &str, first: u64, seq: u64) -> Vec<SessionSnapshotParams> {
    snapshot(vec![
        json!({ "phase": "begin", "session_id": "s1", "epoch": epoch, "first_message_index": first }),
        json!({ "phase": "messages", "session_id": "s1", "messages": [{ "role": "user", "blocks": [] }, { "role": "assistant", "blocks": [] }] }),
        json!({ "phase": "records", "session_id": "s1", "records": [
            { "seq": 2, "record": { "type": "prompt", "turn_id": 7, "text": "x", "images": [], "origin": "desktop" } },
            { "seq": 1, "record": { "type": "event", "event": { "type": "turn_started", "turn_id": 7 } } },
        ] }),
        json!({ "phase": "end", "session_id": "s1", "cursor": { "epoch": epoch, "seq": seq },
                 "pending": { "permissions": [], "questions": [], "computer_access": [] },
                 "turn": { "running": true, "turn_id": 7 } }),
    ])
}

fn feed(controller: &SessionController, parts: Vec<SessionSnapshotParams>) {
    for part in parts {
        controller.handle_snapshot(part);
    }
}

fn attach_snapshot(
    epoch: &'static str,
) -> impl FnMut(&str, &Value) -> Result<Value, RpcError> + Send {
    move |method, _| match method {
        "session.attach" => Ok(json!({ "mode": "snapshot", "epoch": epoch })),
        _ => Ok(json!({})),
    }
}

#[tokio::test(start_paused = true)]
async fn overflowing_live_records_during_a_snapshot_requests_a_fresh_base() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io.clone());
    controller.activate();
    settle().await;
    let mut parts = full_snapshot("e1", 0, 2).into_iter();
    controller.handle_snapshot(parts.next().unwrap());
    for _ in 0..5000 {
        controller.handle_event(prompt(3, 3));
    }
    controller.handle_event(prompt(4, 4));
    feed(&controller, parts.collect());
    settle().await;
    assert_eq!(
        controller.state().status,
        SessionStatus::Loading,
        "an incomplete event range cannot be live"
    );
    assert_eq!(
        io.calls("session.attach"),
        vec![json!({ "session_id": "s1" }), json!({ "session_id": "s1" })]
    );
    // The new full snapshot covers every record we could not retain.
    feed(&controller, full_snapshot("e1", 0, 4));
    controller.handle_event(prompt(10, 10)); // Sequence numbers can legitimately be sparse.
    assert_eq!(controller.state().status, SessionStatus::Live);
    assert_eq!(controller.state().records.last().unwrap().seq, 10);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_covering_overflowed_records_does_not_attach_again() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io.clone());
    controller.activate();
    settle().await;
    let mut parts = full_snapshot("e1", 0, 4).into_iter();
    controller.handle_snapshot(parts.next().unwrap());
    for _ in 0..5000 {
        controller.handle_event(prompt(3, 3));
    }
    controller.handle_event(prompt(4, 4));
    feed(&controller, parts.collect());
    settle().await;
    assert_eq!(controller.state().status, SessionStatus::Live);
    assert_eq!(io.calls("session.attach").len(), 1);
    controller.handle_event(prompt(10, 10));
    assert_eq!(controller.state().records.last().unwrap().seq, 10);
}

#[tokio::test(start_paused = true)]
async fn overflowing_live_records_during_a_delta_attach_retries_without_a_cursor() {
    let controller_slot = Arc::new(Mutex::new(None::<SessionController>));
    let slot = controller_slot.clone();
    let mut attaches = 0;
    let io = FakeIo::new(move |method, _| {
        assert_eq!(method, "session.attach");
        attaches += 1;
        if attaches == 2 {
            let controller = slot.lock().unwrap().clone().unwrap();
            for _ in 0..5000 {
                controller.handle_event(prompt(3, 3));
            }
            controller.handle_event(prompt(4, 4));
            Ok(
                json!({ "mode": "delta", "epoch": "e1", "pending": { "permissions": [], "questions": [], "computer_access": [] } }),
            )
        } else {
            Ok(json!({ "mode": "snapshot", "epoch": "e1" }))
        }
    });
    let controller = SessionController::new("s1", io.clone());
    *controller_slot.lock().unwrap() = Some(controller.clone());
    controller.activate();
    settle().await;
    feed(&controller, full_snapshot("e1", 0, 2));
    controller.connection_lost();
    controller.connection_ready();
    settle().await;
    assert_eq!(
        io.calls("session.attach"),
        vec![
            json!({ "session_id": "s1" }),
            json!({ "session_id": "s1", "resume": { "epoch": "e1", "after_seq": 2 } }),
            json!({ "session_id": "s1" }),
        ]
    );
    assert_eq!(controller.state().status, SessionStatus::Loading);
    feed(&controller, full_snapshot("e1", 0, 4));
    assert_eq!(controller.state().status, SessionStatus::Live);
    controller_slot.lock().unwrap().take();
}

#[tokio::test(start_paused = true)]
async fn attach_assembles_a_snapshot_and_holds_live_events() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io.clone());
    controller.activate();
    settle().await;
    assert_eq!(
        io.calls("session.attach"),
        vec![json!({ "session_id": "s1" })]
    );
    assert_eq!(controller.state().status, SessionStatus::Loading);

    // Live events while the snapshot is still coming are held; one is already covered by it.
    let mut parts = full_snapshot("e1", 3, 2).into_iter();
    controller.handle_snapshot(parts.next().unwrap());
    controller.handle_event(prompt(3, 8));
    controller.handle_event(prompt(2, 7)); // covered
    controller.handle_event(event(4, json!({ "type": "turn_ended", "outcome": { "type": "end_turn" }, "cost": {
        "total_usd": 0.0, "input_tokens": 0, "output_tokens": 0, "api_calls": 0, "session_duration_secs": 0, "formatted": "$0" } })));
    for part in parts {
        controller.handle_snapshot(part);
    }
    settle().await;
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Live);
    assert_eq!(state.first_message_index, 3);
    assert!(state.has_older);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(
        state.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4],
        "sorted, with the held ones after, the covered one skipped"
    );
    assert!(!state.turn.running, "turn_ended after turn_started");
    assert_eq!(io.presence(), 1);

    // Live from now on.
    controller.handle_event(prompt(5, 9));
    assert_eq!(controller.state().records.len(), 5);
    controller.handle_event(prompt(5, 9));
    assert_eq!(
        controller.state().records.len(),
        5,
        "duplicates are ignored"
    );
}

#[tokio::test(start_paused = true)]
async fn reconnect_resumes_with_a_delta_or_starts_over() {
    let reply = Arc::new(Mutex::new(
        json!({ "mode": "delta", "epoch": "e1", "pending": { "permissions": [], "questions": [], "computer_access": [] } }),
    ));
    let reply2 = Arc::clone(&reply);
    let io = FakeIo::new(move |method, params| match method {
        // Without a resume point the host can only send a snapshot.
        "session.attach" if params.get("resume").is_none() => {
            Ok(json!({ "mode": "snapshot", "epoch": "e9" }))
        }
        "session.attach" => Ok(reply2.lock().unwrap().clone()),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));
    // (a snapshot without an attach still sets the cursor)
    controller.activate();
    settle().await;
    assert_eq!(
        io.calls("session.attach")[0],
        json!({ "session_id": "s1", "resume": { "epoch": "e1", "after_seq": 2 } })
    );
    assert_eq!(controller.state().status, SessionStatus::Live);

    controller.connection_lost();
    assert_eq!(controller.state().status, SessionStatus::Offline);
    controller.connection_ready();
    settle().await;
    assert_eq!(controller.state().status, SessionStatus::Live);

    // A delta for another epoch cannot be applied: attach again without resuming.
    *reply.lock().unwrap() = json!({ "mode": "delta", "epoch": "other", "pending": {} });
    let attaches_before = io.calls("session.attach").len();
    controller.connection_lost();
    controller.connection_ready();
    settle().await;
    let calls = io.calls("session.attach");
    assert_eq!(calls.len(), attaches_before + 2);
    assert!(calls.last().unwrap().get("resume").is_none());
}

#[tokio::test(start_paused = true)]
async fn attach_errors() {
    let io = FakeIo::new(|_, _| {
        Err(RpcError::new(
            RemoteErrorCode::UnknownSession,
            "no such session",
        ))
    });
    let controller = SessionController::new("s1", io);
    controller.activate();
    settle().await;
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Error);
    assert_eq!(state.error.unwrap().code, "unknown_session");

    let io = FakeIo::new(|_, _| Err(RpcError::new(RemoteErrorCode::Closed, "gone")));
    let controller = SessionController::new("s1", io);
    controller.activate();
    settle().await;
    assert_eq!(controller.state().status, SessionStatus::Offline);
}

#[tokio::test(start_paused = true)]
async fn reset_waits_for_a_new_snapshot_then_asks_for_one() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io.clone());
    controller.activate();
    settle().await;
    feed(&controller, full_snapshot("e1", 0, 2));
    assert_eq!(controller.state().status, SessionStatus::Live);
    controller.handle_reset();
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Loading);
    assert!(state.records.is_empty() && state.messages.is_empty());
    advance(Duration::from_secs(2)).await;
    assert_eq!(io.calls("session.attach").len(), 1);
    advance(Duration::from_secs(2)).await;
    let calls = io.calls("session.attach");
    assert_eq!(calls.len(), 2);
    assert!(calls[1].get("resume").is_none(), "the cursor was dropped");

    // A snapshot that arrives by itself cancels the wait.
    controller.handle_reset();
    feed(&controller, full_snapshot("e2", 0, 2));
    advance(Duration::from_secs(5)).await;
    assert_eq!(io.calls("session.attach").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn closed_session() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io);
    controller.handle_closed();
    assert_eq!(controller.state().status, SessionStatus::Closed);
}

#[tokio::test(start_paused = true)]
async fn prompts_are_tracked_until_their_record_returns() {
    let attempts = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&attempts);
    let io = FakeIo::new(move |method, params| match method {
        "prompt.send" => {
            let mut n = counter.lock().unwrap();
            *n += 1;
            if *n == 1 {
                Err(RpcError::new(RemoteErrorCode::Capacity, "busy"))
            } else {
                assert!(params["client_request_id"].is_string());
                Ok(json!({ "turn_id": 11 }))
            }
        }
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));

    controller.send_prompt("   ", vec![]).await;
    assert!(
        controller.state().outgoing.is_empty(),
        "empty prompts are not sent"
    );

    controller.send_prompt("do it", vec![]).await;
    let state = controller.state();
    assert_eq!(state.outgoing.len(), 1);
    assert_eq!(state.outgoing[0].status, OutgoingStatus::Failed);
    assert_eq!(state.outgoing[0].error.as_deref(), Some("capacity"));
    let id = state.outgoing[0].id.clone();

    controller.retry_prompt(&id).await;
    let state = controller.state();
    assert_eq!(state.outgoing[0].status, OutgoingStatus::Sending);
    assert_eq!(state.outgoing[0].turn_id, Some(11));
    let sent = io.calls("prompt.send");
    assert_eq!(
        sent[0]["client_request_id"], sent[1]["client_request_id"],
        "a retry is a repeat"
    );

    // The record comes back: the outgoing entry goes away.
    controller.handle_event(prompt(3, 11));
    assert!(controller.state().outgoing.is_empty());
}

#[tokio::test(start_paused = true)]
async fn images_are_filtered_and_dropped_while_a_turn_runs() {
    let io = FakeIo::new(|method, _| match method {
        "prompt.send" => Ok(json!({ "turn_id": 1 })),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    let image = |t: &str| ImageRefDto {
        media_type: t.into(),
        base64: "AAAA".into(),
    };
    // Idle: supported types only, at most four.
    controller
        .send_prompt(
            "look",
            vec![
                image("image/png"),
                image("image/bmp"),
                image("image/jpeg"),
                image("image/gif"),
                image("image/webp"),
                image("image/png"),
            ],
        )
        .await;
    assert_eq!(
        io.calls("prompt.send")[0]["images"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // While a turn runs (from the snapshot): text only.
    feed(&controller, full_snapshot("e1", 0, 2));
    assert!(controller.state().turn.running);
    controller
        .send_prompt("again", vec![image("image/png")])
        .await;
    assert!(io.calls("prompt.send")[1].get("images").is_none());
    // Only images and no text, while running: nothing to send.
    controller.send_prompt("", vec![image("image/png")]).await;
    assert_eq!(io.calls("prompt.send").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn losing_an_answer_race_is_quiet() {
    let io = FakeIo::new(|method, _| match method {
        "permission.respond" => Err(RpcError::code(RemoteErrorCode::NotPending)),
        "question.answer" => Err(RpcError::code(RemoteErrorCode::Internal)),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    controller.respond_permission(4, true).await.unwrap();
    assert!(controller.state().answered_elsewhere);
    assert_eq!(
        io.calls("permission.respond")[0]["decision"],
        json!({ "type": "allow_once" })
    );
    controller.handle_pending(Default::default());
    assert!(
        !controller.state().answered_elsewhere,
        "the next change clears the note"
    );
    controller.respond_permission(4, false).await.unwrap();
    assert_eq!(
        io.calls("permission.respond")[1]["decision"],
        json!({ "type": "deny" })
    );
    // Other errors are the caller's.
    let error = controller
        .answer_question(1, Default::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Internal);
}

#[tokio::test(start_paused = true)]
async fn controls_and_commands() {
    let io = FakeIo::new(|_, _| Ok(json!({})));
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));
    controller.cancel_turn().await.unwrap();
    assert_eq!(
        io.calls("turn.cancel")[0],
        json!({ "session_id": "s1", "turn_id": 7 })
    );
    controller.set_model("m").await.unwrap();
    controller
        .set_permission_mode(PermissionModeId::AcceptEdits)
        .await
        .unwrap();
    assert_eq!(
        io.calls("controls.set_permission_mode")[0]["mode"],
        "acceptEdits"
    );
    let refused = controller
        .set_permission_mode(PermissionModeId::BypassPermissions)
        .await
        .unwrap_err();
    assert_eq!(refused.code, RemoteErrorCode::Forbidden);
    assert_eq!(
        io.calls("controls.set_permission_mode").len(),
        1,
        "never sent"
    );
}

#[tokio::test(start_paused = true)]
async fn older_messages_are_prepended() {
    let io = FakeIo::new(|method, params| match method {
        "session.history" => {
            assert_eq!(params["before_index"], 3);
            Ok(
                json!({ "messages": [{ "role": "user", "blocks": [] }], "first_index": 2, "has_more": true }),
            )
        }
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io);
    feed(&controller, full_snapshot("e1", 3, 2));
    controller.load_older().await;
    let state = controller.state();
    assert_eq!(
        (
            state.messages.len(),
            state.first_message_index,
            state.has_older,
            state.loading_older
        ),
        (3, 2, true, false)
    );
}

#[tokio::test(start_paused = true)]
async fn images_are_fetched_once_and_validated() {
    let io = FakeIo::new(|method, params| match (method, params["ref"].as_str()) {
        ("image.get", Some("good")) => Ok(json!({ "media_type": "image/png", "base64": "QUJD" })),
        ("image.get", Some("evil")) => Ok(json!({ "media_type": "text/html", "base64": "QUJD" })),
        ("image.get", Some("junk")) => Ok(json!({ "media_type": "image/png", "base64": "QU<JD" })),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    assert_eq!(
        controller.fetch_image("good").await.unwrap(),
        "data:image/png;base64,QUJD"
    );
    controller.fetch_image("good").await.unwrap();
    assert_eq!(io.calls("image.get").len(), 1);
    assert!(controller.fetch_image("evil").await.is_err());
    assert!(controller.fetch_image("junk").await.is_err());
}

#[tokio::test(start_paused = true)]
async fn the_client_routes_notifications_and_lists_sessions() {
    let io = FakeIo::new(|method, _| match method {
        "sessions.watch" => Ok(json!({ "sessions": [{
            "session_id": "s1", "title": "A\u{202E}title", "project_label": "p", "status": "running", "viewers": 1 }] })),
        "session.attach" => Ok(json!({ "mode": "snapshot", "epoch": "e1" })),
        "models.list" => Ok(json!({ "models": ["m1"], "current": "m1" })),
        _ => Ok(json!({})),
    });
    let client = RemoteClient::new(io.clone());
    let router = client.connection_listener();
    let controller = client.session("s1");
    assert_eq!(client.sessions(), None);

    router.on_ready();
    settle().await;
    let sessions = client.sessions().unwrap();
    assert_eq!(sessions[0].title, "Atitle", "bidi controls removed");

    controller.activate();
    settle().await;
    for part in full_snapshot("e1", 0, 2) {
        router.on_notification("session.snapshot", &serde_json::to_value(part).unwrap());
    }
    assert_eq!(controller.state().status, SessionStatus::Live);
    router.on_notification("session.event", &json!({ "session_id": "s1", "seq": 3,
        "record": { "type": "prompt", "turn_id": 9, "text": "t", "images": [], "origin": "desktop" } }));
    assert_eq!(controller.state().records.len(), 3);
    router.on_notification(
        "session.event",
        &json!({ "session_id": "other", "seq": 1,
        "record": { "type": "future_record" } }),
    );
    router.on_notification("sessions.changed", &json!({ "sessions": [] }));
    assert_eq!(client.sessions().unwrap().len(), 0);

    // Models are asked once.
    client.models().await.unwrap();
    client.models().await.unwrap();
    assert_eq!(io.calls("models.list").len(), 1);

    // A record type from a newer host does not break anything.
    router.on_notification(
        "session.event",
        &json!({ "session_id": "s1", "seq": 4,
        "record": { "type": "future_record", "x": 1 } }),
    );
    let state = controller.state();
    assert!(matches!(
        state.records.last().unwrap().record,
        RemoteRecord::Unsupported { .. }
    ));
}

#[tokio::test]
async fn configuration_commands_complete_without_a_prompt_echo() {
    let io = FakeIo::new(|_, _| Ok(json!({ "turn_id": 17 })));
    let controller = SessionController::new("s1", io);
    for text in [
        "/model",
        "/model chosen",
        "/effort high",
        "/plan",
        "/plan open",
        "/plan share",
    ] {
        controller.send_prompt(text, vec![]).await;
        assert!(
            controller.state().outgoing.is_empty(),
            "{text} has no prompt echo"
        );
    }
    controller.send_prompt("/plan implement this", vec![]).await;
    assert_eq!(controller.state().outgoing.len(), 1);
}

#[tokio::test]
async fn an_old_history_response_cannot_prepend_to_a_new_snapshot() {
    struct DelayedHistory(tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Value>>>);
    #[async_trait]
    impl SessionIo for DelayedHistory {
        async fn request(
            &self,
            method: &str,
            _: Option<Value>,
            _: Option<Duration>,
        ) -> Result<Value, RpcError> {
            assert_eq!(method, "session.history");
            Ok(self.0.lock().await.take().unwrap().await.unwrap())
        }
        async fn notify(&self, _: &str, _: Option<Value>) {}
        fn connected(&self) -> bool {
            false
        }
    }
    let (response, receiver) = tokio::sync::oneshot::channel();
    let controller = SessionController::new(
        "s1",
        Arc::new(DelayedHistory(tokio::sync::Mutex::new(Some(receiver)))),
    );
    feed(&controller, full_snapshot("old", 100, 2));
    let reader = controller.clone();
    let pending = tokio::spawn(async move { reader.load_older().await });
    while !controller.state().loading_older {
        tokio::task::yield_now().await;
    }
    controller.handle_reset();
    feed(&controller, full_snapshot("new", 100, 2));
    let expected = controller.state();
    response.send(json!({ "messages": [{ "role": "user", "blocks": [] }], "first_index": 99, "has_more": true })).unwrap();
    pending.await.unwrap();
    assert_eq!(controller.state(), expected);
}

#[tokio::test]
async fn a_queued_attach_does_not_reopen_a_deactivated_session() {
    let io = FakeIo::new(attach_snapshot("e1"));
    let controller = SessionController::new("s1", io.clone());
    controller.activate();
    controller.deactivate();
    settle().await;
    assert!(io.calls("session.attach").is_empty());
}

#[tokio::test]
async fn image_cache_is_scoped_to_the_snapshot_epoch() {
    let io = FakeIo::new(|method, _| {
        assert_eq!(method, "image.get");
        Ok(json!({ "media_type": "image/png", "base64": "AQID" }))
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("old", 0, 2));
    controller.fetch_image("s1:m:0:0").await.unwrap();
    controller.fetch_image("s1:m:0:0").await.unwrap();
    assert_eq!(io.calls("image.get").len(), 1);
    feed(&controller, full_snapshot("new", 0, 2));
    controller.fetch_image("s1:m:0:0").await.unwrap();
    assert_eq!(io.calls("image.get").len(), 2);
}

#[tokio::test]
async fn an_image_response_from_an_old_epoch_is_not_returned_or_cached() {
    struct DelayedImage {
        started: tokio::sync::Notify,
        response: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Value>>>,
    }
    #[async_trait]
    impl SessionIo for DelayedImage {
        async fn request(
            &self,
            _: &str,
            _: Option<Value>,
            _: Option<Duration>,
        ) -> Result<Value, RpcError> {
            self.started.notify_one();
            Ok(self.response.lock().await.take().unwrap().await.unwrap())
        }
        async fn notify(&self, _: &str, _: Option<Value>) {}
        fn connected(&self) -> bool {
            false
        }
    }
    let (response, receiver) = tokio::sync::oneshot::channel();
    let io = Arc::new(DelayedImage {
        started: tokio::sync::Notify::new(),
        response: tokio::sync::Mutex::new(Some(receiver)),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("old", 0, 2));
    let reader = controller.clone();
    let fetch = tokio::spawn(async move { reader.fetch_image("s1:m:0:0").await });
    io.started.notified().await;
    feed(&controller, full_snapshot("new", 0, 2));
    response
        .send(json!({ "media_type": "image/png", "base64": "AQID" }))
        .unwrap();
    assert_eq!(
        fetch.await.unwrap().unwrap_err().code,
        RemoteErrorCode::Closed
    );
}
