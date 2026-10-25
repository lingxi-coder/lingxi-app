//! RPC peers over an in-memory link.

mod support;

use async_trait::async_trait;
use remote_control_client::framing::encode_message;
use remote_control_client::rpc::{
    KeepaliveKind, RemoteErrorCode, RequestHandler, RpcError, RpcOptions, RpcPeer,
};
use remote_control_client::transport::{FragmentLink, Link, LinkClosed};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{advance, link_pair, link_pair_logged, settle};

struct BackpressuredSend {
    inner: Arc<dyn FragmentLink>,
    count: AtomicUsize,
    blocked: tokio::sync::Notify,
}

#[async_trait]
impl FragmentLink for BackpressuredSend {
    fn max_fragment_bytes(&self) -> usize {
        self.inner.max_fragment_bytes()
    }
    async fn send_fragment(&self, fragment: Vec<u8>) -> Result<(), LinkClosed> {
        if self.count.fetch_add(1, Ordering::SeqCst) == 1 {
            self.blocked.notify_one();
            std::future::pending::<()>().await;
        }
        self.inner.send_fragment(fragment).await
    }
    fn close(&self, reason: &str) {
        self.inner.close(reason);
    }
}

fn backpressured_pair() -> (RpcPeer, RpcPeer, Arc<BackpressuredSend>) {
    let (a, b) = link_pair(64);
    let blocked = Arc::new(BackpressuredSend {
        inner: a.link,
        count: AtomicUsize::new(0),
        blocked: tokio::sync::Notify::new(),
    });
    let a = RpcPeer::start(
        Link {
            link: blocked.clone(),
            events: a.events,
        },
        RpcOptions::default(),
    );
    let b = RpcPeer::start(
        b,
        RpcOptions {
            requests: Some(Arc::new(Echo)),
            ..Default::default()
        },
    );
    (a, b, blocked)
}

#[tokio::test(start_paused = true)]
async fn timing_out_a_partial_message_closes_the_link_before_another_message_can_interleave() {
    let (a, b, _) = backpressured_pair();
    let error = a
        .request(
            "echo",
            Some(json!("x".repeat(512))),
            Some(Duration::from_secs(1)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Timeout);
    assert_eq!(a.closed_reason().as_deref(), Some("send_incomplete"));
    settle().await;
    assert!(b.is_closed());
    assert_eq!(
        a.request("echo", Some(json!(1)), None)
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Closed
    );
}

#[tokio::test(start_paused = true)]
async fn aborting_a_partial_message_closes_the_link_but_a_queued_timeout_does_not() {
    let (a, b, blocked) = backpressured_pair();
    let sending = {
        let a = a.clone();
        tokio::spawn(async move { a.request("echo", Some(json!("x".repeat(512))), None).await })
    };
    blocked.blocked.notified().await;
    assert_eq!(
        a.request("echo", None, Some(Duration::from_secs(1)))
            .await
            .unwrap_err()
            .code,
        RemoteErrorCode::Timeout
    );
    assert!(
        !a.is_closed(),
        "nothing from the queued request reached the wire"
    );
    sending.abort();
    let _ = sending.await;
    settle().await;
    assert_eq!(a.closed_reason().as_deref(), Some("send_incomplete"));
    assert!(b.is_closed());
}

struct Echo;

#[async_trait]
impl RequestHandler for Echo {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "echo" => Ok(params),
            "fail" => Err(RpcError::new(RemoteErrorCode::InvalidParams, "nope")),
            "slow" => {
                tokio::time::sleep(Duration::from_secs(120)).await;
                Ok(json!({}))
            }
            "huge" => Ok(Value::String("x".repeat(17 * 1024 * 1024))),
            _ => Err(RpcError::code(RemoteErrorCode::Internal)),
        }
    }
}

type Notes = Arc<Mutex<Vec<(String, Value)>>>;

fn pair() -> (RpcPeer, RpcPeer, Notes, Notes) {
    let (a, b) = link_pair(64);
    let (a_notes, b_notes): (Notes, Notes) = Default::default();
    let capture = |notes: &Notes| {
        let notes = Arc::clone(notes);
        Arc::new(move |m: &str, p: Value| notes.lock().unwrap().push((m.to_owned(), p)))
    };
    let pa = RpcPeer::start(
        a,
        RpcOptions {
            timeout: None,
            requests: Some(Arc::new(Echo)),
            notifications: Some(capture(&a_notes)),
        },
    );
    let pb = RpcPeer::start(
        b,
        RpcOptions {
            timeout: None,
            requests: Some(Arc::new(Echo)),
            notifications: Some(capture(&b_notes)),
        },
    );
    (pa, pb, a_notes, b_notes)
}

#[tokio::test(start_paused = true)]
async fn requests_and_errors_cross_in_both_directions() {
    let (a, b, ..) = pair();
    let big = json!({ "text": "ü".repeat(5000), "n": [1, 2, 3] });
    assert_eq!(
        a.request("echo", Some(big.clone()), None).await.unwrap(),
        big
    );
    assert_eq!(
        b.request("echo", Some(json!(7)), None).await.unwrap(),
        json!(7)
    );
    let error = a.request("fail", None, None).await.unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (RemoteErrorCode::InvalidParams, "nope")
    );
    let error = a.request("unknown", None, None).await.unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Internal);
}

#[tokio::test(start_paused = true)]
async fn timeouts_and_late_answers() {
    let (a, ..) = pair();
    let error = a
        .request("slow", None, Some(Duration::from_secs(5)))
        .await
        .unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Timeout);
    // The late answer must not disturb anything.
    advance(Duration::from_secs(130)).await;
    assert_eq!(
        a.request("echo", Some(json!(1)), None).await.unwrap(),
        json!(1)
    );
}

#[tokio::test(start_paused = true)]
async fn the_default_timeout_is_thirty_seconds() {
    let (a, ..) = pair();
    let started = tokio::time::Instant::now();
    let error = a.request("slow", None, None).await.unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Timeout);
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn ping_is_answered_and_hidden_from_the_application() {
    let (a, _b, a_notes, b_notes) = pair();
    a.notify("ping", None).await.unwrap();
    a.notify("custom", Some(json!({ "k": 1 }))).await.unwrap();
    settle().await;
    assert_eq!(
        *b_notes.lock().unwrap(),
        vec![("custom".to_owned(), json!({ "k": 1 }))]
    );
    assert!(
        a_notes.lock().unwrap().is_empty(),
        "the pong is not an application notification"
    );
}

#[tokio::test(start_paused = true)]
async fn closing_fails_pending_requests_and_both_sides() {
    let (a, b, ..) = pair();
    let pending = {
        let a = a.clone();
        tokio::spawn(async move { a.request("slow", None, None).await })
    };
    settle().await;
    b.close("bye");
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Closed);
    settle().await;
    assert!(a.is_closed() && b.is_closed());
    assert_eq!(b.closed_reason().as_deref(), Some("bye"));
    assert_eq!(a.closed().await, "peer_closed");
    assert_eq!(
        a.request("echo", None, None).await.unwrap_err().code,
        RemoteErrorCode::Closed
    );
    // Notifications after close are dropped silently.
    a.notify("x", None).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn garbage_closes_with_protocol_error() {
    for text in [
        "not json",
        "[1]",
        "{}",
        "{\"id\":5,\"method\":\"x\"}",
        "{\"id\":\"1\"}",
    ] {
        let (a, b) = link_pair(64);
        let victim = RpcPeer::start(b, RpcOptions::default());
        for fragment in encode_message(text, 64).unwrap() {
            a.link.send_fragment(fragment).await.unwrap();
        }
        settle().await;
        assert_eq!(
            victim.closed_reason().as_deref(),
            Some("protocol_error"),
            "{text}"
        );
    }
    // A broken fragment sequence does too.
    let (a, b) = link_pair(64);
    let victim = RpcPeer::start(b, RpcOptions::default());
    a.link.send_fragment(vec![2, 1, 2, 3]).await.unwrap();
    settle().await;
    assert_eq!(victim.closed_reason().as_deref(), Some("protocol_error"));
}

#[tokio::test(start_paused = true)]
async fn oversized_messages_are_refused_without_hanging() {
    let (a, ..) = pair();
    // A request above 16 MiB is the caller's error.
    let error = a
        .request(
            "echo",
            Some(Value::String("x".repeat(17 * 1024 * 1024))),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::InvalidParams);
    // A response above 16 MiB is turned into an error answer.
    let error = a.request("huge", None, None).await.unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Internal);
    assert!(a
        .notify("big", Some(Value::String("x".repeat(17 * 1024 * 1024))))
        .await
        .is_err());
    assert!(!a.is_closed());
}

#[tokio::test(start_paused = true)]
async fn keepalive_on_a_data_channel_pings_every_15s_and_gives_up_at_45s() {
    let (a, b, sent_by_a, _) = link_pair_logged(64);
    let peer = RpcPeer::start(a, RpcOptions::default());
    drop(b); // nobody answers
    peer.start_keepalive(KeepaliveKind::DataChannel);
    advance(Duration::from_secs(14)).await;
    assert_eq!(count_pings(&sent_by_a), 0);
    advance(Duration::from_secs(2)).await;
    assert_eq!(count_pings(&sent_by_a), 1);
    advance(Duration::from_secs(15)).await;
    assert_eq!(count_pings(&sent_by_a), 2);
    assert!(!peer.is_closed());
    advance(Duration::from_secs(14)).await; // t = 45
    assert_eq!(peer.closed_reason().as_deref(), Some("keepalive_timeout"));
}

#[tokio::test(start_paused = true)]
async fn keepalive_on_a_tunnel_uses_25_and_75_seconds() {
    let (a, b, sent_by_a, _) = link_pair_logged(64);
    let peer = RpcPeer::start(a, RpcOptions::default());
    drop(b);
    peer.start_keepalive(KeepaliveKind::Tunnel);
    advance(Duration::from_secs(24)).await;
    assert_eq!(count_pings(&sent_by_a), 0);
    advance(Duration::from_secs(2)).await;
    assert_eq!(count_pings(&sent_by_a), 1);
    advance(Duration::from_secs(48)).await; // t = 74
    assert!(!peer.is_closed());
    advance(Duration::from_secs(2)).await;
    assert_eq!(peer.closed_reason().as_deref(), Some("keepalive_timeout"));
}

#[tokio::test(start_paused = true)]
async fn any_traffic_counts_as_life() {
    let (a, b) = pair_pair();
    a.start_keepalive(KeepaliveKind::DataChannel);
    b.start_keepalive(KeepaliveKind::DataChannel);
    advance(Duration::from_secs(200)).await;
    assert!(
        !a.is_closed() && !b.is_closed(),
        "both answer each other's pings"
    );
}

fn pair_pair() -> (RpcPeer, RpcPeer) {
    let (a, b, ..) = pair();
    (a, b)
}

fn count_pings(sent: &Arc<Mutex<Vec<Vec<u8>>>>) -> usize {
    sent.lock()
        .unwrap()
        .iter()
        .filter(|f| String::from_utf8_lossy(f).contains("\"ping\""))
        .count()
}
