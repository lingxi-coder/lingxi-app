//! The RTDB tunnel against the in-memory database: round trips, ordering, the gap
//! timeout, the close notice and cleanup.

mod support;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use remote_control_client::rpc::{RemoteErrorCode, RequestHandler, RpcError, RpcOptions, RpcPeer};
use remote_control_client::rtdb::{MemoryRtdb, RtdbError};
use remote_control_client::signaling::{down_path, frame_key};
use remote_control_client::transport::{FragmentLink, LinkEvent};
use remote_control_client::tunnel::{TunnelLink, TunnelLinkOptions};
use remote_control_client::tunnel_crypto::{
    derive_tunnel_keys, pack_fragments, seal_frame, TunnelDir,
};
use serde_json::{json, Value};
use support::{advance, drain, settle, tunnel_pair, SlowRtdb, HOST, UID};

const CONN: &str = "Ck9XSg5Dub46zKMNo6KiaQ";

fn fragments(events: &[LinkEvent]) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|e| match e {
            LinkEvent::Fragment(f) => Some(f.clone()),
            LinkEvent::Closed(_) => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn fragments_flow_both_ways_and_batch_into_one_frame() {
    let db = Arc::new(MemoryRtdb::new());
    let mut pair = tunnel_pair(db.clone(), CONN);
    for text in ["a", "bb", "ccc"] {
        pair.viewer
            .send_fragment(text.as_bytes().to_vec())
            .await
            .unwrap();
    }
    pair.host
        .send_fragment(b"from host".to_vec())
        .await
        .unwrap();
    settle().await;
    // Nothing is written before the 25 ms flush timer fires.
    assert!(db.writes().is_empty());
    advance(Duration::from_millis(25)).await;
    let puts: Vec<_> = db
        .writes()
        .into_iter()
        .filter(|w| w.method == "PUT")
        .collect();
    assert_eq!(puts.len(), 2, "one frame per direction: {puts:?}");
    assert_eq!(
        fragments(&drain(&mut pair.host_events)),
        vec![b"a".to_vec(), b"bb".to_vec(), b"ccc".to_vec()]
    );
    assert_eq!(
        fragments(&drain(&mut pair.viewer_events)),
        vec![b"from host".to_vec()]
    );
}

#[tokio::test(start_paused = true)]
async fn a_full_frame_is_written_without_waiting() {
    let db = Arc::new(MemoryRtdb::new());
    let pair = tunnel_pair(db.clone(), CONN);
    // Three 16374-byte fragments fill a frame (49134 of 49136 bytes).
    for _ in 0..3 {
        pair.viewer.send_fragment(vec![9u8; 16374]).await.unwrap();
    }
    settle().await;
    assert_eq!(db.writes().iter().filter(|w| w.method == "PUT").count(), 1);
    // A fourth goes into the next frame, written after the timer.
    pair.viewer.send_fragment(vec![1u8; 10]).await.unwrap();
    settle().await;
    assert_eq!(db.writes().iter().filter(|w| w.method == "PUT").count(), 1);
    advance(Duration::from_millis(25)).await;
    assert_eq!(db.writes().iter().filter(|w| w.method == "PUT").count(), 2);
    assert!(db.read(&format!("{}/f0", pair.up)).is_string());
    assert!(db.read(&format!("{}/f1", pair.up)).is_string());
}

#[tokio::test(start_paused = true)]
async fn at_most_four_writes_are_in_flight() {
    let db = Arc::new(MemoryRtdb::new());
    let slow = SlowRtdb::new(db.clone(), Duration::from_millis(100));
    let mut pair = tunnel_pair(slow.clone(), CONN);
    // Ten full frames.
    for _ in 0..30 {
        pair.viewer.send_fragment(vec![5u8; 16374]).await.unwrap();
    }
    settle().await;
    advance(Duration::from_secs(2)).await;
    assert_eq!(slow.max_active.load(Ordering::SeqCst), 4);
    assert_eq!(fragments(&drain(&mut pair.host_events)).len(), 30);
}

#[tokio::test(start_paused = true)]
async fn out_of_order_frames_are_delivered_in_order_and_old_ones_ignored() {
    let db = Arc::new(MemoryRtdb::new());
    let keys = derive_tunnel_keys(&[7u8; 32], CONN).unwrap();
    let (link, mut events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: db.clone(),
        conn_id: CONN.into(),
        send_path: down_path(UID, HOST, CONN).unwrap(),
        send_dir: TunnelDir::Down,
        send_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().down,
        recv_path: "/unused".into(),
        recv_dir: TunnelDir::Up,
        recv_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().up,
        subscribe: false,
    });
    let frame = |seq: u64, text: &str| {
        let plaintext = pack_fragments(&[text.as_bytes().to_vec()]).unwrap();
        Value::String(seal_frame(&keys.up, CONN, TunnelDir::Up, seq, &plaintext).unwrap())
    };
    link.deliver_frame(2, &frame(2, "two"));
    link.deliver_frame(1, &frame(1, "one"));
    settle().await;
    assert!(drain(&mut events).is_empty(), "frame 0 is still missing");
    link.deliver_frame(0, &frame(0, "zero"));
    settle().await;
    assert_eq!(
        fragments(&drain(&mut events)),
        vec![b"zero".to_vec(), b"one".to_vec(), b"two".to_vec()]
    );
    // A repeat after an SSE reconnect is ignored, not delivered twice.
    link.deliver_frame(1, &frame(1, "one"));
    link.deliver_frame(3, &frame(3, "three"));
    link.deliver_frame(3, &frame(3, "three"));
    settle().await;
    assert_eq!(fragments(&drain(&mut events)), vec![b"three".to_vec()]);
    // Non-strings (a deletion) are ignored.
    link.deliver_frame(4, &Value::Null);
    settle().await;
    assert!(drain(&mut events).is_empty());
    assert!(link.close_reason().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_missing_frame_ends_the_link_after_thirty_seconds() {
    let db = Arc::new(MemoryRtdb::new());
    let keys = derive_tunnel_keys(&[7u8; 32], CONN).unwrap();
    let (link, mut events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: db.clone(),
        conn_id: CONN.into(),
        send_path: down_path(UID, HOST, CONN).unwrap(),
        send_dir: TunnelDir::Down,
        send_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().down,
        recv_path: "/unused".into(),
        recv_dir: TunnelDir::Up,
        recv_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().up,
        subscribe: false,
    });
    let plaintext = pack_fragments(&[b"late".to_vec()]).unwrap();
    let stored =
        |seq| Value::String(seal_frame(&keys.up, CONN, TunnelDir::Up, seq, &plaintext).unwrap());
    link.deliver_frame(1, &stored(1));
    settle().await;
    advance(Duration::from_secs(29)).await;
    assert!(link.close_reason().is_none());
    // The frame arrives in time: the clock stops.
    link.deliver_frame(0, &stored(0));
    settle().await;
    assert_eq!(fragments(&drain(&mut events)).len(), 2);
    advance(Duration::from_secs(120)).await;
    assert!(link.close_reason().is_none());

    // A new hole gets a fresh 30 s.
    link.deliver_frame(3, &stored(3));
    settle().await;
    advance(Duration::from_secs(29)).await;
    assert!(link.close_reason().is_none());
    advance(Duration::from_secs(2)).await;
    assert_eq!(link.close_reason().as_deref(), Some("tunnel_gap"));
    assert_eq!(
        drain(&mut events),
        vec![LinkEvent::Closed("tunnel_gap".into())]
    );
    link.when_closed().await;
}

#[tokio::test(start_paused = true)]
async fn a_tampered_or_misrouted_frame_ends_the_link() {
    for (name, seq, value, reason) in [
        (
            "garbage",
            0u64,
            Value::String("AAAA".into()),
            "tunnel_crypto",
        ),
        ("wrong seq", 1u64, Value::Null, "tunnel_crypto"),
    ] {
        let db = Arc::new(MemoryRtdb::new());
        let keys = derive_tunnel_keys(&[7u8; 32], CONN).unwrap();
        let (link, mut events) = TunnelLink::new(TunnelLinkOptions {
            rtdb: db,
            conn_id: CONN.into(),
            send_path: "/w".into(),
            send_dir: TunnelDir::Down,
            send_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().down,
            recv_path: "/unused".into(),
            recv_dir: TunnelDir::Up,
            recv_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().up,
            subscribe: false,
        });
        let value = if value.is_null() {
            let plaintext = pack_fragments(&[b"x".to_vec()]).unwrap();
            // sealed for seq 0 but presented as seq 1
            Value::String(seal_frame(&keys.up, CONN, TunnelDir::Up, 0, &plaintext).unwrap())
        } else {
            value
        };
        link.deliver_frame(seq, &value);
        settle().await;
        assert_eq!(link.close_reason().as_deref(), Some(reason), "{name}");
        assert!(drain(&mut events).contains(&LinkEvent::Closed(reason.into())));
    }
    // A frame that authenticates but holds a malformed plaintext is a protocol error.
    let keys = derive_tunnel_keys(&[7u8; 32], CONN).unwrap();
    let (link, _events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: Arc::new(MemoryRtdb::new()),
        conn_id: CONN.into(),
        send_path: "/w".into(),
        send_dir: TunnelDir::Down,
        send_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().down,
        recv_path: "/unused".into(),
        recv_dir: TunnelDir::Up,
        recv_key: derive_tunnel_keys(&[7u8; 32], CONN).unwrap().up,
        subscribe: false,
    });
    let stored = seal_frame(&keys.up, CONN, TunnelDir::Up, 0, &[0, 0, 0]).unwrap();
    link.deliver_frame(0, &Value::String(stored));
    settle().await;
    assert_eq!(link.close_reason().as_deref(), Some("protocol_error"));
}

#[tokio::test(start_paused = true)]
async fn the_close_notice_follows_the_data_and_cleans_up() {
    let db = Arc::new(MemoryRtdb::new());
    let mut pair = tunnel_pair(db.clone(), CONN);
    pair.viewer
        .send_fragment(b"last words".to_vec())
        .await
        .unwrap();
    pair.viewer.close("bye");
    assert_eq!(pair.viewer.close_reason().as_deref(), Some("bye"));
    assert!(pair.viewer.send_fragment(b"x".to_vec()).await.is_err());
    settle().await;
    pair.viewer.when_closed().await;
    let events = drain(&mut pair.host_events);
    assert_eq!(
        events,
        vec![
            LinkEvent::Fragment(b"last words".to_vec()),
            LinkEvent::Closed("peer_closed".into())
        ]
    );
    assert_eq!(
        drain(&mut pair.viewer_events),
        vec![LinkEvent::Closed("bye".into())]
    );
    // The viewer removed its direction; the host (closed by the notice, which it
    // answers by not sending its own) removes what it consumed in batches.
    pair.host.when_closed().await;
    assert_eq!(db.read(&pair.up), Value::Null);
    // Cleanup is idempotent.
    pair.viewer.close("again");
    assert_eq!(pair.viewer.close_reason().as_deref(), Some("bye"));
}

#[tokio::test(start_paused = true)]
async fn processed_frames_are_deleted_in_batches() {
    let db = Arc::new(MemoryRtdb::new());
    let pair = tunnel_pair(db.clone(), CONN);
    // 31 frames: the 500 ms timer does the deleting.
    for _ in 0..31 {
        for _ in 0..3 {
            pair.viewer.send_fragment(vec![1u8; 16374]).await.unwrap();
        }
    }
    settle().await;
    assert!(
        db.writes().iter().all(|w| w.method != "PATCH"),
        "no batch yet"
    );
    advance(Duration::from_millis(500)).await;
    assert_eq!(
        db.writes().iter().filter(|w| w.method == "PATCH").count(),
        1
    );
    assert_eq!(
        db.read(&pair.up),
        Value::Null,
        "the host deleted every frame it read"
    );
    // 32 frames: deleted as soon as the batch fills, without the timer.
    for _ in 0..32 {
        for _ in 0..3 {
            pair.viewer.send_fragment(vec![1u8; 16374]).await.unwrap();
        }
    }
    settle().await;
    assert_eq!(
        db.writes().iter().filter(|w| w.method == "PATCH").count(),
        2
    );
    assert_eq!(db.read(&pair.up), Value::Null);
}

#[tokio::test(start_paused = true)]
async fn writes_are_retried_then_the_link_gives_up() {
    // A transient failure is retried.
    let db = Arc::new(MemoryRtdb::new());
    let mut pair = tunnel_pair(db.clone(), CONN);
    db.fail_next_write(RtdbError::new(503, "unavailable"));
    db.fail_next_write(RtdbError::new(0, "network error"));
    pair.viewer
        .send_fragment(b"persistent".to_vec())
        .await
        .unwrap();
    advance(Duration::from_secs(5)).await;
    assert_eq!(
        fragments(&drain(&mut pair.host_events)),
        vec![b"persistent".to_vec()]
    );
    assert!(pair.viewer.close_reason().is_none());

    // A permanent refusal ends the link at once.
    db.fail_next_write(RtdbError::new(401, "Permission denied"));
    pair.viewer
        .send_fragment(b"refused".to_vec())
        .await
        .unwrap();
    advance(Duration::from_secs(1)).await;
    assert_eq!(pair.viewer.close_reason().as_deref(), Some("tunnel_write"));
    pair.viewer.when_closed().await;

    // Five transient failures in a row exhaust the retries.
    let db = Arc::new(MemoryRtdb::new());
    let pair = tunnel_pair(db.clone(), "AAAAAAAAAAAAAAAAAAAAAA");
    for _ in 0..5 {
        db.fail_next_write(RtdbError::new(500, "boom"));
    }
    pair.viewer.send_fragment(b"x".to_vec()).await.unwrap();
    advance(Duration::from_secs(10)).await;
    assert_eq!(pair.viewer.close_reason().as_deref(), Some("tunnel_write"));
}

struct Echo;

#[async_trait::async_trait]
impl RequestHandler for Echo {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "echo" => Ok(json!({ "echoed": params })),
            "fail" => Err(RpcError::new(RemoteErrorCode::Forbidden, "no")),
            _ => Err(RpcError::code(RemoteErrorCode::Internal)),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn rpc_over_the_tunnel_carries_large_messages_and_cleans_up() {
    let db = Arc::new(MemoryRtdb::new());
    let pair = tunnel_pair(db.clone(), CONN);
    let viewer = RpcPeer::start(
        pair.viewer.clone().into_link(pair.viewer_events),
        RpcOptions::default(),
    );
    let host = RpcPeer::start(
        pair.host.clone().into_link(pair.host_events),
        RpcOptions {
            requests: Some(Arc::new(Echo)),
            ..RpcOptions::default()
        },
    );
    let payload = "tunnel ".repeat(45_000); // about 300 KB, many frames
    let reply = viewer.request("echo", Some(json!({ "payload": payload })), None);
    tokio::pin!(reply);
    let result = loop {
        tokio::select! {
            r = &mut reply => break r,
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    .unwrap();
    assert_eq!(
        result["echoed"]["payload"].as_str().unwrap().len(),
        payload.len()
    );
    let error = viewer.request("fail", None, None).await.unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Forbidden);

    viewer.close("done");
    settle().await;
    advance(Duration::from_secs(2)).await;
    pair.viewer.when_closed().await;
    pair.host.when_closed().await;
    assert!(host.is_closed());
    assert_eq!(host.closed_reason().as_deref(), Some("peer_closed"));
    assert_eq!(
        db.read(&format!("/remote-control/{UID}")),
        Value::Null,
        "nothing left behind"
    );
    let _ = frame_key(0);
}
