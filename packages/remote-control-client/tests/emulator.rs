//! The real client against the Realtime Database emulator. Skipped unless
//! `FIREBASE_DATABASE_EMULATOR_HOST` (host:port) is set, e.g. from `apps/firebase`:
//!
//! ```text
//! JAVA_HOME=/opt/homebrew/opt/openjdk@21 PATH=/opt/homebrew/opt/openjdk@21/bin:$PATH \
//!   firebase emulators:exec --only database --project demo-lingxi-backend --non-interactive \
//!   "cargo test -p remote-control-client --test emulator"
//! ```
//!
//! Tokens are unsigned (`alg: none`) ID tokens whose `sub` is the uid, which the
//! emulator accepts. The tests share one rule set (`remote-control/$uid` readable and
//! writable by that uid only), installed with the `owner` bearer token and restored
//! afterwards; every test works under its own uid and removes what it wrote. Because
//! the rules are shared, the tests run one after the other.

mod support;

use async_trait::async_trait;
use remote_control_client::clock::system_clock;
use remote_control_client::connection::{ConnectionPhase, ViewerConnection, ViewerConnectionDeps};
use remote_control_client::device::MemoryViewerKeys;
use remote_control_client::encoding::{b64u_encode, random_id};
use remote_control_client::keys::IdentityKey;
use remote_control_client::rpc::{RequestHandler, RpcError, RpcOptions, RpcPeer};
use remote_control_client::rtdb::{
    ListenItem, MemoryRtdb, Rtdb, RtdbClient, RtdbClientOptions, RtdbError, RtdbListenError,
    RtdbMirror, TokenError, TokenProvider,
};
use remote_control_client::signaling::{server_timestamp, DevicePlatform};
use remote_control_client::testing::FakePeerNetwork;
use remote_control_client::tunnel::{TunnelLink, TunnelLinkOptions};
use remote_control_client::tunnel_crypto::TunnelDir;
use remote_control_client::version::RTDB_EMULATOR_NAMESPACE;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use support::host::FakeHost;
use support::HostInbox;
use tokio::sync::Mutex as AsyncMutex;

const OWNER_ONLY: &str = r#"{"rules":{"remote-control":{"$uid":{".read":"auth != null && auth.uid === $uid",".write":"auth != null && auth.uid === $uid"}}}}"#;
const WRITE_ONLY: &str = r#"{"rules":{"remote-control":{"$uid":{".read":"false",".write":"auth != null && auth.uid === $uid"}}}}"#;

/// The tests share the emulator's rules, so they take turns.
static TURN: AsyncMutex<()> = AsyncMutex::const_new(());

fn emulator() -> Option<String> {
    match std::env::var("FIREBASE_DATABASE_EMULATOR_HOST") {
        Ok(host) if !host.is_empty() => Some(format!("http://{host}")),
        _ => {
            eprintln!("skipped: set FIREBASE_DATABASE_EMULATOR_HOST (host:port) to run against the emulator");
            None
        }
    }
}

fn rules_url(origin: &str) -> String {
    format!("{origin}/.settings/rules.json?ns={RTDB_EMULATOR_NAMESPACE}")
}

async fn install_rules(origin: &str, rules: &str) {
    let response = reqwest::Client::new()
        .put(rules_url(origin))
        .header("Authorization", "Bearer owner")
        .body(rules.to_owned())
        .send()
        .await
        .expect("the emulator answers");
    assert_eq!(response.status(), 200, "installing the rules");
}

/// Installs the uid rules for the length of a test and puts the earlier ones back.
struct Rules {
    origin: String,
    earlier: Option<String>,
}

impl Rules {
    async fn install(origin: &str) -> Self {
        let earlier = match reqwest::Client::new()
            .get(rules_url(origin))
            .header("Authorization", "Bearer owner")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.text().await.ok(),
            _ => None,
        };
        install_rules(origin, OWNER_ONLY).await;
        Self {
            origin: origin.to_owned(),
            earlier,
        }
    }

    async fn restore(self) {
        if let Some(earlier) = &self.earlier {
            install_rules(&self.origin, earlier).await;
        }
    }
}

fn id_token(uid: &str) -> String {
    let part = |value: Value| b64u_encode(value.to_string().as_bytes());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({
        "iss": "https://securetoken.google.com/demo-lingxi-backend",
        "aud": "demo-lingxi-backend",
        "auth_time": now, "user_id": uid, "sub": uid, "iat": now, "exp": now + 3600,
        "firebase": { "identities": {}, "sign_in_provider": "password" },
    });
    format!(
        "{}.{}.",
        part(json!({ "alg": "none", "typ": "JWT" })),
        part(claims)
    )
}

struct Tokens {
    uid: String,
    forced: AtomicU32,
    calls: AtomicU32,
    log: std::sync::Mutex<Vec<bool>>,
}

impl Tokens {
    fn new(uid: &str) -> Arc<Self> {
        Arc::new(Self {
            uid: uid.to_owned(),
            forced: AtomicU32::new(0),
            calls: AtomicU32::new(0),
            log: Default::default(),
        })
    }
}

#[async_trait]
impl TokenProvider for Tokens {
    async fn id_token(&self, force_refresh: bool) -> Result<String, TokenError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push(force_refresh);
        if force_refresh {
            self.forced.fetch_add(1, Ordering::SeqCst);
        }
        Ok(id_token(&self.uid))
    }
}

fn client(origin: &str, tokens: Arc<dyn TokenProvider>) -> RtdbClient {
    let mut options = RtdbClientOptions::new(origin, tokens);
    options.namespace = Some(RTDB_EMULATOR_NAMESPACE.into());
    RtdbClient::new(options)
}

fn new_uid() -> String {
    format!("rc-test-{}", random_id())
}

/// Arrays to index-keyed objects, nulls dropped: the shape RtdbMirror keeps.
fn plain(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Object(
            items
                .iter()
                .enumerate()
                .filter(|(_, v)| !v.is_null())
                .map(|(i, v)| (i.to_string(), plain(v)))
                .collect(),
        ),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), plain(v))).collect())
        }
        other => other.clone(),
    }
}

async fn eventually<F: std::future::Future<Output = bool>>(
    what: &str,
    mut check: impl FnMut() -> F,
) {
    for _ in 0..300 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn next_item(listener: &mut remote_control_client::rtdb::Listener) -> ListenItem {
    tokio::time::timeout(Duration::from_secs(15), listener.recv())
        .await
        .expect("an item in time")
        .expect("the stream is open")
}

#[tokio::test]
async fn rest_behaves_like_the_in_memory_model() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let uid = new_uid();
    let root = format!("/remote-control/{uid}/rest");
    let real = client(&origin, Tokens::new(&uid));
    let model = MemoryRtdb::new().coerce_arrays(true);
    let steps: Vec<(&str, Value)> = vec![
        ("a", json!({ "x": 1, "y": { "z": "two" } })),
        ("dense", json!({ "0": "a", "1": "b", "2": "c" })),
        ("holes", json!({ "0": "a", "2": "c" })),
        ("sparse", json!({ "0": "a", "5": "b" })),
        ("half", json!({ "0": "a", "3": "b" })),
        ("edge", json!({ "0": "a", "4": "b" })),
        ("single0", json!({ "0": "a" })),
        ("single1", json!({ "1": "a" })),
        ("single2", json!({ "2": "a" })),
        ("leading-zero", json!({ "00": "a", "1": "b" })),
        (
            "nested",
            json!({ "list": { "0": { "0": "p", "1": "q" }, "1": "r" } }),
        ),
        ("prefixed", json!({ "f0": "a", "f1": "b" })),
        ("empty", json!({})),
        ("gone", json!({ "k": "v" })),
    ];
    for (name, value) in &steps {
        real.put(&format!("{root}/{name}"), value).await.unwrap();
        model.put(&format!("{root}/{name}"), value).await.unwrap();
    }
    let mut patch = Map::new();
    patch.insert("x".into(), Value::Null);
    patch.insert("y/w".into(), json!(3));
    real.patch(&format!("{root}/a"), &patch).await.unwrap();
    model.patch(&format!("{root}/a"), &patch).await.unwrap();
    for db in [&real as &dyn Rtdb, &model as &dyn Rtdb] {
        db.remove(&format!("{root}/gone")).await.unwrap();
        db.remove(&format!("{root}/a/y/z")).await.unwrap();
    }
    assert_eq!(
        real.get(&root).await.unwrap(),
        model.get(&root).await.unwrap()
    );

    real.put(
        &format!("{root}/stamp"),
        &json!({ "created_at": server_timestamp() }),
    )
    .await
    .unwrap();
    let stamped = real
        .get(&format!("{root}/stamp/created_at"))
        .await
        .unwrap()
        .as_u64()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(stamped.abs_diff(now) < 60_000);
    assert_eq!(
        real.get(&format!("{root}/nothing/here")).await.unwrap(),
        Value::Null
    );

    real.remove(&format!("/remote-control/{uid}"))
        .await
        .unwrap();
    rules.restore().await;
}

#[tokio::test]
async fn listen_delivers_an_initial_put_then_events_and_repeats_it_after_a_reconnect() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let uid = new_uid();
    let root = format!("/remote-control/{uid}/listen");
    let real = client(&origin, Tokens::new(&uid));
    real.put(&format!("{root}/seed"), &json!("before"))
        .await
        .unwrap();
    let mut listener = real.listen(&root);
    let mut mirror = RtdbMirror::new();
    let ListenItem::Event(first) = next_item(&mut listener).await else {
        panic!()
    };
    assert_eq!(
        (first.path.as_str(), first.data.clone()),
        ("/", json!({ "seed": "before" }))
    );
    mirror.apply(&first);

    real.put(&format!("{root}/c1/offer"), &json!({ "n": 1 }))
        .await
        .unwrap();
    let mut patch = Map::new();
    patch.insert("up/f0".into(), json!("x"));
    patch.insert("up/f1".into(), json!("y"));
    real.patch(&format!("{root}/c1"), &patch).await.unwrap();
    real.put(&format!("{root}/c1/up/f0"), &Value::Null)
        .await
        .unwrap();
    real.put(&format!("{root}/arr"), &json!({ "0": "a", "1": "b" }))
        .await
        .unwrap();
    real.remove(&format!("{root}/seed")).await.unwrap();
    let expected =
        plain(&json!({ "c1": { "offer": { "n": 1 }, "up": { "f1": "y" } }, "arr": ["a", "b"] }));
    while mirror.get("/") != expected {
        let ListenItem::Event(event) = next_item(&mut listener).await else {
            panic!("stream ended")
        };
        mirror.apply(&event);
    }

    listener.reconnect();
    let ListenItem::Event(again) = next_item(&mut listener).await else {
        panic!()
    };
    assert_eq!(again.path, "/");
    assert_eq!(plain(&again.data), expected);
    assert!(
        mirror.apply(&again).is_empty(),
        "the repeated snapshot changes nothing"
    );

    listener.close();
    real.remove(&format!("/remote-control/{uid}"))
        .await
        .unwrap();
    rules.restore().await;
}

struct Echo;

#[async_trait]
impl RequestHandler for Echo {
    async fn handle(&self, _method: &str, params: Value) -> Result<Value, RpcError> {
        Ok(json!({ "echoed": params }))
    }
}

#[tokio::test]
async fn a_tunnel_runs_over_real_streams() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let uid = new_uid();
    let host_id = random_id();
    let conn_id = random_id();
    let viewer_db = Arc::new(client(&origin, Tokens::new(&uid)));
    let host_db = Arc::new(client(&origin, Tokens::new(&uid)));
    let up = remote_control_client::signaling::up_path(&uid, &host_id, &conn_id).unwrap();
    let down = remote_control_client::signaling::down_path(&uid, &host_id, &conn_id).unwrap();
    let make_keys =
        || remote_control_client::tunnel_crypto::derive_tunnel_keys(&[9u8; 32], &conn_id).unwrap();
    let (vk, hk) = (make_keys(), make_keys());
    let (viewer_link, viewer_events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: viewer_db.clone() as _,
        conn_id: conn_id.clone(),
        send_path: up.clone(),
        send_dir: TunnelDir::Up,
        send_key: vk.up,
        recv_path: down.clone(),
        recv_dir: TunnelDir::Down,
        recv_key: vk.down,
        subscribe: true,
    });
    let (host_link, host_events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: host_db.clone() as _,
        conn_id: conn_id.clone(),
        send_path: down.clone(),
        send_dir: TunnelDir::Down,
        send_key: hk.down,
        recv_path: up.clone(),
        recv_dir: TunnelDir::Up,
        recv_key: hk.up,
        subscribe: false,
    });
    let inbox = HostInbox::start(host_db.as_ref(), &uid, &host_id);
    inbox.register(&conn_id, host_link.clone());
    let viewer_rpc = RpcPeer::start(
        viewer_link.clone().into_link(viewer_events),
        RpcOptions::default(),
    );
    let host_rpc = RpcPeer::start(
        host_link.clone().into_link(host_events),
        RpcOptions {
            timeout: None,
            requests: Some(Arc::new(Echo)),
            notifications: None,
        },
    );
    let payload = "tunnel ".repeat(45_000); // about 300 KB
    let answer = viewer_rpc
        .request(
            "echo",
            Some(json!({ "payload": payload })),
            Some(Duration::from_secs(60)),
        )
        .await
        .unwrap();
    assert_eq!(answer["echoed"]["payload"], json!(payload));

    viewer_rpc.close("done");
    host_rpc.close("done");
    host_link.when_closed().await;
    viewer_link.when_closed().await;
    drop(inbox);
    assert_eq!(viewer_db.get(&up).await.unwrap(), Value::Null);
    assert_eq!(host_db.get(&down).await.unwrap(), Value::Null);
    viewer_db
        .remove(&format!("/remote-control/{uid}"))
        .await
        .unwrap();
    rules.restore().await;
}

#[tokio::test]
async fn another_users_subtree_is_refused() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let (me, other) = (new_uid(), new_uid());
    let tokens = Tokens::new(&me);
    let real = client(&origin, tokens.clone());
    let denied = |error: RtdbError| error.status == 401 && error.message == "Permission denied";
    assert!(denied(
        real.put(&format!("/remote-control/{other}/x"), &json!(1))
            .await
            .unwrap_err()
    ));
    assert!(denied(
        real.get(&format!("/remote-control/{other}/x"))
            .await
            .unwrap_err()
    ));
    assert_eq!(
        tokens.forced.load(Ordering::SeqCst),
        2,
        "each request is tried again once with a fresh token"
    );
    assert_eq!(tokens.calls.load(Ordering::SeqCst), 4);

    let mut listener = real.listen(&format!("/remote-control/{other}"));
    let item = next_item(&mut listener).await;
    assert!(
        matches!(item, ListenItem::Error(RtdbListenError::Http { status: 401, ref message }) if message == "Permission denied"),
        "{item:?}"
    );
    assert_eq!(
        *tokens.log.lock().unwrap(),
        vec![false, true, false, true, false, true],
        "a first 401 is retried once with a fresh token"
    );
    // Final: nothing more arrives.
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), listener.recv())
            .await
            .map_or(true, |item| item.is_none())
    );

    real.put(&format!("/remote-control/{me}/x"), &json!(1))
        .await
        .unwrap();
    assert_eq!(
        real.get(&format!("/remote-control/{me}/x")).await.unwrap(),
        json!(1)
    );
    real.remove(&format!("/remote-control/{me}")).await.unwrap();
    rules.restore().await;
}

#[tokio::test]
async fn read_access_withdrawn_from_a_live_stream_cancels_it_once() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let uid = new_uid();
    let path = format!("/remote-control/{uid}/live");
    let real = client(&origin, Tokens::new(&uid));
    real.put(&format!("{path}/x"), &json!(1)).await.unwrap();
    let mut listener = real.listen(&path);
    assert!(matches!(
        next_item(&mut listener).await,
        ListenItem::Event(_)
    ));

    install_rules(&origin, WRITE_ONLY).await;
    let item = tokio::time::timeout(Duration::from_secs(20), listener.recv())
        .await
        .expect("the stream ends");
    assert_eq!(item, Some(ListenItem::Error(RtdbListenError::Cancel)));
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), listener.recv())
            .await
            .map_or(true, |i| i.is_none())
    );

    install_rules(&origin, OWNER_ONLY).await;
    real.remove(&format!("/remote-control/{uid}"))
        .await
        .unwrap();
    rules.restore().await;
}

#[tokio::test]
async fn a_viewer_pairs_with_a_fake_desktop_through_the_emulator() {
    let Some(origin) = emulator() else { return };
    let _turn = TURN.lock().await;
    let rules = Rules::install(&origin).await;
    let uid = new_uid();
    let viewer_db = Arc::new(client(&origin, Tokens::new(&uid)));
    let host_db = Arc::new(client(&origin, Tokens::new(&uid)));
    let host = FakeHost::start(
        host_db.clone() as _,
        system_clock(),
        FakePeerNetwork::new(),
        &uid,
        IdentityKey::generate(),
    );
    host.publish_presence().await;

    let keys = MemoryViewerKeys::new();
    let deps = ViewerConnectionDeps::new(
        &uid,
        host.host_id.clone(),
        viewer_db.clone() as _,
        keys.clone() as _,
        DevicePlatform::Android,
    );
    let conn = ViewerConnection::new(deps);
    conn.start();
    eventually("the pairing code", || async {
        conn.state().phase == ConnectionPhase::AwaitingApproval
    })
    .await;
    let conn_id = host.conns()[0].conn_id.clone();
    host.approve(&conn_id).await;
    eventually("ready", || async {
        conn.state().phase == ConnectionPhase::Ready
    })
    .await;
    assert_eq!(keys.pinned_hosts().len(), 1);
    assert_eq!(
        conn.request("models.list", Some(json!({})), None)
            .await
            .unwrap(),
        json!({})
    );

    conn.stop();
    let (v, h) = (viewer_db.clone(), host.host_id.clone());
    let uid2 = uid.clone();
    eventually("the connection's nodes to be removed", move || {
        let (v, h, uid) = (v.clone(), h.clone(), uid2.clone());
        async move {
            let to_host = v
                .get(&remote_control_client::signaling::to_host_path(&uid, &h).unwrap())
                .await
                .unwrap();
            let to_viewer = v
                .get(&remote_control_client::signaling::to_viewer_path(&uid, &h).unwrap())
                .await
                .unwrap();
            to_host.is_null() && to_viewer.is_null()
        }
    })
    .await;
    viewer_db
        .remove(&format!("/remote-control/{uid}"))
        .await
        .unwrap();
    rules.restore().await;
}
