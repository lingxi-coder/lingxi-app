//! The in-memory database and mirror, and the REST/SSE client against a local server.

mod support;

use async_trait::async_trait;
use remote_control_client::rtdb::{
    ChangeKind, ListenItem, MemoryRtdb, Rtdb, RtdbClient, RtdbClientOptions, RtdbError, RtdbEvent,
    RtdbEventKind, RtdbListenError, RtdbMirror, StaticToken, TokenError, TokenProvider,
};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use support::http::{Resp, Server};
use support::settle;

fn event(kind: RtdbEventKind, path: &str, data: Value) -> RtdbEvent {
    RtdbEvent {
        kind,
        path: path.into(),
        data,
    }
}

// ---------------------------------------------------------------------------
// Mirror
// ---------------------------------------------------------------------------

#[test]
fn mirror_put_patch_delete_and_depths() {
    let mut mirror = RtdbMirror::new();
    let changes = mirror.apply(&event(
        RtdbEventKind::Put,
        "/",
        json!({ "c1": { "offer": { "a": 1 }, "up": { "f0": "x" } } }),
    ));
    let at1 = changes.at(1);
    assert_eq!(at1.len(), 1);
    assert_eq!(
        (at1[0].path.clone(), at1[0].kind),
        (vec!["c1".to_owned()], ChangeKind::Added)
    );
    assert_eq!(changes.at(3).len(), 2, "offer/a and up/f0");
    assert_eq!(changes.at(2).len(), 2);

    // A patch replaces the listed children, multi-segment keys included, and keeps the rest.
    let changes = mirror.apply(&event(
        RtdbEventKind::Patch,
        "/c1/up",
        json!({ "f1": "y", "f0": null }),
    ));
    let frames = changes.at(3);
    assert_eq!(frames.len(), 2);
    assert!(frames
        .iter()
        .any(|c| c.path[2] == "f1" && c.kind == ChangeKind::Added));
    assert!(frames
        .iter()
        .any(|c| c.path[2] == "f0" && c.kind == ChangeKind::Removed));
    assert_eq!(mirror.get("/c1/offer"), json!({ "a": 1 }));

    // Deleting a parent reports the removed children at depth.
    let changes = mirror.apply(&event(RtdbEventKind::Put, "/c1", Value::Null));
    assert_eq!(changes.at(2).len(), 2);
    assert!(changes.at(2).iter().all(|c| c.kind == ChangeKind::Removed));
    assert_eq!(mirror.get("/"), Value::Null);

    // The same value again changes nothing.
    mirror.apply(&event(RtdbEventKind::Put, "/k", json!(1)));
    assert!(mirror
        .apply(&event(RtdbEventKind::Put, "/k", json!(1)))
        .is_empty());
}

#[test]
fn mirror_normalizes_arrays_and_prunes_empty_containers() {
    let mut mirror = RtdbMirror::new();
    mirror.apply(&event(
        RtdbEventKind::Put,
        "/",
        json!({ "q": [null, "a", null, "b"] }),
    ));
    assert_eq!(mirror.get("/q"), json!({ "1": "a", "3": "b" }));
    mirror.apply(&event(RtdbEventKind::Put, "/q/1", Value::Null));
    mirror.apply(&event(RtdbEventKind::Put, "/q/3", Value::Null));
    assert_eq!(mirror.get("/"), Value::Null, "empty parents disappear");
}

// ---------------------------------------------------------------------------
// MemoryRtdb
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_rtdb_behaves_like_rtdb() {
    let db = MemoryRtdb::new();
    assert_eq!(db.get("/a/b").await.unwrap(), Value::Null);
    db.put("/a/b", &json!({ "x": 1, "y": null })).await.unwrap();
    assert_eq!(
        db.get("/a").await.unwrap(),
        json!({ "b": { "x": 1 } }),
        "nulls are not stored"
    );
    let mut patch = Map::new();
    patch.insert("b/z".into(), json!(2));
    patch.insert("b/x".into(), Value::Null);
    db.patch("/a", &patch).await.unwrap();
    assert_eq!(db.get("/a/b").await.unwrap(), json!({ "z": 2 }));
    db.remove("/a").await.unwrap();
    assert_eq!(db.read("/"), Value::Null);

    // Server timestamps resolve to the database clock.
    db.put("/t", &remote_control_client::signaling::server_timestamp())
        .await
        .unwrap();
    assert!(db.get("/t").await.unwrap().as_u64().unwrap() > 1_700_000_000_000);
}

#[tokio::test]
async fn memory_rtdb_listeners_get_an_initial_put_then_changes() {
    let db = MemoryRtdb::new();
    db.put("/w/x", &json!(1)).await.unwrap();
    let mut listener = db.listen("/w");
    let ListenItem::Event(first) = listener.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (first.kind, first.path.as_str(), first.data.clone()),
        (RtdbEventKind::Put, "/", json!({ "x": 1 }))
    );
    db.put("/w/y", &json!(2)).await.unwrap();
    let ListenItem::Event(second) = listener.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (second.path.as_str(), second.data.clone()),
        ("/y", json!(2))
    );
    db.put("/other", &json!(3)).await.unwrap();
    db.remove("/w/x").await.unwrap();
    let ListenItem::Event(third) = listener.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!((third.path.as_str(), third.data), ("/x", Value::Null));
    assert_eq!(db.listener_count(), 1);
    drop(listener);
    assert_eq!(db.listener_count(), 0);
}

#[tokio::test]
async fn memory_rtdb_can_cancel_listeners_and_fail_writes() {
    let db = MemoryRtdb::new();
    let mut listener = db.listen("/w");
    let _ = listener.recv().await;
    db.cancel_listeners();
    assert_eq!(
        listener.recv().await,
        Some(ListenItem::Error(RtdbListenError::Cancel))
    );
    db.fail_next_write(RtdbError::new(503, "later"));
    assert_eq!(db.put("/a", &json!(1)).await.unwrap_err().status, 503);
    db.put("/a", &json!(1)).await.unwrap();
    assert_eq!(db.writes().len(), 1, "the failed write is not logged");
}

#[tokio::test]
async fn memory_rtdb_can_hand_out_arrays_like_the_real_one() {
    let db = MemoryRtdb::new().coerce_arrays(true);
    db.put("/a", &json!({ "0": "x", "1": "y" })).await.unwrap();
    assert_eq!(db.get("/a").await.unwrap(), json!(["x", "y"]));
    db.put("/s", &json!({ "f5": "x" })).await.unwrap();
    assert_eq!(db.get("/s").await.unwrap(), json!({ "f5": "x" }));
}

// ---------------------------------------------------------------------------
// REST
// ---------------------------------------------------------------------------

struct Tokens {
    forced: AtomicU32,
    calls: AtomicU32,
}

#[async_trait]
impl TokenProvider for Tokens {
    async fn id_token(&self, force_refresh: bool) -> Result<String, TokenError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if force_refresh {
            self.forced.fetch_add(1, Ordering::SeqCst);
            Ok("fresh-token".into())
        } else {
            Ok("old-token".into())
        }
    }
}

fn client(server: &Server, tokens: Arc<dyn TokenProvider>) -> RtdbClient {
    let mut options = RtdbClientOptions::new(server.url.clone(), tokens);
    options.namespace = Some("ns-1".into());
    options.random = Some(Arc::new(|| 0.0));
    RtdbClient::new(options)
}

#[tokio::test]
async fn rest_verbs_urls_and_bodies() {
    let server = Server::start(|req, _| match req.method.as_str() {
        "GET" => Resp::Body(200, "{\"a\":1}".into()),
        _ => Resp::Body(200, "null".into()),
    })
    .await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    assert_eq!(
        db.get("/remote-control/u/hosts").await.unwrap(),
        json!({ "a": 1 })
    );
    db.put("/x/y", &json!({ "k": ".sv" })).await.unwrap();
    let mut patch = Map::new();
    patch.insert("f1".into(), json!("v"));
    patch.insert("f0".into(), Value::Null);
    db.patch("/x/up", &patch).await.unwrap();
    db.remove("/x").await.unwrap();
    db.remove("/").await.unwrap();

    let seen = server.requests.lock().unwrap().clone();
    let summary: Vec<(String, String)> = seen
        .iter()
        .map(|r| (r.method.clone(), r.path().to_owned()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("GET".to_owned(), "/remote-control/u/hosts.json".to_owned()),
            ("PUT".to_owned(), "/x/y.json".to_owned()),
            ("PATCH".to_owned(), "/x/up.json".to_owned()),
            ("DELETE".to_owned(), "/x.json".to_owned()),
            ("DELETE".to_owned(), "/.json".to_owned()),
        ]
    );
    assert!(seen[0].query().contains("auth=tok") && seen[0].query().contains("ns=ns-1"));
    assert!(!seen[0].query().contains("print"), "reads are not silent");
    for write in &seen[1..] {
        assert!(write.query().contains("print=silent"), "{}", write.query());
    }
    assert_eq!(
        serde_json::from_str::<Value>(&seen[1].body).unwrap(),
        json!({ "k": ".sv" })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&seen[2].body).unwrap(),
        json!({ "f1": "v", "f0": null })
    );
}

#[tokio::test]
async fn a_401_is_retried_once_with_a_fresh_token() {
    let server = Server::start(|req, _| {
        if req.query().contains("auth=fresh-token") {
            Resp::Body(200, "{\"ok\":true}".into())
        } else {
            Resp::Body(401, "{\"error\":\"Auth token is expired\"}".into())
        }
    })
    .await;
    let tokens = Arc::new(Tokens {
        forced: AtomicU32::new(0),
        calls: AtomicU32::new(0),
    });
    let db = client(&server, tokens.clone());
    assert_eq!(db.get("/a").await.unwrap(), json!({ "ok": true }));
    assert_eq!(tokens.forced.load(Ordering::SeqCst), 1);
    assert_eq!(server.count(), 2);

    // Still refused with a fresh token: give up, do not loop.
    let server =
        Server::start(|_, _| Resp::Body(401, "{\"error\":\"Permission denied\"}".into())).await;
    let tokens = Arc::new(Tokens {
        forced: AtomicU32::new(0),
        calls: AtomicU32::new(0),
    });
    let db = client(&server, tokens.clone());
    let error = db.put("/a", &json!(1)).await.unwrap_err();
    assert_eq!(
        (error.status, error.message.as_str()),
        (401, "Permission denied")
    );
    assert_eq!(server.count(), 2);

    // Other statuses are not retried.
    let server = Server::start(|_, _| Resp::Body(403, "{\"error\":\"no\"}".into())).await;
    let db = client(&server, Arc::new(StaticToken("t".into())));
    assert_eq!(db.get("/a").await.unwrap_err().status, 403);
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn tokens_never_appear_in_errors() {
    let server =
        Server::start(|_, _| Resp::Body(500, "failed at /a.json?auth=SECRET-TOKEN&x=1".into()))
            .await;
    let db = client(&server, Arc::new(StaticToken("SECRET-TOKEN".into())));
    let error = db.get("/a").await.unwrap_err();
    assert_eq!(error.status, 500);
    assert!(!error.message.contains("SECRET-TOKEN"), "{}", error.message);
    // Nothing listening: a network error with status 0 and no token either.
    let db = RtdbClient::new(RtdbClientOptions::new(
        "http://127.0.0.1:1",
        Arc::new(StaticToken("SECRET-TOKEN".into())),
    ));
    let error = db.get("/a").await.unwrap_err();
    assert_eq!(error.status, 0);
    assert!(!error.message.contains("SECRET-TOKEN"), "{}", error.message);
}

// ---------------------------------------------------------------------------
// SSE
// ---------------------------------------------------------------------------

fn sse(kind: &str, path: &str, data: Value) -> String {
    format!(
        "event: {kind}\ndata: {}\n\n",
        json!({ "path": path, "data": data })
    )
}

async fn next(listener: &mut remote_control_client::rtdb::Listener) -> ListenItem {
    tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .expect("an item in time")
        .expect("the stream is open")
}

#[tokio::test]
async fn sse_events_reach_a_mirror_and_the_stream_reconnects() {
    let server = Server::start(|req, index| {
        assert_eq!(req.header("accept"), Some("text/event-stream"));
        assert!(!req.query().contains("print"));
        if index == 0 {
            Resp::Sse(
                vec![
                    sse("put", "/", Value::Null),
                    "event: keep-alive\ndata: null\n\n".into(),
                    sse("patch", "/c1", json!({ "offer": { "n": 1 } })),
                ],
                false,
            )
        } else {
            Resp::Sse(vec![sse("put", "/", json!({ "c2": { "offer": 2 } }))], true)
        }
    })
    .await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    let mut listener = db.listen("/remote-control/u/to_host/h");
    let mut mirror = RtdbMirror::new();
    for expected in [Value::Null, json!({ "c1": { "offer": { "n": 1 } } })] {
        let ListenItem::Event(e) = next(&mut listener).await else {
            panic!()
        };
        mirror.apply(&e);
        assert_eq!(mirror.get("/"), expected);
    }
    // The stream ended: after the backoff (0.5 s without jitter) it is reopened and the
    // fresh initial put replaces the state.
    let ListenItem::Event(e) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(e.kind, RtdbEventKind::Put);
    mirror.apply(&e);
    assert_eq!(mirror.get("/"), json!({ "c2": { "offer": 2 } }));
    assert_eq!(server.count(), 2);
    assert_eq!(
        server.requests.lock().unwrap()[0].path(),
        "/remote-control/u/to_host/h.json"
    );
}

#[tokio::test]
async fn sse_cancel_is_final() {
    let server = Server::start(|_, _| {
        Resp::Sse(
            vec!["event: cancel\ndata: \"permission_denied\"\n\n".into()],
            true,
        )
    })
    .await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    let mut listener = db.listen("/a");
    assert_eq!(
        next(&mut listener).await,
        ListenItem::Error(RtdbListenError::Cancel)
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(server.count(), 1, "no reconnect after cancel");
}

#[tokio::test]
async fn sse_fatal_statuses_and_the_single_401_retry() {
    let server = Server::start(|_, _| Resp::Body(403, "{\"error\":\"nope\"}".into())).await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    let mut listener = db.listen("/a");
    assert!(matches!(
        next(&mut listener).await,
        ListenItem::Error(RtdbListenError::Http { status: 403, .. })
    ));
    assert_eq!(server.count(), 1);

    let server = Server::start(|req, _| {
        if req.query().contains("auth=fresh-token") {
            Resp::Sse(vec![sse("put", "/", json!(1))], true)
        } else {
            Resp::Body(401, "{\"error\":\"expired\"}".into())
        }
    })
    .await;
    let tokens = Arc::new(Tokens {
        forced: AtomicU32::new(0),
        calls: AtomicU32::new(0),
    });
    let db = client(&server, tokens.clone());
    let mut listener = db.listen("/a");
    assert!(matches!(next(&mut listener).await, ListenItem::Event(_)));
    assert_eq!(tokens.forced.load(Ordering::SeqCst), 1);
    assert_eq!(server.count(), 2, "the retry came at once");

    let server =
        Server::start(|_, _| Resp::Body(401, "{\"error\":\"Permission denied\"}".into())).await;
    let db = client(
        &server,
        Arc::new(Tokens {
            forced: AtomicU32::new(0),
            calls: AtomicU32::new(0),
        }),
    );
    let mut listener = db.listen("/a");
    assert!(matches!(
        next(&mut listener).await,
        ListenItem::Error(RtdbListenError::Http { status: 401, .. })
    ));
    assert_eq!(server.count(), 2);
}

#[tokio::test]
async fn sse_auth_revoked_reconnects_with_a_fresh_token() {
    let server = Server::start(|req, index| {
        if index == 0 {
            Resp::Sse(
                vec![
                    sse("put", "/", json!(1)),
                    "event: auth_revoked\ndata: \"x\"\n\n".into(),
                ],
                true,
            )
        } else {
            assert!(req.query().contains("auth=fresh-token"), "{}", req.query());
            Resp::Sse(vec![sse("put", "/", json!(2))], true)
        }
    })
    .await;
    let tokens = Arc::new(Tokens {
        forced: AtomicU32::new(0),
        calls: AtomicU32::new(0),
    });
    let db = client(&server, tokens.clone());
    let mut listener = db.listen("/a");
    let ListenItem::Event(first) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(first.data, json!(1));
    let ListenItem::Event(second) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(second.data, json!(2));
    assert_eq!(tokens.forced.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sse_garbage_is_not_trusted_and_closing_stops_everything() {
    let server = Server::start(|_, index| {
        if index == 0 {
            Resp::Sse(vec!["event: put\ndata: {not json\n\n".into()], false)
        } else {
            Resp::Sse(vec![sse("put", "/", json!("ok"))], true)
        }
    })
    .await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    let mut listener = db.listen("/a");
    let ListenItem::Event(e) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(
        e.data,
        json!("ok"),
        "the bad stream was dropped, the next one used"
    );
    listener.close();
    settle().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.count(), 2);
}

#[tokio::test]
async fn reconnect_on_request_skips_the_backoff() {
    let server =
        Server::start(|_, index| Resp::Sse(vec![sse("put", "/", json!(index))], true)).await;
    let db = client(&server, Arc::new(StaticToken("tok".into())));
    let mut listener = db.listen("/a");
    let ListenItem::Event(first) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(first.data, json!(0));
    listener.reconnect();
    let ListenItem::Event(second) = next(&mut listener).await else {
        panic!()
    };
    assert_eq!(second.data, json!(1));
}
