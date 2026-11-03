//! Session attach / snapshot / live-record handling against a scripted connection.

mod support;

use async_trait::async_trait;
use client::protocol::commands::ImageRefDto;
use client::protocol::listings::SessionModeDto;
use remote_control_client::messages::{
    PermissionModeId, RemoteRecord, RemoteSeqRecord, SessionSnapshotParams,
};
use remote_control_client::rpc::{RemoteErrorCode, RpcError};
use remote_control_client::session::{
    OutgoingStatus, RemoteClient, SessionController, SessionIo, SessionListener, SessionState,
    SessionStatus,
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
        json!({ "phase": "begin", "session_id": "s1", "epoch": epoch, "first_message_index": first, "session_mode": "code" }),
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

#[tokio::test]
async fn snapshot_and_live_events_preserve_the_remote_session_mode() {
    let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = Some(SessionModeDto::Chat);
    } else {
        panic!("expected snapshot begin");
    }
    feed(&controller, parts);
    assert_eq!(controller.state().session_mode, Some(SessionModeDto::Chat));
    controller.handle_event(event(
        3,
        json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
    ));
    controller.handle_event(event(
        4,
        json!({"type":"session_resumed", "session_id":"s1", "mode":"chat", "messages":[]}),
    ));
    let before = controller.state();
    assert_eq!(controller.state().session_mode, Some(SessionModeDto::Chat));
    controller.handle_event(event(
        5,
        json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
    ));
    let after = controller.state();
    assert_eq!(after.status, SessionStatus::Error);
    assert_eq!(after.session_mode, Some(SessionModeDto::Chat));
    assert_eq!(after.records, before.records);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.first_message_index, before.first_message_index);
}

#[tokio::test]
async fn foreign_or_mixed_epoch_snapshots_cannot_replace_the_current_owner() {
    let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
    feed(&controller, full_snapshot("e1", 8, 2));
    let before = controller.state();
    controller.handle_snapshot(
        snapshot(vec![json!({"phase":"begin", "session_id":"s2", "epoch":"e2", "first_message_index":0, "session_mode":"chat"})])
            .pop().unwrap(),
    );
    assert_eq!(controller.state(), before);
    let mut parts = full_snapshot("e2", 0, 2);
    if let SessionSnapshotParams::End { cursor, .. } = parts.last_mut().unwrap() {
        cursor.epoch = "e3".into();
    }
    feed(&controller, parts);
    let after = controller.state();
    assert_eq!(after.status, SessionStatus::Loading);
    assert_eq!(after.session_mode, before.session_mode);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.records, before.records);
}

#[tokio::test]
async fn conflicting_snapshot_records_or_held_events_do_not_replace_the_base() {
    for held_conflict in [false, true] {
        let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
        feed(&controller, full_snapshot("e1", 8, 2));
        let before = controller.state();
        let mut next = full_snapshot("e2", 0, 2);
        let conflicting = event(
            3,
            json!({"type":"session_resumed", "session_id":"s1", "mode":"chat", "messages":[]}),
        );
        if !held_conflict {
            if let SessionSnapshotParams::Records { records, .. } = &mut next[2] {
                records.push(conflicting.clone());
            } else {
                panic!("expected records");
            }
        }
        let mut parts = next.into_iter();
        controller.handle_snapshot(parts.next().unwrap());
        if held_conflict {
            controller.handle_event(prompt(4, 44));
            controller.handle_event(conflicting);
        }
        feed(&controller, parts.collect());
        let rejected = controller.state();
        assert_eq!(rejected.status, SessionStatus::Error);
        assert_eq!(rejected.session_mode, before.session_mode);
        assert_eq!(rejected.messages, before.messages);
        assert_eq!(rejected.records, before.records);
        assert_eq!(rejected.first_message_index, before.first_message_index);
        assert_eq!(
            rejected.error.as_ref().unwrap().code,
            "SESSION_MODE_MISMATCH"
        );

        // A matching snapshot, live record, reset, or pending update cannot
        // clear a conflict or mutate the old transcript behind the error.
        feed(&controller, full_snapshot("e3", 0, 5));
        controller.handle_event(prompt(6, 6));
        controller.handle_reset();
        controller.handle_pending(Default::default());
        assert_eq!(controller.state(), rejected);
    }
}

#[tokio::test]
async fn snapshot_lifecycle_records_can_confirm_a_missing_mode_but_must_agree() {
    for conflict in [false, true] {
        let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
        let mut parts = full_snapshot("e1", 0, 2);
        if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
            *session_mode = None;
        }
        if let SessionSnapshotParams::Records { records, .. } = &mut parts[2] {
            records.push(event(
                1,
                json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
            ));
            if conflict {
                records.push(event(2, json!({"type":"session_resumed", "session_id":"s1", "mode":"code", "messages":[]})));
            }
        }
        feed(&controller, parts);
        let state = controller.state();
        if conflict {
            assert_eq!(state.status, SessionStatus::Error);
            assert_eq!(state.session_mode, None);
            assert!(state.messages.is_empty());
            assert!(state.records.is_empty());
        } else {
            assert_eq!(state.status, SessionStatus::Live);
            assert_eq!(state.session_mode, Some(SessionModeDto::Chat));
        }
    }
}

#[tokio::test]
async fn unknown_or_conflicting_mode_cannot_send_commands_or_retries() {
    let io = FakeIo::new(|method, _| match method {
        "prompt.send" => Err(RpcError::code(RemoteErrorCode::Capacity)),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = None;
    }
    feed(&controller, parts);
    assert_eq!(controller.state().status, SessionStatus::Loading);
    assert_eq!(
        controller
            .send_prompt("not yet", vec![])
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert!(controller.state().outgoing.is_empty());
    assert_eq!(
        controller.cancel_turn().await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .respond_permission(4, true)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .answer_question(4, Default::default())
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.cancel_question(4).await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.set_model("m").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .set_permission_mode(PermissionModeId::AcceptEdits)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.fetch_image("s1:m:0:0").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert!(io.requests.lock().unwrap().is_empty());

    controller.handle_event(event(
        3,
        json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
    ));
    assert_eq!(controller.state().status, SessionStatus::Live);
    controller.send_prompt("retry later", vec![]).await.unwrap();
    let prompt = controller.state().outgoing[0].clone();
    assert_eq!(prompt.status, OutgoingStatus::Failed);
    assert_eq!(io.calls("prompt.send").len(), 1);
    controller.handle_event(event(
        4,
        json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
    ));
    assert_eq!(controller.state().status, SessionStatus::Error);
    assert_eq!(
        controller.retry_prompt(&prompt.id).await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .send_prompt("never sent", vec![])
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.set_model("m").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .respond_permission(4, false)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(io.calls("prompt.send").len(), 1);
    assert!(io.calls("controls.set_model").is_empty());
    assert!(io.calls("permission.respond").is_empty());
    assert_eq!(
        controller.state().outgoing[0].status,
        OutgoingStatus::Failed
    );
}

#[tokio::test]
async fn image_requests_wait_until_the_conversation_mode_is_confirmed() {
    let io = FakeIo::new(|method, _| {
        assert_eq!(method, "image.get");
        Ok(json!({ "media_type": "image/png", "base64": "AQID" }))
    });
    let controller = SessionController::new("s1", io.clone());
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = None;
    }
    feed(&controller, parts);
    assert_eq!(
        controller.fetch_image("s1:m:0:0").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert!(io.calls("image.get").is_empty());
    controller.handle_event(event(
        3,
        json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
    ));
    assert_eq!(
        controller.fetch_image("s1:m:0:0").await.unwrap(),
        "data:image/png;base64,AQID"
    );
    assert_eq!(io.calls("image.get").len(), 1);
}

#[tokio::test]
async fn mode_less_live_records_are_held_until_confirmation_and_replayed_in_order() {
    let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = None;
    }
    feed(&controller, parts);
    let before = controller.state();
    controller.handle_event(prompt(3, 3));
    controller.handle_event(prompt(4, 4));
    assert_eq!(
        controller.state(),
        before,
        "unconfirmed records stay private"
    );
    controller.handle_event(event(
        5,
        json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
    ));
    let admitted = controller.state();
    assert_eq!(admitted.status, SessionStatus::Live);
    assert_eq!(admitted.session_mode, Some(SessionModeDto::Chat));
    assert_eq!(
        admitted
            .records
            .iter()
            .map(|item| item.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
}

#[tokio::test]
async fn a_buffered_burst_publishes_one_consolidated_replay() {
    struct Listener(Mutex<Vec<SessionState>>);
    impl SessionListener for Listener {
        fn on_session_state(&self, state: &SessionState) {
            self.0.lock().unwrap().push(state.clone());
        }
    }
    let listener = Arc::new(Listener(Mutex::new(Vec::new())));
    let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
    controller.add_listener(listener.clone());
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = None;
    }
    feed(&controller, parts);
    for seq in 3..=102 {
        controller.handle_event(prompt(seq, seq));
    }
    controller.handle_event(event(
        103,
        json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
    ));
    settle().await;
    let states = listener.0.lock().unwrap();
    let live: Vec<_> = states
        .iter()
        .filter(|state| state.status == SessionStatus::Live)
        .collect();
    assert_eq!(
        live.len(),
        2,
        "base and one complete replay, not a full transcript clone per record"
    );
    assert_eq!(live.last().unwrap().records.len(), 103);
}

#[tokio::test]
async fn unknown_mode_pending_updates_are_coalesced_and_published_after_confirmation() {
    struct Listener(Mutex<Vec<SessionState>>);
    impl SessionListener for Listener {
        fn on_session_state(&self, state: &SessionState) {
            self.0.lock().unwrap().push(state.clone());
        }
    }
    let listener = Arc::new(Listener(Mutex::new(Vec::new())));
    let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
    controller.add_listener(listener.clone());
    let mut parts = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
        *session_mode = None;
    }
    feed(&controller, parts);
    settle().await;
    assert!(controller.state().messages.is_empty());
    assert!(controller.state().records.is_empty());
    assert!(
        listener
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|state| state.messages.is_empty() && state.records.is_empty()),
        "unknown-mode listeners cannot receive a staged transcript"
    );
    let delivered_before = listener.0.lock().unwrap().len();
    for request_id in 1..=100 {
        controller.handle_pending(
            serde_json::from_value(json!({
                "permissions":[{"request_id":request_id,"kind":{"type":"tool_use_confirm",
                    "default_allow":false,"tool_name":"Bash","tool_input_json":"{}"}}],
                "questions":[],"computer_access":[]
            }))
            .unwrap(),
        );
    }
    settle().await;
    assert_eq!(
        listener.0.lock().unwrap().len(),
        delivered_before,
        "pending updates cannot enqueue repeated copies of the unknown base"
    );
    controller.handle_event(event(
        3,
        json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
    ));
    settle().await;
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Live);
    assert_eq!(state.pending.permissions[0].request_id, 100);
}

#[tokio::test]
async fn unconfirmed_records_are_bounded_and_overflow_is_sticky() {
    for too_many in [true, false] {
        let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
        let mut parts = full_snapshot("e1", 0, 2);
        if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
            *session_mode = None;
        }
        feed(&controller, parts);
        let base = controller.state();
        if too_many {
            for seq in 3..=131 {
                controller.handle_event(prompt(seq, seq));
            }
        } else {
            controller.handle_event(record(
                3,
                json!({"type":"prompt", "turn_id":3, "text":"x".repeat(4 * 1024 * 1024), "images":[], "origin":"device"}),
            ));
        }
        let rejected = controller.state();
        assert_eq!(rejected.status, SessionStatus::Error);
        assert_eq!(
            rejected.error.as_ref().unwrap().code,
            "SESSION_MODE_OVERFLOW"
        );
        assert_eq!(rejected.records, base.records);
        controller.handle_event(event(
            132,
            json!({"type":"session_started", "session_id":"s1", "mode":"code"}),
        ));
        assert_eq!(controller.state(), rejected);
        assert_eq!(
            controller
                .send_prompt("not sent", vec![])
                .await
                .unwrap_err()
                .code,
            RemoteErrorCode::Closed
        );
    }
}

#[tokio::test]
async fn mode_less_snapshot_pages_and_pending_updates_share_the_wait_budget() {
    for oversized_page in [true, false] {
        let controller = SessionController::new("s1", FakeIo::new(attach_snapshot("e1")));
        controller.handle_snapshot(
            snapshot(vec![json!({
                "phase":"begin", "session_id":"s1", "epoch":"e1", "first_message_index":0
            })])
            .pop()
            .unwrap(),
        );
        if oversized_page {
            controller.handle_snapshot(
                snapshot(vec![json!({
                    "phase":"records", "session_id":"s1", "records":[{
                        "seq":1, "record":{"type":"prompt", "turn_id":1,
                            "text":"x".repeat(4 * 1024 * 1024), "origin":"device"}
                    }]
                })])
                .pop()
                .unwrap(),
            );
        } else {
            for _ in 0..129 {
                controller.handle_pending(Default::default());
            }
        }
        let state = controller.state();
        assert_eq!(state.status, SessionStatus::Error);
        assert_eq!(state.error.as_ref().unwrap().code, "SESSION_MODE_OVERFLOW");
        assert!(state.records.is_empty());
        assert!(state.messages.is_empty());
    }
}

fn team_snapshot(controller: &SessionController, id: &str) {
    feed(
        controller,
        snapshot(vec![
            json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":0}),
            json!({"phase":"end", "session_id":id, "cursor":{"epoch":"e1", "seq":0},
            "pending":{"permissions":[{"request_id":1,"suppress_always_allow_rule":true,
                "kind":{"type":"tool_use_confirm","default_allow":false,"tool_name":"Bash","tool_input_json":"{}"}}],
                "questions":[],"computer_access":[]}, "turn":{"running":false}}),
        ]),
    );
}

#[tokio::test]
async fn team_approval_capability_has_only_permission_response_authority() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let io = FakeIo::new(|_, _| Ok(json!({})));
    let controller = SessionController::new(id, io.clone());
    team_snapshot(&controller, id);
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Live);
    assert_eq!(state.session_mode, None);
    assert_eq!(state.pending.permissions.len(), 1);
    io.connected.store(false, Ordering::SeqCst);
    assert!(controller.respond_permission(1, true).await.is_err());
    assert!(io.requests.lock().unwrap().is_empty());
    io.connected.store(true, Ordering::SeqCst);
    controller.respond_permission(1, true).await.unwrap();
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(
        io.calls("permission.respond")[0]["decision"],
        json!({"type":"allow_once"})
    );
    assert_eq!(io.calls("permission.respond").len(), 1);
    team_snapshot(&controller, id);
    assert!(controller.state().pending.permissions.is_empty());
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(
        controller
            .respond_permission(9, true)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .send_prompt("forbidden", vec![])
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.retry_prompt("id").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.cancel_turn().await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .answer_question(1, Default::default())
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.cancel_question(1).await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.set_model("m").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller
            .set_permission_mode(PermissionModeId::AcceptEdits)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
    assert_eq!(
        controller.fetch_image("forbidden").await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    assert_eq!(io.requests.lock().unwrap().len(), 1);

    // A disconnected transport must reject even before its state callback
    // changes the last published Live snapshot.
    io.connected.store(false, Ordering::SeqCst);
    assert!(controller.respond_permission(1, true).await.is_err());
    assert_eq!(io.requests.lock().unwrap().len(), 1);
    io.connected.store(true, Ordering::SeqCst);

    // A capability cannot gain conversation authority from an invented mode.
    controller.handle_snapshot(snapshot(vec![json!({"phase":"begin", "session_id":id, "epoch":"e2", "first_message_index":0, "session_mode":"code"})]).pop().unwrap());
    assert_eq!(controller.state().status, SessionStatus::Error);
    assert_eq!(
        controller
            .respond_permission(1, true)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
}

#[tokio::test]
async fn team_approval_reserves_concurrent_responses_through_caller_cancellation() {
    struct BlockingApprovalIo {
        calls: std::sync::atomic::AtomicUsize,
        release: tokio::sync::Semaphore,
        fail_once: AtomicBool,
    }
    #[async_trait]
    impl SessionIo for BlockingApprovalIo {
        async fn request(
            &self,
            _: &str,
            _: Option<Value>,
            _: Option<Duration>,
        ) -> Result<Value, RpcError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.release.acquire().await.unwrap().forget();
            if self.fail_once.swap(false, Ordering::SeqCst) {
                Err(RpcError::code(RemoteErrorCode::Capacity))
            } else {
                Ok(json!({}))
            }
        }
        async fn notify(&self, _: &str, _: Option<Value>) {}
        fn connected(&self) -> bool {
            true
        }
    }

    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let io = Arc::new(BlockingApprovalIo {
        calls: std::sync::atomic::AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
        fail_once: AtomicBool::new(true),
    });
    let controller = SessionController::new(id, io.clone());
    team_snapshot(&controller, id);
    let first = tokio::spawn({
        let controller = controller.clone();
        async move { controller.respond_permission(1, true).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while io.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(io.calls.load(Ordering::SeqCst), 1);
    io.release.add_permits(1);
    assert_eq!(
        first.await.unwrap().unwrap_err().code,
        RemoteErrorCode::Capacity
    );
    let cancelled = tokio::spawn({
        let controller = controller.clone();
        async move { controller.respond_permission(1, true).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while io.calls.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The request has reached the transport; cancelling its caller must not
    // allow a conflicting answer while the host is still handling it.
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(io.calls.load(Ordering::SeqCst), 2);
    io.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !controller.state().pending.permissions.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    team_snapshot(&controller, id);
    assert!(controller.state().pending.permissions.is_empty());
    assert!(controller.respond_permission(1, false).await.is_err());
}

#[tokio::test]
async fn team_approval_caller_cancellation_keeps_a_sent_rpc_reserved_until_host_finishes() {
    use remote_control_client::rpc::{RequestHandler, RpcOptions, RpcPeer};

    struct RpcIo(RpcPeer);
    #[async_trait]
    impl SessionIo for RpcIo {
        async fn request(
            &self,
            method: &str,
            params: Option<Value>,
            timeout: Option<Duration>,
        ) -> Result<Value, RpcError> {
            self.0.request(method, params, timeout).await
        }
        async fn notify(&self, method: &str, params: Option<Value>) {
            let _ = self.0.notify(method, params).await;
        }
        fn connected(&self) -> bool {
            !self.0.is_closed()
        }
    }

    struct SlowHost {
        requests: Mutex<Vec<Value>>,
        received: tokio::sync::Semaphore,
        release: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl RequestHandler for SlowHost {
        async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            assert_eq!(method, "permission.respond");
            self.requests.lock().unwrap().push(params);
            self.received.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            Ok(json!({}))
        }
    }

    let (viewer_link, host_link) = support::link_pair(1024);
    let host = Arc::new(SlowHost {
        requests: Mutex::new(Vec::new()),
        received: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let _host_peer = RpcPeer::start(
        host_link,
        RpcOptions {
            requests: Some(host.clone()),
            ..Default::default()
        },
    );
    let peer = RpcPeer::start(viewer_link, RpcOptions::default());
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let controller = SessionController::new(id, Arc::new(RpcIo(peer)));
    team_snapshot(&controller, id);

    let first = tokio::spawn({
        let controller = controller.clone();
        async move { controller.respond_permission(1, true).await }
    });
    tokio::time::timeout(Duration::from_secs(2), host.received.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(host.requests.lock().unwrap().len(), 1);

    host.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !controller.state().pending.permissions.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    team_snapshot(&controller, id);
    assert!(controller.respond_permission(1, false).await.is_err());
    assert_eq!(host.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn team_approval_transport_loss_cannot_be_answered_with_the_opposite_decision() {
    for code in [
        RemoteErrorCode::Timeout,
        RemoteErrorCode::Closed,
        RemoteErrorCode::Internal,
    ] {
        let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
        let io = FakeIo::new(move |_, _| Err(RpcError::code(code)));
        let controller = SessionController::new(id, io.clone());
        team_snapshot(&controller, id);
        assert_eq!(
            controller
                .respond_permission(1, true)
                .await
                .unwrap_err()
                .code,
            code
        );
        team_snapshot(&controller, id);
        assert!(controller.state().pending.permissions.is_empty());
        assert!(controller.respond_permission(1, false).await.is_err());
        assert_eq!(io.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn team_approval_answered_elsewhere_is_consumed() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let io = FakeIo::new(|_, _| Err(RpcError::code(RemoteErrorCode::NotPending)));
    let controller = SessionController::new(id, io.clone());
    team_snapshot(&controller, id);
    controller.respond_permission(1, false).await.unwrap();
    assert!(controller.state().answered_elsewhere);
    team_snapshot(&controller, id);
    assert!(controller.state().pending.permissions.is_empty());
    assert!(controller.respond_permission(1, true).await.is_err());
    assert_eq!(io.calls("permission.respond").len(), 1);
}

#[tokio::test]
async fn team_approval_rejects_conversation_state_in_snapshot() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let controller = SessionController::new(id, FakeIo::new(attach_snapshot("e1")));
    feed(
        &controller,
        snapshot(vec![
            json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":0}),
            json!({"phase":"end", "session_id":id, "cursor":{"epoch":"e1", "seq":0},
                "pending":{}, "turn":{"running":true, "turn_id":1}}),
        ]),
    );
    assert_eq!(controller.state().status, SessionStatus::Error);
    assert!(controller.respond_permission(1, true).await.is_err());
}

#[tokio::test]
async fn team_approval_rejects_history_pagination() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let controller = SessionController::new(id, FakeIo::new(attach_snapshot("e1")));
    feed(
        &controller,
        snapshot(vec![
            json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":1}),
        ]),
    );
    assert_eq!(controller.state().status, SessionStatus::Error);
    assert!(!controller.state().has_older);
    assert!(controller.respond_permission(1, true).await.is_err());
}

#[tokio::test]
async fn team_approval_rejects_content_before_a_snapshot_can_grow() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    for page in [
        json!({"phase":"messages", "session_id":id, "messages":[{"role":"user","blocks":[{"type":"text","text":"forbidden"}]}]}),
        json!({"phase":"records", "session_id":id, "records":[{"seq":1,"record":{"type":"prompt","turn_id":1,"text":"forbidden","origin":"desktop"}}]}),
    ] {
        let controller = SessionController::new(id, FakeIo::new(attach_snapshot("e1")));
        feed(
            &controller,
            snapshot(vec![
                json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":0}),
                page,
            ]),
        );
        assert_eq!(controller.state().status, SessionStatus::Error);
        assert!(controller.state().messages.is_empty());
        assert!(controller.state().records.is_empty());
    }
}

#[tokio::test]
async fn team_approval_rejects_a_conversation_cursor() {
    let id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
    let controller = SessionController::new(id, FakeIo::new(attach_snapshot("e1")));
    feed(
        &controller,
        snapshot(vec![
            json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":0}),
            json!({"phase":"end", "session_id":id, "cursor":{"epoch":"e1", "seq":1}, "pending":{}, "turn":{"running":false}}),
        ]),
    );
    assert_eq!(controller.state().status, SessionStatus::Error);
}

#[tokio::test]
async fn a_nearby_or_malformed_team_id_remains_an_unknown_conversation() {
    for id in [
        "team-approval:short",
        "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcde!",
        "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef-",
        "TEAM-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef",
    ] {
        let io = FakeIo::new(|_, _| Ok(json!({})));
        let controller = SessionController::new(id, io.clone());
        feed(
            &controller,
            snapshot(vec![
                json!({"phase":"begin", "session_id":id, "epoch":"e1", "first_message_index":0}),
                json!({"phase":"end", "session_id":id, "cursor":{"epoch":"e1", "seq":0}, "pending":{}, "turn":{"running":false}}),
            ]),
        );
        assert_eq!(controller.state().status, SessionStatus::Loading);
        assert_eq!(
            controller
                .respond_permission(1, true)
                .await
                .unwrap_err()
                .code,
            RemoteErrorCode::Closed
        );
        assert!(io.calls("permission.respond").is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn conflicting_delta_is_rejected_before_any_held_record_is_applied() {
    let slot = Arc::new(Mutex::new(None::<SessionController>));
    let source = slot.clone();
    let io = FakeIo::new(move |method, _| {
        assert_eq!(method, "session.attach");
        let controller = source.lock().unwrap().clone().unwrap();
        controller.handle_event(prompt(3, 3));
        controller.handle_event(event(
            4,
            json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
        ));
        Ok(
            json!({ "mode": "delta", "epoch": "e1", "pending": { "permissions": [], "questions": [], "computer_access": [] } }),
        )
    });
    let controller = SessionController::new("s1", io.clone());
    *slot.lock().unwrap() = Some(controller.clone());
    feed(&controller, full_snapshot("e1", 8, 2));
    let before = controller.state();
    controller.activate();
    settle().await;
    let state = controller.state();
    assert_eq!(state.status, SessionStatus::Error);
    assert_eq!(state.records, before.records);
    assert_eq!(state.messages, before.messages);
    assert_eq!(state.first_message_index, before.first_message_index);
    assert_eq!(
        io.calls("session.attach"),
        vec![json!({"session_id":"s1", "resume":{"epoch":"e1", "after_seq":2}})]
    );
    controller.connection_ready();
    settle().await;
    assert_eq!(io.calls("session.attach").len(), 1, "conflict is sticky");
    slot.lock().unwrap().take();
}

#[tokio::test(start_paused = true)]
async fn events_between_snapshot_end_and_attach_reply_are_validated_and_flushed() {
    for conflict in [false, true] {
        let slot = Arc::new(Mutex::new(None::<SessionController>));
        let source = slot.clone();
        let io = FakeIo::new(move |method, _| {
            assert_eq!(method, "session.attach");
            let controller = source.lock().unwrap().clone().unwrap();
            let mut parts = full_snapshot("e1", 0, 2);
            if !conflict {
                if let SessionSnapshotParams::Begin { session_mode, .. } = &mut parts[0] {
                    *session_mode = None;
                }
            }
            feed(&controller, parts);
            controller.handle_event(prompt(3, 3));
            controller.handle_event(event(
                4,
                json!({
                    "type":"session_started", "session_id":"s1",
                    "mode": if conflict { "chat" } else { "code" }
                }),
            ));
            Ok(json!({ "mode": "snapshot", "epoch": "e1" }))
        });
        let controller = SessionController::new("s1", io);
        *slot.lock().unwrap() = Some(controller.clone());
        controller.activate();
        settle().await;
        let state = controller.state();
        if conflict {
            assert_eq!(state.status, SessionStatus::Error);
            assert_eq!(
                state.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
                vec![1, 2]
            );
        } else {
            assert_eq!(state.status, SessionStatus::Live);
            assert_eq!(state.session_mode, Some(SessionModeDto::Code));
            assert_eq!(
                state.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
                vec![1, 2, 3, 4]
            );
        }
        slot.lock().unwrap().take();
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

    controller.send_prompt("   ", vec![]).await.unwrap();
    assert!(
        controller.state().outgoing.is_empty(),
        "empty prompts are not sent"
    );

    controller.send_prompt("do it", vec![]).await.unwrap();
    let state = controller.state();
    assert_eq!(state.outgoing.len(), 1);
    assert_eq!(state.outgoing[0].status, OutgoingStatus::Failed);
    assert_eq!(state.outgoing[0].error.as_deref(), Some("capacity"));
    let id = state.outgoing[0].id.clone();

    controller.retry_prompt(&id).await.unwrap();
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
async fn attachments_are_rejected_while_a_turn_runs() {
    let io = FakeIo::new(|method, _| match method {
        "prompt.send" => Ok(json!({ "turn_id": 1 })),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    let image = |t: &str| ImageRefDto {
        media_type: t.into(),
        base64: "AAAA".into(),
    };
    let mut idle = full_snapshot("e1", 0, 2);
    if let SessionSnapshotParams::End { turn, .. } = &mut idle[3] {
        *turn = Default::default();
    }
    feed(&controller, idle);
    // Invalid batches are refused intact instead of silently dropping inputs.
    for images in [
        vec![image("image/png"), image("image/bmp")],
        vec![image("image/png"); 5],
    ] {
        assert_eq!(
            controller
                .send_prompt("keep draft", images)
                .await
                .unwrap_err()
                .code,
            RemoteErrorCode::InvalidParams
        );
    }
    assert_eq!(
        controller
            .send_prompt_with_files("keep files", vec![], vec!["receipt".into(); 5])
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::InvalidParams
    );
    assert!(io.calls("prompt.send").is_empty());
    assert!(controller.state().outgoing.is_empty());

    // Idle: the entire supported batch is sent.
    controller
        .send_prompt(
            "look",
            vec![
                image("image/png"),
                image("image/jpeg"),
                image("image/gif"),
                image("image/webp"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        io.calls("prompt.send")[0]["images"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // A stale idle composer must be told that attachments were not admitted.
    feed(&controller, full_snapshot("e1", 0, 2));
    assert!(controller.state().turn.running);
    assert!(controller
        .send_prompt("again", vec![image("image/png")])
        .await
        .is_err());
    assert!(controller
        .send_prompt("", vec![image("image/png")])
        .await
        .is_err());
    assert!(controller
        .send_prompt_with_files("again", vec![], vec!["receipt".into()])
        .await
        .is_err());
    assert_eq!(io.calls("prompt.send").len(), 1);
    controller.send_prompt("again", vec![]).await.unwrap();
    assert!(io.calls("prompt.send")[1].get("images").is_none());
    assert_eq!(io.calls("prompt.send").len(), 2);
}

#[tokio::test]
async fn retry_retains_failed_attachments_until_the_running_turn_finishes() {
    for with_image in [true, false] {
        let mut attempts = 0;
        let io = FakeIo::new(move |method, _| {
            if method == "prompt.send" {
                attempts += 1;
                if attempts == 1 {
                    return Err(RpcError::code(RemoteErrorCode::Capacity));
                }
                return Ok(json!({ "turn_id": 22 }));
            }
            Ok(json!({}))
        });
        let controller = SessionController::new("s1", io.clone());
        let mut idle = full_snapshot("e1", 0, 2);
        if let SessionSnapshotParams::End { turn, .. } = &mut idle[3] {
            *turn = Default::default();
        }
        feed(&controller, idle);
        let images = if with_image {
            vec![ImageRefDto {
                media_type: "image/png".into(),
                base64: "AAAA".into(),
            }]
        } else {
            vec![]
        };
        let files = if with_image {
            vec![]
        } else {
            vec!["receipt".into()]
        };
        controller
            .send_prompt_with_files("preserve", images, files)
            .await
            .unwrap();
        let failed = controller.state().outgoing[0].clone();
        assert_eq!(failed.status, OutgoingStatus::Failed);

        controller.handle_event(event(3, json!({"type":"turn_started", "turn_id":9})));
        assert_eq!(
            controller.retry_prompt(&failed.id).await.unwrap_err().code,
            RemoteErrorCode::Forbidden
        );
        assert_eq!(controller.state().outgoing[0], failed);
        assert_eq!(io.calls("prompt.send").len(), 1);

        let mut idle = full_snapshot("e1", 0, 3);
        if let SessionSnapshotParams::End { turn, .. } = &mut idle[3] {
            *turn = Default::default();
        }
        feed(&controller, idle);
        controller.retry_prompt(&failed.id).await.unwrap();
        let sent = io.calls("prompt.send");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["client_request_id"], sent[1]["client_request_id"]);
        assert_eq!(sent[0].get("images"), sent[1].get("images"));
        assert_eq!(sent[0].get("file_refs"), sent[1].get("file_refs"));
    }
}

#[tokio::test]
async fn disconnected_transport_rejects_a_stale_live_session_before_queueing() {
    let io = FakeIo::new(|_, _| Ok(json!({})));
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));
    assert_eq!(controller.state().status, SessionStatus::Live);
    io.connected.store(false, Ordering::SeqCst);
    assert!(controller.require_conversation_access().is_err());
    assert!(controller
        .send_prompt("keep the draft", vec![])
        .await
        .is_err());
    assert!(controller.cancel_turn().await.is_err());
    assert!(controller.fetch_image("s1:image").await.is_err());
    assert!(controller.state().outgoing.is_empty());
    assert!(io.requests.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn losing_an_answer_race_is_quiet() {
    let io = FakeIo::new(|method, _| match method {
        "permission.respond" => Err(RpcError::code(RemoteErrorCode::NotPending)),
        "question.answer" => Err(RpcError::code(RemoteErrorCode::Internal)),
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));
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
        ("image.get", Some("s1:m:0:0")) => {
            Ok(json!({ "media_type": "image/png", "base64": "QUJD" }))
        }
        ("image.get", Some("s1:m:0:1")) => {
            Ok(json!({ "media_type": "text/html", "base64": "QUJD" }))
        }
        ("image.get", Some("s1:m:0:2")) => {
            Ok(json!({ "media_type": "image/png", "base64": "QU<JD" }))
        }
        _ => Ok(json!({})),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 0, 2));
    assert_eq!(
        controller.fetch_image("s1:m:0:0").await.unwrap(),
        "data:image/png;base64,QUJD"
    );
    controller.fetch_image("s1:m:0:0").await.unwrap();
    assert_eq!(io.calls("image.get").len(), 1);
    assert!(controller.fetch_image("s1:m:0:1").await.is_err());
    assert!(controller.fetch_image("s1:m:0:2").await.is_err());
    assert!(controller.fetch_image("s2:m:0:0").await.is_err());
    assert_eq!(
        io.calls("image.get").len(),
        3,
        "a foreign session reference never reaches the host"
    );
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
    feed(&controller, full_snapshot("e1", 0, 2));
    for text in [
        "/model",
        "/model chosen",
        "/effort high",
        "/plan",
        "/plan open",
        "/plan share",
    ] {
        controller.send_prompt(text, vec![]).await.unwrap();
        assert!(
            controller.state().outgoing.is_empty(),
            "{text} has no prompt echo"
        );
    }
    controller
        .send_prompt("/plan implement this", vec![])
        .await
        .unwrap();
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
            true
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
    tokio::time::timeout(Duration::from_secs(5), async {
        while !controller.state().loading_older {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("history request starts");
    controller.handle_reset();
    feed(&controller, full_snapshot("new", 100, 2));
    let expected = controller.state();
    response.send(json!({ "messages": [{ "role": "user", "blocks": [] }], "first_index": 99, "has_more": true })).unwrap();
    pending.await.unwrap();
    assert_eq!(controller.state(), expected);
}

#[tokio::test]
async fn mode_conflict_cancels_history_ui_and_blocks_new_history_requests() {
    struct DelayedHistory {
        requests: std::sync::atomic::AtomicUsize,
        response: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Value>>>,
    }
    #[async_trait]
    impl SessionIo for DelayedHistory {
        async fn request(
            &self,
            method: &str,
            _: Option<Value>,
            _: Option<Duration>,
        ) -> Result<Value, RpcError> {
            assert_eq!(method, "session.history");
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.lock().await.take().unwrap().await.unwrap())
        }
        async fn notify(&self, _: &str, _: Option<Value>) {}
        fn connected(&self) -> bool {
            true
        }
    }
    let (response, receiver) = tokio::sync::oneshot::channel();
    let io = Arc::new(DelayedHistory {
        requests: std::sync::atomic::AtomicUsize::new(0),
        response: tokio::sync::Mutex::new(Some(receiver)),
    });
    let controller = SessionController::new("s1", io.clone());
    feed(&controller, full_snapshot("e1", 100, 2));
    let reader = controller.clone();
    let pending = tokio::spawn(async move { reader.load_older().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while io.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("history request starts");
    assert!(controller.state().loading_older);
    controller.handle_event(event(
        3,
        json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
    ));
    let rejected = controller.state();
    assert_eq!(rejected.status, SessionStatus::Error);
    assert!(!rejected.loading_older);
    response.send(json!({ "messages": [{ "role": "user", "blocks": [] }], "first_index": 99, "has_more": true })).unwrap();
    pending.await.unwrap();
    controller.load_older().await;
    assert_eq!(io.requests.load(Ordering::SeqCst), 1);
    assert_eq!(controller.state(), rejected);
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
async fn a_late_image_response_after_epoch_or_mode_change_is_not_returned_or_cached() {
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
            true
        }
    }
    for mode_conflict in [false, true] {
        let (response, receiver) = tokio::sync::oneshot::channel();
        let io = Arc::new(DelayedImage {
            started: tokio::sync::Notify::new(),
            response: tokio::sync::Mutex::new(Some(receiver)),
        });
        let controller = SessionController::new("s1", io.clone());
        feed(&controller, full_snapshot("old", 0, 2));
        let reader = controller.clone();
        let fetch = tokio::spawn(async move { reader.fetch_image("s1:m:0:0").await });
        tokio::time::timeout(Duration::from_secs(5), io.started.notified())
            .await
            .expect("image request starts");
        if mode_conflict {
            controller.handle_event(event(
                3,
                json!({"type":"session_started", "session_id":"s1", "mode":"chat"}),
            ));
        } else {
            feed(&controller, full_snapshot("new", 0, 2));
        }
        response
            .send(json!({ "media_type": "image/png", "base64": "AQID" }))
            .unwrap();
        assert_eq!(
            fetch.await.unwrap().unwrap_err().code,
            RemoteErrorCode::Closed
        );
        if mode_conflict {
            assert_eq!(
                controller.fetch_image("s1:m:0:0").await.unwrap_err().code,
                RemoteErrorCode::Closed
            );
        }
    }
}
