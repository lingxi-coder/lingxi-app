//! The viewer state machine against a fake desktop over an in-memory RTDB.

mod support;

use remote_control_client::clock::{Clock, ManualClock};
use remote_control_client::connection::{
    ConnectionListener, ConnectionPhase, ConnectionState, FailureKind, ViewerConnection,
    ViewerConnectionDeps,
};
use remote_control_client::device::{MemoryViewerKeys, PinnedHost, TunnelMemory, ViewerKeys};
use remote_control_client::keys::IdentityKey;
use remote_control_client::media::{
    RemoteMediaAnswer, RemoteMediaConnection, RemoteMediaControlRequest, RemoteMediaControlResult,
    RemoteMediaOffer, RemoteMediaOfferHandler, MEDIA_CONTROL_METHOD,
};
use remote_control_client::messages::methods;
use remote_control_client::rtdb::{MemoryRtdb, Rtdb, RtdbError};
use remote_control_client::signaling::{
    to_host_path, to_viewer_path, AnswerRejectReason, DevicePlatform, TransportKind,
};
use remote_control_client::testing::{FakePeerNetwork, PeerBehavior};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::host::FakeHost;
use support::{advance, settle, UID};

/// Fails the viewer's offer writes a given number of times.
struct FlakyOffers {
    inner: Arc<MemoryRtdb>,
    failures: AtomicU32,
}

#[async_trait::async_trait]
impl remote_control_client::rtdb::Rtdb for FlakyOffers {
    async fn get(&self, path: &str) -> Result<Value, RtdbError> {
        self.inner.get(path).await
    }
    async fn put(&self, path: &str, value: &Value) -> Result<(), RtdbError> {
        if path.ends_with("/offer")
            && self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return Err(RtdbError::new(500, "boom"));
        }
        self.inner.put(path, value).await
    }
    async fn patch(
        &self,
        path: &str,
        value: &serde_json::Map<String, Value>,
    ) -> Result<(), RtdbError> {
        self.inner.patch(path, value).await
    }
    async fn remove(&self, path: &str) -> Result<(), RtdbError> {
        self.inner.remove(path).await
    }
    fn listen(&self, path: &str) -> remote_control_client::rtdb::Listener {
        self.inner.listen(path)
    }
}

#[derive(Default)]
struct Recorder {
    phases: Mutex<Vec<ConnectionPhase>>,
    ready: Mutex<u32>,
    notifications: Mutex<Vec<(String, Value)>>,
}

#[derive(Default)]
struct TestMediaOfferHandler {
    accepted: Mutex<Vec<(RemoteMediaConnection, RemoteMediaOffer)>>,
    closed: Mutex<Vec<RemoteMediaConnection>>,
}

#[async_trait::async_trait]
impl RemoteMediaOfferHandler for TestMediaOfferHandler {
    async fn accept_offer(
        &self,
        connection: &RemoteMediaConnection,
        offer: RemoteMediaOffer,
        _control: Arc<dyn remote_control_client::media::RemoteMediaControlChannel>,
    ) -> Result<RemoteMediaAnswer, remote_control_client::rpc::RpcError> {
        self.accepted
            .lock()
            .unwrap()
            .push((connection.clone(), offer.clone()));
        Ok(RemoteMediaAnswer {
            v: 1,
            media_session_ref: offer.media_session_ref,
            answer_sdp: offer.offer_sdp,
        })
    }

    async fn control(
        &self,
        _connection: &RemoteMediaConnection,
        request: RemoteMediaControlRequest,
    ) -> Result<RemoteMediaControlResult, remote_control_client::rpc::RpcError> {
        Ok(RemoteMediaControlResult {
            v: 1,
            media_session_ref: request.media_session_ref,
            generation: request.generation,
            audio_sequence: request.target_audio_sequence,
            accepted_sample_frames: request.target_sample_frames,
            played_sample_frames: request.target_sample_frames,
            rendered_at_unix_ms: Some(1_761_502_000_000),
        })
    }

    async fn connection_closed(&self, connection: &RemoteMediaConnection) {
        self.closed.lock().unwrap().push(connection.clone());
    }
}

impl ConnectionListener for Recorder {
    fn on_state(&self, state: &ConnectionState) {
        let mut phases = self.phases.lock().unwrap();
        if phases.last() != Some(&state.phase) {
            phases.push(state.phase);
        }
    }
    fn on_ready(&self) {
        *self.ready.lock().unwrap() += 1;
    }
    fn on_notification(&self, method: &str, params: &Value) {
        self.notifications
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
    }
}

struct Rig {
    rtdb: Arc<MemoryRtdb>,
    clock: Arc<ManualClock>,
    net: FakePeerNetwork,
    host: Arc<FakeHost>,
    keys: Arc<MemoryViewerKeys>,
    memory: Arc<TunnelMemory>,
    recorder: Arc<Recorder>,
    conn: ViewerConnection,
}

fn deps_for(
    rig_parts: (
        &Arc<MemoryRtdb>,
        &Arc<ManualClock>,
        &Arc<MemoryViewerKeys>,
        &Arc<TunnelMemory>,
    ),
    host_id: &str,
) -> ViewerConnectionDeps {
    let (rtdb, clock, keys, memory) = rig_parts;
    let mut deps = ViewerConnectionDeps::new(
        UID,
        host_id,
        Arc::clone(rtdb) as _,
        Arc::clone(keys) as _,
        DevicePlatform::Android,
    );
    deps.clock = Arc::clone(clock) as _;
    deps.random = Arc::new(|| 0.0);
    deps.tunnel_memory = Some(Arc::clone(memory));
    deps
}

fn make_rig(with_peers: bool) -> Rig {
    make_rig_with(with_peers, 0)
}

fn make_rig_with(with_peers: bool, offer_failures: u32) -> Rig {
    make_rig_with_media_handler(with_peers, offer_failures, None)
}

fn make_rig_with_media_handler(
    with_peers: bool,
    offer_failures: u32,
    media_offer_handler: Option<Arc<dyn RemoteMediaOfferHandler>>,
) -> Rig {
    let clock = ManualClock::new(1_800_000_000_000);
    let rtdb = Arc::new(MemoryRtdb::with_clock(Arc::clone(&clock) as _));
    let net = FakePeerNetwork::new();
    let host = FakeHost::start(
        Arc::clone(&rtdb) as _,
        Arc::clone(&clock) as _,
        net.clone(),
        UID,
        IdentityKey::generate(),
    );
    let keys = MemoryViewerKeys::new();
    let memory = Arc::new(TunnelMemory::new(Arc::clone(&clock) as _));
    let mut deps = deps_for((&rtdb, &clock, &keys, &memory), &host.host_id);
    deps.media_offer_handler = media_offer_handler;
    if offer_failures > 0 {
        deps.rtdb = Arc::new(FlakyOffers {
            inner: Arc::clone(&rtdb),
            failures: AtomicU32::new(offer_failures),
        });
    }
    if with_peers {
        deps.peers = Some(net.connector());
    }
    let conn = ViewerConnection::new(deps);
    let recorder = Arc::new(Recorder::default());
    conn.add_listener(recorder.clone());
    Rig {
        rtdb,
        clock,
        net,
        host,
        keys,
        memory,
        recorder,
        conn,
    }
}

impl Rig {
    /// Pin the host and make the host know this device: an earlier pairing.
    async fn pair_beforehand(&self) {
        let identity = self.keys.identity().await.unwrap();
        self.host.know_device(&identity.id);
        self.keys
            .pin(PinnedHost {
                host_id: self.host.host_id.clone(),
                host_key: self.host.host_key_b64u(),
                name: "Studio Mac".into(),
                paired_at: 1,
            })
            .await;
    }

    async fn wait(&self, what: &str, done: impl Fn(&ConnectionState) -> bool) -> ConnectionState {
        for i in 0..2000 {
            settle().await;
            let state = self.conn.state();
            if done(&state) {
                return state;
            }
            let step = if i < 400 { 5 } else { 250 };
            tokio::time::sleep(Duration::from_millis(step)).await;
        }
        panic!(
            "timed out waiting for {what}; state: {:?}",
            self.conn.state()
        );
    }

    async fn phase(&self, phase: ConnectionPhase) -> ConnectionState {
        self.wait(&format!("{phase:?}"), |s| s.phase == phase).await
    }

    fn nodes(&self) -> (Value, Value) {
        (
            self.rtdb
                .read(&to_host_path(UID, &self.host.host_id).unwrap()),
            self.rtdb
                .read(&to_viewer_path(UID, &self.host.host_id).unwrap()),
        )
    }
}

fn failure(state: &ConnectionState) -> (FailureKind, String, bool) {
    let f = state.failure.clone().expect("failure");
    (f.kind, f.detail.unwrap_or_default(), f.auto)
}

#[tokio::test(start_paused = true)]
async fn first_pairing_over_the_tunnel() {
    let rig = make_rig(false);
    rig.host.publish_presence().await;
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::AwaitingApproval).await;
    let code = state.pairing_code.clone().expect("code");
    assert_eq!(code.len(), 6);
    assert!(
        rig.keys.pinned_hosts().is_empty(),
        "nothing is pinned before approval"
    );

    let conn_id = rig.host.conns()[0].conn_id.clone();
    // Until approved the host refuses everything else.
    let error = rig
        .conn
        .request("session.list", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        remote_control_client::rpc::RemoteErrorCode::Closed
    );

    rig.host.approve(&conn_id).await;
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Tunnel));
    assert_eq!(
        state.host.as_ref().map(|h| h.name.as_str()),
        Some("Studio Mac")
    );
    assert!(state.ever_ready);
    assert_eq!(state.pairing_code, None);
    settle().await;
    let pinned = rig.keys.pinned_hosts();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].host_key, rig.host.host_key_b64u());
    assert_eq!(*rig.recorder.ready.lock().unwrap(), 1);
    assert_eq!(
        *rig.recorder.phases.lock().unwrap(),
        vec![
            ConnectionPhase::Connecting,
            ConnectionPhase::AwaitingApproval,
            ConnectionPhase::Ready
        ]
    );

    // Requests reach the host and answers return.
    let answer = rig
        .conn
        .request("models.list", Some(json!({})), None)
        .await
        .unwrap();
    assert_eq!(answer, json!({}));
    let init = &rig.host.requests()[0];
    assert_eq!(init.1, methods::INITIALIZE);
    assert_eq!(init.2["protocol"], 1);

    rig.conn.stop();
    advance(Duration::from_secs(12)).await;
    assert_eq!(rig.conn.state().phase, ConnectionPhase::Idle);
    assert_eq!(
        rig.nodes(),
        (Value::Null, Value::Null),
        "everything of the connection is removed"
    );
    assert_eq!(
        rig.rtdb.listener_count(),
        1,
        "only the host's own listener is left"
    );
}

#[tokio::test(start_paused = true)]
async fn host_media_offer_is_denied_before_approval_then_bound_and_stopped_with_its_connection() {
    let handler = Arc::new(TestMediaOfferHandler::default());
    let rig = make_rig_with_media_handler(false, 0, Some(handler.clone()));
    rig.host.publish_presence().await;
    rig.conn.start();
    rig.phase(ConnectionPhase::AwaitingApproval).await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    let offer = json!({
        "v": 1,
        "media_session_ref": "media_0123456789abcdef",
        "session_id": "12121212-3434-4567-8899-aaaaaaaaaaaa",
        "generation": 1,
        "mode": "audio_video",
        "intent": "conversation",
        "offer_sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"
    });
    let denied = rig
        .host
        .request(&conn_id, "media.offer", offer.clone())
        .await
        .expect_err("a pending pairing cannot negotiate media");
    assert_eq!(
        denied.code,
        remote_control_client::rpc::RemoteErrorCode::NotApproved
    );
    assert!(handler.accepted.lock().unwrap().is_empty());

    rig.host.approve(&conn_id).await;
    rig.phase(ConnectionPhase::Ready).await;
    let answer = rig
        .host
        .request(&conn_id, "media.offer", offer)
        .await
        .expect("approved control channel answers the offer");
    assert_eq!(answer["media_session_ref"], "media_0123456789abcdef");
    let identity = rig.keys.identity().await.unwrap();
    let accepted = handler.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].0.account_uid, UID);
    assert_eq!(accepted[0].0.host_id, rig.host.host_id);
    assert_eq!(accepted[0].0.device_id, identity.id);
    assert_eq!(accepted[0].0.connection_id, conn_id);
    drop(accepted);

    let control = rig
        .host
        .request(
            &conn_id,
            MEDIA_CONTROL_METHOD,
            json!({
                "v": 1,
                "media_session_ref": "media_0123456789abcdef",
                "generation": 1,
                "action": "flush_audio",
                "target_audio_sequence": 1,
                "target_sample_frames": 960
            }),
        )
        .await
        .expect("approved session flush waits for the native receipt");
    assert_eq!(control["played_sample_frames"], 960);

    rig.conn.stop();
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(handler.closed.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn approved_reconnect_over_a_direct_link() {
    let rig = make_rig(true);
    rig.pair_beforehand().await;
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Webrtc));
    assert_eq!(rig.net.created(), 1);
    assert_eq!(*rig.recorder.ready.lock().unwrap(), 1);
    assert!(!rig
        .recorder
        .phases
        .lock()
        .unwrap()
        .contains(&ConnectionPhase::AwaitingApproval));

    // Notifications in order, straight after the handshake.
    let conn_id = rig.host.conns()[0].conn_id.clone();
    for i in 0..3 {
        rig.host
            .notify(&conn_id, "session.event", json!({ "i": i }))
            .await;
    }
    settle().await;
    let seen: Vec<i64> = rig
        .recorder
        .notifications
        .lock()
        .unwrap()
        .iter()
        .map(|(_, p)| p["i"].as_i64().unwrap())
        .collect();
    assert_eq!(seen, vec![0, 1, 2]);

    // The link drops: reconnect after the backoff, with a fresh connection id.
    rig.host.drop_connection(&conn_id, "restart");
    let state = rig.phase(ConnectionPhase::Reconnecting).await;
    let (kind, detail, auto) = failure(&state);
    assert_eq!((kind, auto), (FailureKind::Closed, true));
    assert!(!detail.is_empty());
    assert_eq!(state.attempt, 1);
    assert!(state.retry_at.is_some());
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.failure, None);
    assert_eq!(*rig.recorder.ready.lock().unwrap(), 2);
    assert_eq!(rig.host.conns().len(), 2);
    assert_ne!(rig.host.conns()[1].conn_id, conn_id);
    assert_eq!(
        rig.nodes().0.as_object().map(|o| o.len()),
        Some(1),
        "the dead connection was cleaned up"
    );
}

#[tokio::test(start_paused = true)]
async fn direct_path_fails_and_the_tunnel_takes_over() {
    let rig = make_rig(true);
    rig.pair_beforehand().await;
    rig.net.script([PeerBehavior::Hang]);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Tunnel));
    assert_eq!(
        rig.host.offers_seen().len(),
        2,
        "a second offer on a new connection id"
    );
    let offers = rig.host.offers_seen();
    assert_ne!(offers[0], offers[1]);
    assert!(rig.memory.prefers(&rig.host.host_id));
    assert_eq!(rig.net.created(), 1);
    settle().await;
    // The abandoned connection left nothing behind.
    advance(Duration::from_secs(1)).await;
    let (to_host, _) = rig.nodes();
    let left: Vec<&String> = to_host.as_object().unwrap().keys().collect();
    assert_eq!(left, vec![&offers[1]]);

    // A later connection goes straight to the tunnel while the memory lasts ...
    rig.conn.stop();
    advance(Duration::from_secs(12)).await;
    rig.conn.start();
    rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(rig.net.created(), 1, "no direct attempt within ten minutes");
    assert_eq!(rig.host.offers_seen().len(), 3);

    // ... and tries the direct path again afterwards.
    rig.conn.stop();
    advance(Duration::from_secs(12)).await;
    rig.clock.advance_ms(10 * 60_000 + 1);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Webrtc));
    assert_eq!(rig.net.created(), 2);
}

#[tokio::test(start_paused = true)]
async fn ice_failure_falls_back_too() {
    let rig = make_rig(true);
    rig.pair_beforehand().await;
    rig.net.script([PeerBehavior::IceFailed]);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Tunnel));
    assert_eq!(rig.host.offers_seen().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn no_webrtc_available_means_tunnel_without_a_failed_attempt() {
    let rig = make_rig(true);
    rig.pair_beforehand().await;
    rig.net.script([PeerBehavior::CreateFails]);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Tunnel));
    assert_eq!(rig.host.offers_seen().len(), 1);
    assert!(rig.memory.prefers(&rig.host.host_id));
}

#[tokio::test(start_paused = true)]
async fn the_host_may_downgrade_to_the_tunnel_on_the_same_connection() {
    let rig = make_rig(true);
    rig.pair_beforehand().await;
    rig.host.configure(|c| c.downgrade = true);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Ready).await;
    assert_eq!(state.transport, Some(TransportKind::Tunnel));
    assert_eq!(rig.host.offers_seen().len(), 1, "no second offer");
    assert!(rig.memory.prefers(&rig.host.host_id));
    rig.conn.request("models.list", None, None).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn force_tunnel_never_creates_a_peer() {
    let clock = ManualClock::new(1_800_000_000_000);
    let rtdb = Arc::new(MemoryRtdb::with_clock(Arc::clone(&clock) as _));
    let net = FakePeerNetwork::new();
    let host = FakeHost::start(
        Arc::clone(&rtdb) as _,
        Arc::clone(&clock) as _,
        net.clone(),
        UID,
        IdentityKey::generate(),
    );
    let keys = MemoryViewerKeys::new();
    let id = keys.identity().await.unwrap().id;
    host.know_device(&id);
    host.publish_presence().await;
    let memory = Arc::new(TunnelMemory::new(Arc::clone(&clock) as _));
    let mut deps = deps_for((&rtdb, &clock, &keys, &memory), &host.host_id);
    deps.peers = Some(net.connector());
    deps.force_tunnel = true;
    let conn = ViewerConnection::new(deps);
    conn.start();
    for _ in 0..200 {
        settle().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        if conn.state().phase == ConnectionPhase::Ready {
            break;
        }
    }
    assert_eq!(conn.state().transport, Some(TransportKind::Tunnel));
    assert_eq!(net.created(), 0);
}

#[tokio::test(start_paused = true)]
async fn no_answer_in_thirty_seconds() {
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    rig.host.configure(|c| c.answer = false);
    rig.conn.start();
    advance(Duration::from_secs(29)).await;
    assert_eq!(rig.conn.state().phase, ConnectionPhase::Connecting);
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(
        failure(&state),
        (FailureKind::NoAnswer, String::new(), false)
    );
    advance(Duration::from_secs(2)).await;
    assert_eq!(
        rig.nodes(),
        (Value::Null, Value::Null),
        "the unanswered offer is deleted"
    );

    // The user can try again.
    rig.host.configure(|c| c.answer = true);
    rig.conn.retry();
    rig.phase(ConnectionPhase::Ready).await;
}

#[tokio::test(start_paused = true)]
async fn refusals_by_the_host() {
    let cases = [
        (AnswerRejectReason::Revoked, FailureKind::Revoked, false),
        (AnswerRejectReason::Disabled, FailureKind::Disabled, false),
        (
            AnswerRejectReason::UpgradeRequired,
            FailureKind::UpgradeRequired,
            false,
        ),
        (AnswerRejectReason::Expired, FailureKind::Expired, false),
        (AnswerRejectReason::Capacity, FailureKind::Capacity, true),
        (AnswerRejectReason::Busy, FailureKind::Busy, true),
    ];
    for (reason, kind, auto) in cases {
        let rig = make_rig(false);
        rig.pair_beforehand().await;
        rig.host.configure(|c| c.reject = Some(reason));
        rig.conn.start();
        let want = if auto {
            ConnectionPhase::Reconnecting
        } else {
            ConnectionPhase::Failed
        };
        let state = rig.phase(want).await;
        assert_eq!(failure(&state).0, kind, "{reason:?}");
        assert_eq!(failure(&state).2, auto);
        if kind == FailureKind::Revoked {
            assert!(rig.keys.pinned_hosts().is_empty(), "revoked clears the pin");
        } else {
            assert_eq!(rig.keys.pinned_hosts().len(), 1);
        }
        rig.conn.stop();
    }
}

#[tokio::test(start_paused = true)]
async fn repairing_after_revocation_uses_a_new_device_key() {
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    let old = rig.keys.identity().await.unwrap().id;
    rig.host
        .configure(|c| c.reject = Some(AnswerRejectReason::Revoked));
    rig.conn.start();
    rig.phase(ConnectionPhase::Failed).await;
    rig.host.configure(|c| c.reject = None);
    rig.host.publish_presence().await;
    rig.conn.repair().await;
    let state = rig.phase(ConnectionPhase::AwaitingApproval).await;
    assert!(state.pairing_code.is_some());
    assert_ne!(rig.keys.identity().await.unwrap().id, old);
}

#[tokio::test(start_paused = true)]
async fn the_user_denies_the_pairing() {
    let rig = make_rig(false);
    rig.host.publish_presence().await;
    rig.conn.start();
    rig.phase(ConnectionPhase::AwaitingApproval).await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.deny(&conn_id, "denied").await;
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(
        failure(&state),
        (FailureKind::Denied, "denied".into(), false)
    );
    assert!(rig.keys.pinned_hosts().is_empty());
    advance(Duration::from_secs(12)).await;
    assert_eq!(rig.nodes(), (Value::Null, Value::Null));
}

#[tokio::test(start_paused = true)]
async fn a_wrong_pairing_code_is_a_man_in_the_middle() {
    let rig = make_rig(false);
    rig.host.publish_presence().await;
    rig.host.configure(|c| c.wrong_pairing_code = true);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(failure(&state).0, FailureKind::PairingCodeMismatch);
    assert_eq!(state.pairing_code, None);
}

#[tokio::test(start_paused = true)]
async fn answers_that_do_not_verify() {
    // Signed by somebody else.
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    rig.host
        .configure(|c| c.sign_with = Some(Arc::new(IdentityKey::generate())));
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Failed).await;
    let (kind, detail, auto) = failure(&state);
    assert_eq!((kind, auto), (FailureKind::Verification, false));
    assert!(detail.contains("bad_signature"), "{detail}");

    // A host whose key differs from the pinned one.
    let rig = make_rig(false);
    rig.keys
        .pin(PinnedHost {
            host_id: rig.host.host_id.clone(),
            host_key: remote_control_client::encoding::b64u_encode(
                &IdentityKey::generate().public_key(),
            ),
            name: "x".into(),
            paired_at: 1,
        })
        .await;
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert!(
        failure(&state).1.contains("key_mismatch"),
        "{:?}",
        failure(&state)
    );
}

#[tokio::test(start_paused = true)]
async fn an_unknown_host() {
    let rig = make_rig(false);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(failure(&state).0, FailureKind::UnknownHost);
}

#[tokio::test(start_paused = true)]
async fn handshake_errors() {
    use remote_control_client::rpc::RemoteErrorCode;
    for (code, kind, auto) in [
        (
            RemoteErrorCode::UpgradeRequired,
            FailureKind::UpgradeRequired,
            false,
        ),
        (RemoteErrorCode::Capacity, FailureKind::Capacity, true),
    ] {
        let rig = make_rig(false);
        rig.pair_beforehand().await;
        rig.host.configure(|c| c.initialize_error = Some(code));
        rig.conn.start();
        let want = if auto {
            ConnectionPhase::Reconnecting
        } else {
            ConnectionPhase::Failed
        };
        let state = rig.phase(want).await;
        assert_eq!(failure(&state).0, kind);
        rig.conn.stop();
    }
    // A host that never answers `initialize`.
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    rig.host.configure(|c| c.initialize_hangs = true);
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(
        failure(&state),
        (FailureKind::Closed, "handshake_timeout".into(), false)
    );
}

#[tokio::test(start_paused = true)]
async fn network_failures_back_off_and_give_up() {
    let rig = make_rig_with(false, 9);
    rig.pair_beforehand().await;
    rig.conn.start();
    let state = rig.phase(ConnectionPhase::Reconnecting).await;
    assert_eq!(failure(&state), (FailureKind::Network, "500".into(), true));
    assert_eq!(state.attempt, 1);
    assert_eq!(
        state.retry_at,
        Some(rig.clock.now_ms() + 500),
        "first delay is 1 s scaled by 0.5 with no jitter"
    );
    // Delays: 0.5 1 2 4 8 15 15 15 s; the ninth failure ends it.
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(state.attempt, 9);
    assert_eq!(failure(&state).0, FailureKind::Network);
    // Gave up on its own, but a foreground / network-up nudge may start over.
    assert!(failure(&state).2);
    rig.conn.nudge();
    rig.phase(ConnectionPhase::Ready).await;
}

#[tokio::test(start_paused = true)]
async fn a_stable_link_resets_the_failure_count() {
    let rig = make_rig_with(false, 3);
    rig.pair_beforehand().await;
    rig.conn.start();
    rig.phase(ConnectionPhase::Ready).await;
    advance(Duration::from_secs(61)).await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.drop_connection(&conn_id, "restart");
    let state = rig.phase(ConnectionPhase::Reconnecting).await;
    assert_eq!(state.attempt, 1, "the earlier failures no longer count");

    // A link that dies at once keeps counting.
    rig.phase(ConnectionPhase::Ready).await;
    let second = rig.host.conns()[1].conn_id.clone();
    rig.host.drop_connection(&second, "restart");
    let state = rig.phase(ConnectionPhase::Reconnecting).await;
    assert_eq!(state.attempt, 2);
}

#[tokio::test(start_paused = true)]
async fn the_host_ends_an_approved_connection() {
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    rig.conn.start();
    rig.phase(ConnectionPhase::Ready).await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host
        .forget_device(&rig.keys.identity().await.unwrap().id);
    rig.host
        .notify(
            &conn_id,
            methods::AUTH_DENIED,
            json!({ "reason": "revoked" }),
        )
        .await;
    let state = rig.phase(ConnectionPhase::Failed).await;
    assert_eq!(failure(&state).0, FailureKind::Revoked);
    settle().await;
    assert!(rig.keys.pinned_hosts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stop_while_connecting_cleans_up() {
    let rig = make_rig(false);
    rig.pair_beforehand().await;
    rig.host.configure(|c| c.answer = false);
    rig.conn.start();
    advance(Duration::from_secs(2)).await;
    assert_ne!(rig.nodes().0, Value::Null, "the offer is out");
    rig.conn.stop();
    advance(Duration::from_secs(2)).await;
    assert_eq!(rig.conn.state().phase, ConnectionPhase::Idle);
    assert_eq!(rig.nodes(), (Value::Null, Value::Null));
    advance(Duration::from_secs(60)).await;
    assert_eq!(
        rig.conn.state().phase,
        ConnectionPhase::Idle,
        "no late failure after stop"
    );
}

struct DelayedResetKeys {
    inner: Arc<MemoryViewerKeys>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    resets: AtomicU32,
}

#[async_trait::async_trait]
impl ViewerKeys for DelayedResetKeys {
    async fn identity(
        &self,
    ) -> Result<
        remote_control_client::device::DeviceIdentity,
        remote_control_client::device::KeysError,
    > {
        self.inner.identity().await
    }
    async fn reset_identity(&self) -> Result<(), remote_control_client::device::KeysError> {
        self.resets.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.reset_identity().await
    }
    async fn pinned_host(&self, id: &str) -> Option<PinnedHost> {
        self.inner.pinned_host(id).await
    }
    async fn pin(&self, host: PinnedHost) {
        self.inner.pin(host).await;
    }
    async fn unpair(&self, id: &str) {
        self.inner.unpair(id).await;
    }
}

async fn revoked_with_delayed_reset() -> (Rig, Arc<DelayedResetKeys>) {
    let mut rig = make_rig(false);
    let keys = Arc::new(DelayedResetKeys {
        inner: rig.keys.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        resets: AtomicU32::new(0),
    });
    let mut deps = deps_for(
        (&rig.rtdb, &rig.clock, &rig.keys, &rig.memory),
        &rig.host.host_id,
    );
    deps.keys = keys.clone();
    rig.conn = ViewerConnection::new(deps);
    rig.pair_beforehand().await;
    rig.host
        .configure(|c| c.reject = Some(AnswerRejectReason::Revoked));
    rig.conn.start();
    rig.phase(ConnectionPhase::Failed).await;
    (rig, keys)
}

#[tokio::test(start_paused = true)]
async fn a_stopped_connection_does_not_restart_after_a_delayed_identity_repair() {
    let (rig, keys) = revoked_with_delayed_reset().await;
    let connection = rig.conn.clone();
    let repairing = tokio::spawn(async move {
        connection.repair().await;
    });
    keys.entered.notified().await;
    rig.conn.stop();
    keys.release.notify_one();
    repairing.await.unwrap();
    settle().await;
    assert_eq!(rig.conn.state().phase, ConnectionPhase::Idle);
    assert_eq!(keys.resets.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn concurrent_identity_repairs_are_single_flight_and_only_apply_to_revocation() {
    let (rig, keys) = revoked_with_delayed_reset().await;
    let connection = rig.conn.clone();
    let first = tokio::spawn(async move {
        connection.repair().await;
    });
    keys.entered.notified().await;
    let connection = rig.conn.clone();
    let second = tokio::spawn(async move {
        connection.repair().await;
    });
    settle().await;
    assert_eq!(
        keys.resets.load(Ordering::SeqCst),
        1,
        "the device identity must rotate only once"
    );
    rig.conn.stop();
    keys.release.notify_one();
    first.await.unwrap();
    second.await.unwrap();
    rig.conn.repair().await; // Idle is not revocation and must not touch persistent identity.
    assert_eq!(keys.resets.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn stopping_before_the_dial_task_runs_does_not_create_an_offer() {
    let rig = make_rig(false);
    rig.host.publish_presence().await;
    rig.conn.start();
    rig.conn.stop();
    settle().await;
    assert_eq!(rig.conn.state().phase, ConnectionPhase::Idle);
    assert!(
        rig.host.conns().is_empty(),
        "a queued stale dial must not contact the desktop"
    );
    assert_eq!(
        rig.rtdb
            .get(&to_host_path(UID, &rig.host.host_id).unwrap())
            .await
            .unwrap(),
        Value::Null
    );
}

#[tokio::test(start_paused = true)]
async fn approval_before_the_initialize_reply_does_not_mute_live_notifications() {
    let rig = make_rig(false);
    rig.host
        .configure(|config| config.approve_before_initialize_reply = true);
    rig.host.publish_presence().await;
    rig.conn.start();
    rig.phase(ConnectionPhase::Ready).await;
    let conn = rig.host.conns()[0].conn_id.clone();
    rig.host
        .notify(&conn, "test.after-approval", json!({ "ready": true }))
        .await;
    advance(Duration::from_millis(100)).await;
    assert_eq!(
        *rig.recorder.notifications.lock().unwrap(),
        vec![("test.after-approval".into(), json!({ "ready": true }))]
    );
    rig.conn.stop();
}
