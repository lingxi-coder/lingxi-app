//! RemoteClient over a real ViewerConnection and the fake desktop (tunnel transport).

mod support;

use remote_control_client::clock::ManualClock;
use remote_control_client::connection::{ConnectionPhase, ViewerConnection, ViewerConnectionDeps};
use remote_control_client::device::{MemoryViewerKeys, PinnedHost, ViewerKeys};
use remote_control_client::keys::IdentityKey;
use remote_control_client::rtdb::MemoryRtdb;
use remote_control_client::session::{RemoteClient, SessionStatus};
use remote_control_client::signaling::DevicePlatform;
use remote_control_client::testing::FakePeerNetwork;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::host::FakeHost;
use support::{settle, UID};

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for i in 0..2000 {
        settle().await;
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(if i < 400 { 5 } else { 250 })).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(start_paused = true)]
async fn sessions_over_a_live_connection() {
    let clock = ManualClock::new(1_800_000_000_000);
    let rtdb = Arc::new(MemoryRtdb::with_clock(Arc::clone(&clock) as _));
    let host = FakeHost::start(
        Arc::clone(&rtdb) as _,
        Arc::clone(&clock) as _,
        FakePeerNetwork::new(),
        UID,
        IdentityKey::generate(),
    );
    host.configure(|c| {
        c.respond = Some(Arc::new(|method: &str, _params: &Value| match method {
            "sessions.watch" => Some(Ok(json!({ "sessions": [
                { "session_id": "s1", "title": "Fix bug", "project_label": "app", "status": "running", "viewers": 0 }
            ] }))),
            "session.attach" => Some(Ok(json!({ "mode": "snapshot", "epoch": "e1" }))),
            _ => None,
        }));
    });
    let keys = MemoryViewerKeys::new();
    let identity = keys.identity().await.unwrap();
    host.know_device(&identity.id);
    keys.pin(PinnedHost {
        host_id: host.host_id.clone(),
        host_key: host.host_key_b64u(),
        name: "Mac".into(),
        paired_at: 1,
    })
    .await;
    let mut deps = ViewerConnectionDeps::new(
        UID,
        host.host_id.clone(),
        rtdb.clone() as _,
        keys,
        DevicePlatform::Android,
    );
    deps.clock = clock as _;
    deps.random = Arc::new(|| 0.0);
    let conn = ViewerConnection::new(deps);
    let client = RemoteClient::for_connection(&conn);
    let controller = client.session("s1");
    controller.activate(); // not connected yet: nothing is sent
    conn.start();

    until("the session list", || client.sessions().is_some()).await;
    assert_eq!(client.sessions().unwrap()[0].title, "Fix bug");
    until("the attach", || {
        host.requests().iter().any(|r| r.1 == "session.attach")
    })
    .await;
    assert_eq!(controller.state().status, SessionStatus::Loading);

    // The desktop streams the snapshot, then live records.
    let conn_id = host.conns()[0].conn_id.clone();
    for part in [
        json!({ "phase": "begin", "session_id": "s1", "epoch": "e1", "first_message_index": 0 }),
        json!({ "phase": "messages", "session_id": "s1", "messages": [{ "role": "user", "blocks": [] }] }),
        json!({ "phase": "records", "session_id": "s1", "records": [] }),
        json!({ "phase": "end", "session_id": "s1", "cursor": { "epoch": "e1", "seq": 5 },
               "pending": {}, "turn": { "running": false } }),
    ] {
        host.notify(&conn_id, "session.snapshot", part).await;
    }
    host.notify(&conn_id, "session.event", json!({ "session_id": "s1", "seq": 6,
        "record": { "type": "prompt", "turn_id": 1, "text": "hi", "images": [], "origin": "desktop" } })).await;
    until("the session to be live with the record", || {
        let s = controller.state();
        s.status == SessionStatus::Live && s.records.len() == 1
    })
    .await;

    // The link drops; the session goes offline, and resumes from its cursor afterwards.
    host.drop_connection(&conn_id, "restart");
    until("offline", || {
        controller.state().status == SessionStatus::Offline
    })
    .await;
    until("ready again", || {
        conn.state().phase == ConnectionPhase::Ready
    })
    .await;
    until("a resuming attach", || {
        host.requests()
            .iter()
            .filter(|r| r.1 == "session.attach")
            .any(|r| r.2["resume"] == json!({ "epoch": "e1", "after_seq": 6 }))
    })
    .await;
    conn.stop();
    client.dispose();
}
