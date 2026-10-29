//! Host tests of `remote.rs`: conversions, the delta rule, the callback bridges,
//! and a whole pairing / attach / prompt flow against the viewer crate's fake desktop.

use super::*;
use remote_control_client::clock::ManualClock;
use remote_control_client::rtdb::MemoryRtdb;
use remote_control_client::testing::FakePeerNetwork;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::task::Poll;
use std::time::Duration;

// The fake desktop of the viewer crate's own integration tests (it only uses that
// crate's public API), so the flow below talks to a real tunnel and handshake.
#[path = "../../../remote-control-client/tests/support/host.rs"]
mod fake_host;
use fake_host::FakeHost;

const UID: &str = "user-1";

async fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

// ---------------------------------------------------------------------------
// Doubles
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StoreState {
    identity: Option<Vec<u8>>,
    pins: HashMap<String, RemotePinnedHostFfi>,
    identity_stores: u32,
    identity_deletes: u32,
}

#[derive(Default)]
struct FakeKeyStore {
    state: Arc<Mutex<StoreState>>,
    fail_pin_writes: Arc<AtomicBool>,
}

impl FakeKeyStore {
    fn boxed(&self) -> Box<dyn PlatformRemoteKeyStore> {
        Box::new(Self {
            state: Arc::clone(&self.state),
            fail_pin_writes: Arc::clone(&self.fail_pin_writes),
        })
    }
}

#[async_trait]
impl PlatformRemoteKeyStore for FakeKeyStore {
    async fn load_identity(&self) -> Result<Option<Vec<u8>>, RemoteFfiError> {
        Ok(self.state.lock().unwrap().identity.clone())
    }
    async fn store_identity(&self, pkcs8: Vec<u8>) -> Result<(), RemoteFfiError> {
        let mut state = self.state.lock().unwrap();
        state.identity = Some(pkcs8);
        state.identity_stores += 1;
        Ok(())
    }
    async fn delete_identity(&self) -> Result<(), RemoteFfiError> {
        let mut state = self.state.lock().unwrap();
        state.identity = None;
        state.identity_deletes += 1;
        Ok(())
    }
    async fn load_pinned_host(
        &self,
        host_id: String,
    ) -> Result<Option<RemotePinnedHostFfi>, RemoteFfiError> {
        Ok(self.state.lock().unwrap().pins.get(&host_id).cloned())
    }
    async fn store_pinned_host(&self, host: RemotePinnedHostFfi) -> Result<(), RemoteFfiError> {
        if self.fail_pin_writes.load(Ordering::SeqCst) {
            return Err(RemoteFfiError::KeyStore {
                message: "disk full".into(),
            });
        }
        self.state
            .lock()
            .unwrap()
            .pins
            .insert(host.host_id.clone(), host);
        Ok(())
    }
    async fn delete_pinned_host(&self, host_id: String) -> Result<(), RemoteFfiError> {
        self.state.lock().unwrap().pins.remove(&host_id);
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum Seen {
    State(RemoteConnectionStateFfi),
    Hosts(Vec<RemoteHostFfi>),
    HostsFailed(String),
    Sessions(String, Vec<RemoteSessionSummaryFfi>),
    Session(Box<RemoteSessionUpdateFfi>),
}

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<Seen>>,
}

impl Recorder {
    fn all(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
    fn states(&self) -> Vec<RemoteConnectionStateFfi> {
        self.all()
            .into_iter()
            .filter_map(|s| match s {
                Seen::State(state) => Some(state),
                _ => None,
            })
            .collect()
    }
    fn session_updates(&self) -> Vec<RemoteSessionUpdateFfi> {
        self.all()
            .into_iter()
            .filter_map(|s| match s {
                Seen::Session(update) => Some(*update),
                _ => None,
            })
            .collect()
    }
    fn last_hosts(&self) -> Option<Vec<RemoteHostFfi>> {
        self.all().into_iter().rev().find_map(|s| match s {
            Seen::Hosts(hosts) => Some(hosts),
            _ => None,
        })
    }
}

#[async_trait]
impl PlatformRemoteListener for Recorder {
    async fn on_connection_state(&self, state: RemoteConnectionStateFfi) {
        self.seen.lock().unwrap().push(Seen::State(state));
    }
    async fn on_hosts(&self, hosts: Vec<RemoteHostFfi>) {
        self.seen.lock().unwrap().push(Seen::Hosts(hosts));
    }
    async fn on_hosts_failed(&self, message: String) {
        self.seen.lock().unwrap().push(Seen::HostsFailed(message));
    }
    async fn on_sessions(&self, host_id: String, sessions: Vec<RemoteSessionSummaryFfi>) {
        self.seen
            .lock()
            .unwrap()
            .push(Seen::Sessions(host_id, sessions));
    }
    async fn on_session_update(&self, update: RemoteSessionUpdateFfi) {
        self.seen
            .lock()
            .unwrap()
            .push(Seen::Session(Box::new(update)));
    }
}

/// Lets every ready task run without advancing paused time, then polls.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for i in 0..2000 {
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(if i < 400 { 5 } else { 250 })).await;
    }
    panic!("timed out waiting for {what}");
}

fn record_json(value: Value) -> RemoteRecord {
    serde_json::from_value(value).unwrap()
}

fn message(text: &str) -> MessageDto {
    serde_json::from_value(json!({
        "role": "user",
        "blocks": [{ "type": "text", "text": text }],
    }))
    .or_else(|_| serde_json::from_value(json!({ "role": "user", "blocks": [] })))
    .unwrap()
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

#[test]
fn a_known_event_embeds_the_client_event() {
    let record = record_json(json!({
        "type": "event",
        "event": { "type": "turn_started", "turn_id": 4 },
        "truncated": true,
    }));
    match record_to_ffi(&record) {
        RemoteRecordFfi::Event {
            event: ClientEvent::TurnStarted { turn_id },
            truncated,
        } => {
            assert_eq!(turn_id, Some(4));
            assert!(truncated);
        }
        other => panic!("not a turn_started event: {other:?}"),
    }
}

#[test]
fn unknown_events_and_records_become_unsupported() {
    let event = record_json(json!({ "type": "event", "event": { "type": "hologram", "k": 1 } }));
    assert_eq!(
        record_to_ffi(&event),
        RemoteRecordFfi::Unsupported {
            type_name: "hologram".into()
        }
    );
    let record = record_json(json!({ "type": "teleport" }));
    assert_eq!(
        record_to_ffi(&record),
        RemoteRecordFfi::Unsupported {
            type_name: "teleport".into()
        }
    );
}

#[test]
fn prompts_omitted_and_requests_keep_their_fields() {
    let prompt = record_json(json!({
        "type": "prompt", "turn_id": 3, "text": "hi",
        "images": [{ "ref": "r1", "media_type": "image/png" }],
        "origin": "device", "device_name": "Pixel",
    }));
    assert_eq!(
        record_to_ffi(&prompt),
        RemoteRecordFfi::Prompt {
            turn_id: 3,
            text: "hi".into(),
            images: vec![RemoteImageRefFfi {
                image_ref: "r1".into(),
                media_type: "image/png".into()
            }],
            origin: RemotePromptOriginFfi::Device,
            device_name: Some("Pixel".into()),
        }
    );
    for (wire, origin) in [
        ("desktop", RemotePromptOriginFfi::Desktop),
        ("scheduled", RemotePromptOriginFfi::Scheduled),
    ] {
        let record =
            record_json(json!({ "type": "prompt", "turn_id": 1, "text": "x", "origin": wire }));
        match record_to_ffi(&record) {
            RemoteRecordFfi::Prompt {
                origin: got,
                device_name,
                ..
            } => {
                assert_eq!(got, origin);
                assert_eq!(device_name, None);
            }
            other => panic!("{other:?}"),
        }
    }
    let omitted = record_json(
        json!({ "type": "omitted", "event_type": "tool_use_result", "size_bytes": 2_000_000 }),
    );
    assert_eq!(
        record_to_ffi(&omitted),
        RemoteRecordFfi::Omitted {
            event_type: Some("tool_use_result".into()),
            size_bytes: Some(2_000_000)
        }
    );
    let permission = record_json(json!({
        "type": "permission_request",
        "request": { "request_id": 9, "kind": {
            "type": "tool_use_confirm", "tool_name": "Bash",
            "tool_input_json": "{}", "default_allow": false } },
    }));
    match record_to_ffi(&permission) {
        RemoteRecordFfi::PermissionRequest { request } => assert_eq!(request.request_id, 9),
        other => panic!("{other:?}"),
    }
}

fn connection_state(phase: ConnectionPhase) -> ConnectionState {
    ConnectionState {
        phase,
        host_id: "h1".into(),
        transport: None,
        pairing_code: None,
        failure: None,
        attempt: 0,
        retry_at: None,
        host: None,
        ever_ready: false,
    }
}

#[test]
fn connection_states_map_phase_failure_and_host() {
    for (phase, expected) in [
        (ConnectionPhase::Idle, RemoteConnectionPhaseFfi::Idle),
        (
            ConnectionPhase::Connecting,
            RemoteConnectionPhaseFfi::Connecting,
        ),
        (
            ConnectionPhase::AwaitingApproval,
            RemoteConnectionPhaseFfi::Pairing,
        ),
        (ConnectionPhase::Ready, RemoteConnectionPhaseFfi::Ready),
        (
            ConnectionPhase::Reconnecting,
            RemoteConnectionPhaseFfi::Reconnecting,
        ),
        (ConnectionPhase::Failed, RemoteConnectionPhaseFfi::Failed),
    ] {
        assert_eq!(
            connection_state_to_ffi(&connection_state(phase)).phase,
            expected
        );
    }

    let mut state = connection_state(ConnectionPhase::AwaitingApproval);
    state.transport = Some(TransportKind::Tunnel);
    state.pairing_code = Some("123456".into());
    let ffi = connection_state_to_ffi(&state);
    assert_eq!(ffi.pairing_code.as_deref(), Some("123456"));
    assert_eq!(ffi.transport, Some(RemoteTransportFfi::Tunnel));
    assert_eq!(ffi.host_id, "h1");

    let mut state = connection_state(ConnectionPhase::Failed);
    state.failure = Some(ConnectionFailure {
        kind: FailureKind::Revoked,
        detail: Some("revoked".into()),
        auto: false,
    });
    state.retry_at = Some(5);
    state.attempt = 2;
    state.ever_ready = true;
    state.host = Some(remote_control_client::messages::RemoteHostInfo {
        name: "Mac".into(),
        platform: "macos".into(),
        app_version: "1".into(),
    });
    let ffi = connection_state_to_ffi(&state);
    assert_eq!(
        ffi.failure,
        Some(RemoteFailureFfi {
            kind: RemoteFailureKindFfi::Revoked,
            detail: Some("revoked".into()),
            auto_retry: false
        })
    );
    assert_eq!(
        (ffi.retry_at_ms, ffi.attempt, ffi.ever_ready),
        (Some(5), 2, true)
    );
    assert_eq!(ffi.host.unwrap().name, "Mac");
}

#[test]
fn every_failure_kind_has_a_counterpart() {
    let kinds = [
        (FailureKind::Unsupported, RemoteFailureKindFfi::Unsupported),
        (FailureKind::UnknownHost, RemoteFailureKindFfi::UnknownHost),
        (FailureKind::Network, RemoteFailureKindFfi::Network),
        (FailureKind::NoAnswer, RemoteFailureKindFfi::NoAnswer),
        (FailureKind::Closed, RemoteFailureKindFfi::Closed),
        (FailureKind::Capacity, RemoteFailureKindFfi::Capacity),
        (FailureKind::Busy, RemoteFailureKindFfi::Busy),
        (FailureKind::Revoked, RemoteFailureKindFfi::Revoked),
        (FailureKind::Disabled, RemoteFailureKindFfi::Disabled),
        (
            FailureKind::UpgradeRequired,
            RemoteFailureKindFfi::UpgradeRequired,
        ),
        (FailureKind::Denied, RemoteFailureKindFfi::Denied),
        (FailureKind::Expired, RemoteFailureKindFfi::Expired),
        (FailureKind::Rejected, RemoteFailureKindFfi::Rejected),
        (
            FailureKind::Verification,
            RemoteFailureKindFfi::Verification,
        ),
        (
            FailureKind::PairingCodeMismatch,
            RemoteFailureKindFfi::PairingCodeMismatch,
        ),
        (FailureKind::Protocol, RemoteFailureKindFfi::Protocol),
    ];
    for (kind, expected) in kinds {
        assert_eq!(failure_kind(kind), expected);
    }
}

#[test]
fn session_summaries_map_status_and_pending_action() {
    let summary = RemoteSessionSummary {
        session_id: "s1".into(),
        title: "Fix".into(),
        project_label: "app".into(),
        status: RemoteSessionStatus::RequiresAction,
        pending_action: Some(RemotePendingAction::ComputerAccess),
        model: Some("m".into()),
        viewers: 2,
    };
    let ffi = session_summary_to_ffi(&summary);
    assert_eq!(ffi.status, RemoteSessionStatusFfi::RequiresAction);
    assert_eq!(
        ffi.pending_action,
        Some(RemotePendingActionFfi::ComputerAccess)
    );
    assert_eq!((ffi.viewers, ffi.model.as_deref()), (2, Some("m")));
}

#[test]
fn images_cross_as_bytes_and_stay_within_two_mib() {
    let encoded = prompt_images(vec![RemotePromptImageFfi {
        media_type: "image/png".into(),
        bytes: vec![1, 2, 3, 4],
    }])
    .unwrap();
    assert_eq!(encoded[0].base64, "AQIDBA==");
    let too_big = prompt_images(vec![RemotePromptImageFfi {
        media_type: "image/png".into(),
        bytes: vec![0; MAX_PROMPT_IMAGE_BYTES + 1],
    }]);
    assert!(matches!(
        too_big,
        Err(RemoteFfiError::InvalidArgument { .. })
    ));

    let image = image_from_data_url("data:image/png;base64,AQIDBA==").unwrap();
    assert_eq!(image.media_type, "image/png");
    assert_eq!(image.bytes, vec![1, 2, 3, 4]);
    assert!(image_from_data_url("https://x/y.png").is_err());
    assert!(image_from_data_url("data:image/png;base64,***").is_err());
}

#[test]
fn native_webrtc_connector_is_wired() {
    assert!(webrtc_peer_connector().is_some());
}

// ---------------------------------------------------------------------------
// The delta rule
// ---------------------------------------------------------------------------

fn seq_prompt(seq: u64) -> RemoteSeqRecord {
    RemoteSeqRecord {
        seq,
        record: record_json(
            json!({ "type": "prompt", "turn_id": seq, "text": "p", "origin": "desktop" }),
        ),
    }
}

fn state_with(status: SessionStatus, first: u64, messages: usize, seqs: &[u64]) -> SessionState {
    SessionState {
        session_id: "s1".into(),
        status,
        error: None,
        messages: (0..messages).map(|i| message(&i.to_string())).collect(),
        first_message_index: first,
        records: seqs.iter().map(|s| seq_prompt(*s)).collect(),
        pending: RemotePending::default(),
        turn: RemoteTurnState::default(),
        controls: None,
        outgoing: Vec::new(),
        has_older: first > 0,
        loading_older: false,
        answered_elsewhere: false,
    }
}

#[test]
fn the_first_content_replaces_and_later_records_are_appended() {
    let mut marker = SentMarker::new();
    let loading = session_update(&mut marker, &state_with(SessionStatus::Loading, 0, 0, &[]));
    assert!(!loading.replace && loading.records.is_empty() && loading.messages.is_empty());
    assert_eq!(loading.phase, RemoteSessionPhaseFfi::Loading);

    let first = session_update(&mut marker, &state_with(SessionStatus::Live, 5, 2, &[1, 2]));
    assert!(first.replace);
    assert_eq!((first.messages.len(), first.records.len()), (2, 2));
    assert_eq!(first.first_message_index, 5);

    let next = session_update(
        &mut marker,
        &state_with(SessionStatus::Live, 5, 2, &[1, 2, 3]),
    );
    assert!(!next.replace);
    assert_eq!(
        next.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![3]
    );
    assert!(next.messages.is_empty() && next.older_messages.is_empty());

    // Only meta changed: nothing to add.
    let same = session_update(
        &mut marker,
        &state_with(SessionStatus::Live, 5, 2, &[1, 2, 3]),
    );
    assert!(!same.replace && same.records.is_empty());
}

#[test]
fn an_older_page_is_prepended_and_a_reload_replaces() {
    let mut marker = SentMarker::new();
    session_update(&mut marker, &state_with(SessionStatus::Live, 5, 2, &[1]));

    let older = session_update(&mut marker, &state_with(SessionStatus::Live, 2, 5, &[1]));
    assert!(!older.replace);
    assert_eq!(older.older_messages.len(), 3);
    assert_eq!(older.first_message_index, 2);
    assert!(older.records.is_empty());

    // A connection loss and a new attach: whatever arrives next replaces everything,
    // even when it looks the same.
    session_update(&mut marker, &state_with(SessionStatus::Offline, 2, 5, &[1]));
    session_update(&mut marker, &state_with(SessionStatus::Loading, 2, 5, &[1]));
    let again = session_update(&mut marker, &state_with(SessionStatus::Live, 2, 5, &[1]));
    assert!(again.replace);
    assert_eq!((again.messages.len(), again.records.len()), (5, 1));
}

#[test]
fn a_state_that_does_not_extend_what_was_sent_replaces() {
    let mut marker = SentMarker::new();
    session_update(
        &mut marker,
        &state_with(SessionStatus::Live, 0, 1, &[1, 2, 3]),
    );
    // Records shrank or changed identity.
    let shrunk = session_update(&mut marker, &state_with(SessionStatus::Live, 0, 1, &[1]));
    assert!(shrunk.replace);
    let changed = session_update(&mut marker, &state_with(SessionStatus::Live, 0, 1, &[7]));
    assert!(changed.replace);
    // The base moved forward.
    let moved = session_update(&mut marker, &state_with(SessionStatus::Live, 3, 4, &[7]));
    assert!(moved.replace);
}

// ---------------------------------------------------------------------------
// Callback bridges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_device_key_is_created_once_stored_and_reused() {
    let store = FakeKeyStore::default();
    let bridge = KeyStoreBridge::new(store.boxed());
    let first = bridge.identity().await.unwrap();
    let again = bridge.identity().await.unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(store.state.lock().unwrap().identity_stores, 1);
    let stored = store.state.lock().unwrap().identity.clone().unwrap();
    assert_eq!(stored.len(), 48, "PKCS#8: header and seed");

    // A new process loads the same key.
    let restarted = KeyStoreBridge::new(store.boxed());
    assert_eq!(restarted.identity().await.unwrap().id, first.id);
    assert_eq!(store.state.lock().unwrap().identity_stores, 1);

    // Resetting deletes the key; the next identity is a new one.
    restarted.reset_identity().await.unwrap();
    assert_eq!(store.state.lock().unwrap().identity_deletes, 1);
    assert_ne!(restarted.identity().await.unwrap().id, first.id);
}

#[tokio::test]
async fn a_corrupt_stored_key_is_reported_not_replaced() {
    let store = FakeKeyStore::default();
    store.state.lock().unwrap().identity = Some(vec![1, 2, 3]);
    let bridge = KeyStoreBridge::new(store.boxed());
    let error = bridge.identity().await.unwrap_err();
    assert!(error.0.contains("stored device key"), "{error}");
    assert_eq!(store.state.lock().unwrap().identity, Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn pins_round_trip_and_survive_a_failing_store() {
    let store = FakeKeyStore::default();
    let bridge = KeyStoreBridge::new(store.boxed());
    assert!(bridge.pinned_host("h1").await.is_none());
    bridge
        .pin(PinnedHost {
            host_id: "h1".into(),
            host_key: "key".into(),
            name: "Mac".into(),
            paired_at: 7,
        })
        .await;
    let stored = store.state.lock().unwrap().pins.get("h1").cloned().unwrap();
    assert_eq!((stored.host_key.as_str(), stored.paired_at_ms), ("key", 7));
    // A new process reads it back from the store.
    let restarted = KeyStoreBridge::new(store.boxed());
    assert_eq!(restarted.pinned_host("h1").await.unwrap().name, "Mac");
    restarted.unpair("h1").await;
    assert!(store.state.lock().unwrap().pins.is_empty());
    assert!(restarted.pinned_host("h1").await.is_none());

    // The store refusing a write does not lose the pin for this run.
    store.fail_pin_writes.store(true, Ordering::SeqCst);
    restarted
        .pin(PinnedHost {
            host_id: "h2".into(),
            host_key: "k2".into(),
            name: "Other".into(),
            paired_at: 1,
        })
        .await;
    assert!(store.state.lock().unwrap().pins.is_empty());
    assert_eq!(restarted.pinned_host("h2").await.unwrap().host_key, "k2");
}

struct PausedLoadStore {
    inner: FakeKeyStore,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl PlatformRemoteKeyStore for Arc<PausedLoadStore> {
    async fn load_identity(&self) -> Result<Option<Vec<u8>>, RemoteFfiError> {
        self.inner.load_identity().await
    }
    async fn store_identity(&self, key: Vec<u8>) -> Result<(), RemoteFfiError> {
        self.inner.store_identity(key).await
    }
    async fn delete_identity(&self) -> Result<(), RemoteFfiError> {
        self.inner.delete_identity().await
    }
    async fn load_pinned_host(
        &self,
        id: String,
    ) -> Result<Option<RemotePinnedHostFfi>, RemoteFfiError> {
        let result = self.inner.load_pinned_host(id).await;
        self.entered.notify_one();
        self.release.notified().await;
        result
    }
    async fn store_pinned_host(&self, host: RemotePinnedHostFfi) -> Result<(), RemoteFfiError> {
        self.inner.store_pinned_host(host).await
    }
    async fn delete_pinned_host(&self, id: String) -> Result<(), RemoteFfiError> {
        self.inner.delete_pinned_host(id).await
    }
}

#[tokio::test]
async fn forgotten_pin_cannot_be_recached_by_an_inflight_store_read() {
    let store = Arc::new(PausedLoadStore {
        inner: FakeKeyStore::default(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    store
        .inner
        .store_pinned_host(RemotePinnedHostFfi {
            host_id: "a".into(),
            host_key: "key".into(),
            name: "Mac".into(),
            paired_at_ms: 1,
        })
        .await
        .unwrap();
    let bridge = Arc::new(KeyStoreBridge::new(Box::new(store.clone())));
    let reading = bridge.clone();
    let read = tokio::spawn(async move { reading.pinned_host("a").await });
    store.entered.notified().await;
    let forget = bridge.unpair("a");
    tokio::pin!(forget);
    assert_pending(forget.as_mut()).await;
    store.release.notify_one();
    assert!(read.await.unwrap().is_some());
    forget.await;
    assert!(bridge.coordinator.pins.lock().unwrap().get("a").is_none());
    assert!(store.inner.state.lock().unwrap().pins.get("a").is_none());
}

#[tokio::test]
async fn shared_store_bridges_observe_identity_reset_and_pin_replacement() {
    let store = FakeKeyStore::default();
    let coordinator = Arc::new(AccountCoordinator::default());
    let first = KeyStoreBridge::with_coordinator(store.boxed(), coordinator.clone());
    let second = KeyStoreBridge::with_coordinator(store.boxed(), coordinator);
    let old_identity = first.identity().await.unwrap().id;
    assert_eq!(second.identity().await.unwrap().id, old_identity);
    second.reset_identity().await.unwrap();
    assert_ne!(first.identity().await.unwrap().id, old_identity);
    first.pin(pin_for("a")).await;
    let mut newer = pin_for("a");
    newer.host_key = "new key".into();
    second.pin(newer).await;
    assert_eq!(first.pinned_host("a").await.unwrap().host_key, "new key");
}

#[test]
fn production_coordinator_matches_native_storage_domain() {
    let first = account_coordinator("account-a");
    let same = account_coordinator("account-a");
    let other = account_coordinator("account-b");
    assert!(Arc::ptr_eq(&first, &same));
    assert_eq!(
        Arc::ptr_eq(&first, &other),
        matches!(REMOTE_DEVICE_PLATFORM, DevicePlatform::Android)
    );
}

struct Tokens {
    calls: Arc<Mutex<Vec<bool>>>,
    fail: bool,
}

#[async_trait]
impl PlatformRemoteTokenProvider for Tokens {
    async fn id_token(&self, force_refresh: bool) -> Result<String, RemoteFfiError> {
        self.calls.lock().unwrap().push(force_refresh);
        if self.fail {
            Err(RemoteFfiError::Token {
                message: "signed out".into(),
            })
        } else {
            Ok("token-1".into())
        }
    }
}

#[tokio::test]
async fn the_token_provider_is_asked_with_the_refresh_flag() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let bridge = TokenBridge {
        inner: Box::new(Tokens {
            calls: Arc::clone(&calls),
            fail: false,
        }),
    };
    assert_eq!(bridge.id_token(false).await.unwrap(), "token-1");
    assert_eq!(bridge.id_token(true).await.unwrap(), "token-1");
    assert_eq!(*calls.lock().unwrap(), vec![false, true]);

    let failing = TokenBridge {
        inner: Box::new(Tokens { calls, fail: true }),
    };
    let error = failing.id_token(false).await.unwrap_err();
    assert!(error.0.contains("signed out"), "{error}");
}

// ---------------------------------------------------------------------------
// A whole flow
// ---------------------------------------------------------------------------

struct Rig {
    client: RemoteControlCore,
    recorder: Arc<Recorder>,
    store: FakeKeyStore,
    host: Arc<FakeHost>,
}

fn rig() -> Rig {
    let clock = ManualClock::new(1_800_000_000_000);
    let rtdb = Arc::new(MemoryRtdb::with_clock(Arc::clone(&clock) as _));
    let host = FakeHost::start(
        Arc::clone(&rtdb) as _,
        Arc::clone(&clock) as _,
        FakePeerNetwork::new(),
        UID,
        IdentityKey::generate(),
    );
    host.configure(|config| {
        config.respond = Some(Arc::new(|method: &str, _params: &Value| match method {
            "sessions.watch" => Some(Ok(json!({ "sessions": [{
                "session_id": "s1", "title": "Fix bug", "project_label": "app",
                "status": "running", "viewers": 0,
            }] }))),
            "session.attach" => Some(Ok(json!({ "mode": "snapshot", "epoch": "e1" }))),
            "prompt.send" => Some(Ok(json!({ "turn_id": 7 }))),
            "models.list" => Some(Ok(json!({ "models": ["a", "b"], "current": "a" }))),
            _ => None,
        }));
    });
    let store = FakeKeyStore::default();
    let recorder = Arc::new(Recorder::default());
    let coordinator = Arc::new(AccountCoordinator::default());
    let parts = Parts {
        uid: UID.into(),
        rtdb: rtdb as _,
        keys: Arc::new(KeyStoreBridge::with_coordinator(
            store.boxed(),
            coordinator.clone(),
        )),
        coordinator,
        clock: clock as _,
        random: Arc::new(|| 0.0),
        // This fixture exercises the encrypted fallback; native WebRTC has its own adapter tests.
        peers: None,
        media_offer_handler: None,
        device_name: "Pixel".into(),
        app_version: "1.0".into(),
    };
    let client = RemoteControlCore::from_parts(parts, Arc::clone(&recorder) as _);
    Rig {
        client,
        recorder,
        store,
        host,
    }
}

fn requests_of(host: &FakeHost, method: &str) -> Vec<Value> {
    host.requests()
        .into_iter()
        .filter(|r| r.1 == method)
        .map(|r| r.2)
        .collect()
}

#[tokio::test(start_paused = true)]
async fn local_apps_wire_round_trip_and_rejects_invalid_launch() {
    const ID: &str = "a00000000000000000000000000000000";
    let rig = rig();
    assert!(matches!(
        rig.client.list_apps().await,
        Err(RemoteFfiError::NotConnected)
    ));
    rig.host.configure(|config| {
        config.respond = Some(Arc::new(|method, _| match method {
            "apps.list" => Some(Ok(json!({"apps": [{
                "app_id": ID, "name": "Shared", "summary": "Desktop app", "revision": 3,
                "phase": "running", "background": true, "execution_host": "desktop", "remote_enabled": true
            }]}))),
            "apps.open" => Some(Ok(json!({
                "app_id": ID, "revision": 3,
                "url": "https://a00000000000000000000000000000000-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                "expires_at": 1800000001000u64
            }))),
            _ => None,
        }));
    });
    rig.host.publish_presence().await;
    rig.client.connect(rig.host.host_id.clone()).await.unwrap();
    until("pairing", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    rig.host.approve(&rig.host.conns()[0].conn_id).await;
    until("ready", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;

    let apps = rig.client.list_apps().await.unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(
        (
            &apps[0].app_id,
            &apps[0].name,
            &apps[0].summary,
            apps[0].revision,
            &apps[0].phase,
            apps[0].background,
            &apps[0].execution_host,
            apps[0].remote_enabled
        ),
        (
            &ID.to_owned(),
            &"Shared".to_owned(),
            &"Desktop app".to_owned(),
            3,
            &"running".to_owned(),
            true,
            &"desktop".to_owned(),
            true
        )
    );
    let ticket = rig.client.open_app(ID.into()).await.unwrap();
    assert_eq!(
        (ticket.app_id.as_str(), ticket.revision, ticket.expires_at),
        (ID, 3, 1800000001000)
    );
    assert!(ticket.url.starts_with("https://"));
    assert_eq!(
        requests_of(&rig.host, "apps.open"),
        vec![json!({"app_id": ID})]
    );

    rig.host.configure(|config| config.respond = Some(Arc::new(|method, _| match method {
        "apps.open" => Some(Ok(json!({"app_id": ID, "revision": 3, "url": "http://example.com/#bad", "expires_at": 1800000001000u64}))),
        _ => None,
    })));
    assert!(matches!(
        rig.client.open_app(ID.into()).await,
        Err(RemoteFfiError::Rpc { .. })
    ));
    rig.client.disconnect().await;
    assert!(matches!(
        rig.client.open_app(ID.into()).await,
        Err(RemoteFfiError::NotConnected)
    ));
}

#[tokio::test(start_paused = true)]
async fn hosts_are_listed_and_watched_with_their_online_state() {
    let rig = rig();
    assert!(rig.client.list_hosts().await.unwrap().is_empty());

    rig.host.publish_presence().await;
    let listed = rig.client.list_hosts().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].host_id, rig.host.host_id);
    assert_eq!(listed[0].name, "Studio Mac");
    assert!(listed[0].online);
    assert!(!listed[0].paired);

    rig.client.watch_hosts().await;
    until("the host list", || rig.recorder.last_hosts().is_some()).await;
    let first = rig.recorder.last_hosts().unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].online);

    rig.client.stop_watching_hosts().await;
    assert!(rig
        .recorder
        .all()
        .iter()
        .all(|s| !matches!(s, Seen::HostsFailed(_))));
}

#[tokio::test(start_paused = true)]
async fn host_watch_waits_for_the_initial_snapshot_and_recovers_after_timeout() {
    use remote_control_client::device::MemoryViewerKeys;
    use remote_control_client::rtdb::{
        ListenControl, Listener, RtdbError, RtdbEvent, RtdbEventKind,
    };

    struct PendingDatabase(Mutex<Option<mpsc::UnboundedReceiver<ListenItem>>>);
    struct Control;
    impl ListenControl for Control {
        fn close(&self) {}
        fn reconnect(&self) {}
    }
    #[async_trait]
    impl Rtdb for PendingDatabase {
        async fn get(&self, _: &str) -> Result<Value, RtdbError> {
            unreachable!()
        }
        async fn put(&self, _: &str, _: &Value) -> Result<(), RtdbError> {
            unreachable!()
        }
        async fn patch(
            &self,
            _: &str,
            _: &serde_json::Map<String, Value>,
        ) -> Result<(), RtdbError> {
            unreachable!()
        }
        async fn remove(&self, _: &str) -> Result<(), RtdbError> {
            unreachable!()
        }
        fn listen(&self, _: &str) -> Listener {
            Listener::new(self.0.lock().unwrap().take().unwrap(), Arc::new(Control))
        }
    }
    let (input, input_rx) = mpsc::unbounded_channel();
    let (out, mut received) = mpsc::unbounded_channel();
    let task = tokio::spawn(watch_hosts_task(
        UID.into(),
        Arc::new(PendingDatabase(Mutex::new(Some(input_rx)))),
        MemoryViewerKeys::new(),
        ManualClock::new(1_800_000_000_000),
        out,
    ));
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
    assert!(
        matches!(received.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "no empty list before the server responds"
    );
    input
        .send(ListenItem::Event(RtdbEvent {
            kind: RtdbEventKind::Patch,
            path: "/partial".into(),
            data: json!({}),
        }))
        .unwrap();
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        received.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    tokio::time::advance(Duration::from_secs(20)).await;
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
    assert!(
        matches!(received.try_recv(), Ok(Out::HostsFailed(_))),
        "a delayed first snapshot should offer a retry"
    );
    input
        .send(ListenItem::Event(RtdbEvent {
            kind: RtdbEventKind::Put,
            path: "/".into(),
            data: json!({}),
        }))
        .unwrap();
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
    assert!(
        matches!(received.try_recv(), Ok(Out::Hosts(hosts)) if hosts.is_empty()),
        "a later real snapshot clears the error"
    );
    drop(input);
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(received.try_recv(), Ok(Out::HostsFailed(_))));
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn active_client_can_be_freed_on_a_foreign_thread() {
    let rig = rig();
    rig.host.publish_presence().await;
    rig.client.connect(rig.host.host_id.clone()).await.unwrap();
    until("pairing", || !rig.host.conns().is_empty()).await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.approve(&conn_id).await;
    until("ready", || {
        rig.client
            .connection_state()
            .is_some_and(|state| state.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    rig.client.attach_session("s1".into()).await.unwrap();
    until("attach", || {
        !requests_of(&rig.host, "session.attach").is_empty()
    })
    .await;

    // Mirrors Swift deinit / JVM finalization: no Tokio runtime on this thread.
    let client = rig.client;
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(client);
    })
    .join()
    .expect("foreign finalization must not panic");
}

#[tokio::test(start_paused = true)]
async fn pairing_attach_prompt_and_commands_over_the_tunnel() {
    let rig = rig();
    let client = &rig.client;
    rig.host.publish_presence().await;
    let host_id = rig.host.host_id.clone();

    // Nothing is connected yet.
    assert_eq!(
        client.attach_session("s1".into()).await,
        Err(RemoteFfiError::NotConnected)
    );
    assert!(client.connection_state().is_none());

    client.connect(host_id.clone()).await.unwrap();
    until("the pairing code", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    let pairing = rig
        .recorder
        .states()
        .into_iter()
        .find(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
        .unwrap();
    assert_eq!(pairing.host_id, host_id);
    assert_eq!(pairing.pairing_code.as_deref().map(str::len), Some(6));
    assert!(
        rig.store.state.lock().unwrap().pins.is_empty(),
        "not pinned before approval"
    );

    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.approve(&conn_id).await;
    until("ready", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    let ready = rig
        .recorder
        .states()
        .into_iter()
        .find(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
        .unwrap();
    assert_eq!(
        ready.transport,
        Some(RemoteTransportFfi::Tunnel),
        "this fixture forces the encrypted fallback"
    );
    assert_eq!(
        ready.host.as_ref().map(|h| h.name.as_str()),
        Some("Studio Mac")
    );
    assert_eq!(rig.store.state.lock().unwrap().pins.len(), 1);
    assert_eq!(rig.store.state.lock().unwrap().identity_stores, 1);
    assert_eq!(
        client.connection_state().map(|s| s.phase),
        Some(RemoteConnectionPhaseFfi::Ready)
    );
    // The device identity the offer carried is the one the key store persisted.
    let pinned = rig
        .store
        .state
        .lock()
        .unwrap()
        .pins
        .values()
        .next()
        .cloned()
        .unwrap();
    assert_eq!(pinned.host_key, rig.host.host_key_b64u());

    // The session list arrives by itself once ready.
    until("the session list", || {
        rig.recorder
            .all()
            .iter()
            .any(|s| matches!(s, Seen::Sessions(h, list) if *h == host_id && list.len() == 1))
    })
    .await;
    let sessions = client.sessions().unwrap();
    assert_eq!(sessions[0].title, "Fix bug");
    assert_eq!(sessions[0].status, RemoteSessionStatusFfi::Running);

    // Attach: the first update is the empty loading state, then the snapshot replaces.
    client.attach_session("s1".into()).await.unwrap();
    until("the attach", || {
        !requests_of(&rig.host, "session.attach").is_empty()
    })
    .await;
    assert_eq!(
        requests_of(&rig.host, "session.attach")[0],
        json!({ "session_id": "s1" })
    );
    for part in [
        json!({ "phase": "begin", "session_id": "s1", "epoch": "e1", "first_message_index": 0 }),
        json!({ "phase": "messages", "session_id": "s1", "messages": [{ "role": "user", "blocks": [] }] }),
        json!({ "phase": "records", "session_id": "s1", "records": [
            { "seq": 1, "record": { "type": "prompt", "turn_id": 1, "text": "hi",
                "images": [{ "ref": "r1", "media_type": "image/png" }],
                "origin": "device", "device_name": "Pixel" } },
            { "seq": 2, "record": { "type": "event", "event": { "type": "hologram" } } },
        ] }),
        json!({ "phase": "end", "session_id": "s1", "cursor": { "epoch": "e1", "seq": 2 },
               "pending": {}, "turn": { "running": false } }),
    ] {
        rig.host.notify(&conn_id, "session.snapshot", part).await;
    }
    until("the live snapshot", || {
        rig.recorder
            .session_updates()
            .iter()
            .any(|u| u.phase == RemoteSessionPhaseFfi::Live && u.replace)
    })
    .await;
    let snapshot = rig
        .recorder
        .session_updates()
        .into_iter()
        .find(|u| u.replace)
        .unwrap();
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.records.len(), 2);
    assert!(matches!(
        &snapshot.records[0].record,
        RemoteRecordFfi::Prompt { origin: RemotePromptOriginFfi::Device, images, .. } if images.len() == 1
    ));
    assert_eq!(
        snapshot.records[1].record,
        RemoteRecordFfi::Unsupported {
            type_name: "hologram".into()
        }
    );

    // A prompt with an image reaches the desktop as base64 and shows as outgoing.
    client
        .send_prompt(
            "s1".into(),
            "look".into(),
            vec![RemotePromptImageFfi {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3, 4],
            }],
        )
        .await
        .unwrap();
    until("the prompt", || {
        !requests_of(&rig.host, "prompt.send").is_empty()
    })
    .await;
    let sent = &requests_of(&rig.host, "prompt.send")[0];
    assert_eq!(sent["session_id"], "s1");
    assert_eq!(sent["text"], "look");
    assert_eq!(
        sent["images"][0],
        json!({ "media_type": "image/png", "base64": "AQIDBA==" })
    );
    until("the outgoing prompt", || {
        rig.recorder
            .session_updates()
            .iter()
            .any(|u| u.outgoing.iter().any(|p| p.turn_id == Some(7)))
    })
    .await;
    let outgoing = rig
        .recorder
        .session_updates()
        .into_iter()
        .rev()
        .find_map(|u| u.outgoing.into_iter().next())
        .unwrap();
    assert_eq!(outgoing.status, RemoteOutgoingStatusFfi::Sending);
    assert_eq!(outgoing.text, "look");

    // A live record is a one-record delta carrying the UniFFI ClientEvent.
    rig.host
        .notify(
            &conn_id,
            "session.event",
            json!({ "session_id": "s1", "seq": 3,
                "record": { "type": "event", "event": { "type": "turn_started", "turn_id": 4 } } }),
        )
        .await;
    until("the live record", || {
        rig.recorder
            .session_updates()
            .iter()
            .any(|u| !u.replace && u.records.iter().any(|r| r.seq == 3))
    })
    .await;
    let live = rig
        .recorder
        .session_updates()
        .into_iter()
        .find(|u| u.records.iter().any(|r| r.seq == 3))
        .unwrap();
    assert_eq!(live.records.len(), 1);
    assert!(matches!(
        live.records[0].record,
        RemoteRecordFfi::Event {
            event: ClientEvent::TurnStarted { .. },
            ..
        }
    ));
    assert!(live.turn.running);

    // While the turn runs the desktop takes plain text only: images are left out.
    client
        .send_prompt(
            "s1".into(),
            "more".into(),
            vec![RemotePromptImageFfi {
                media_type: "image/png".into(),
                bytes: vec![9],
            }],
        )
        .await
        .unwrap();
    let second = requests_of(&rig.host, "prompt.send")[1].clone();
    assert_eq!(second["text"], "more");
    assert!(second.get("images").is_none());

    // The other commands.
    client.cancel_turn("s1".into()).await.unwrap();
    client
        .respond_permission("s1".into(), 9, true)
        .await
        .unwrap();
    client
        .answer_question(
            "s1".into(),
            10,
            HashMap::from([("Which?".to_owned(), "A".to_owned())]),
        )
        .await
        .unwrap();
    client.cancel_question("s1".into(), 11).await.unwrap();
    client.set_model("s1".into(), "b".into()).await.unwrap();
    client
        .set_reasoning("s1".into(), ReasoningSelectionDto::Disabled)
        .await
        .unwrap();
    client
        .set_permission_mode("s1".into(), RemotePermissionModeFfi::Plan)
        .await
        .unwrap();
    client
        .set_session_foreground("s1".into(), false)
        .await
        .unwrap();
    assert_eq!(requests_of(&rig.host, "turn.cancel")[0]["turn_id"], 4);
    assert_eq!(
        requests_of(&rig.host, "permission.respond")[0]["request_id"],
        9
    );
    assert_eq!(
        requests_of(&rig.host, "question.answer")[0]["answers"],
        json!({ "Which?": "A" })
    );
    assert_eq!(
        requests_of(&rig.host, "question.cancel")[0]["request_id"],
        11
    );
    assert_eq!(
        requests_of(&rig.host, "controls.set_model")[0]["model"],
        "b"
    );
    assert_eq!(
        requests_of(&rig.host, "controls.set_reasoning")[0]["selection"],
        json!({ "type": "disabled" })
    );
    assert_eq!(
        requests_of(&rig.host, "controls.set_permission_mode")[0]["mode"],
        "plan"
    );
    let models = client.list_models().await.unwrap();
    assert_eq!((models.models.len(), models.current.as_str()), (2, "a"));

    // Detaching tells the desktop; the position is kept.
    client.detach_session("s1".into()).await.unwrap();
    until("the detach", || {
        !requests_of(&rig.host, "session.detach").is_empty()
    })
    .await;

    // Disconnecting ends it and says so.
    client.disconnect().await;
    until("idle", || {
        rig.recorder
            .states()
            .last()
            .is_some_and(|s| s.phase == RemoteConnectionPhaseFfi::Idle)
    })
    .await;
    assert!(client.connection_state().is_none());
    assert_eq!(
        client.cancel_turn("s1".into()).await,
        Err(RemoteFfiError::NotConnected)
    );
}

#[tokio::test(start_paused = true)]
async fn an_approved_device_reconnects_without_pairing_and_forgetting_unpairs() {
    let rig = rig();
    rig.host.publish_presence().await;
    let host_id = rig.host.host_id.clone();

    // Pair once.
    rig.client.connect(host_id.clone()).await.unwrap();
    until("pairing", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.approve(&conn_id).await;
    until("ready", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    rig.client.disconnect().await;

    // Dialling again skips pairing.
    let before = rig.recorder.states().len();
    rig.client.connect(host_id.clone()).await.unwrap();
    until("ready again", || {
        rig.recorder.states()[before..]
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    assert!(rig.recorder.states()[before..]
        .iter()
        .all(|s| s.phase != RemoteConnectionPhaseFfi::Pairing));
    assert!(rig.client.list_hosts().await.unwrap()[0].paired);

    // Forgetting disconnects and drops the pin.
    rig.client.forget_host(host_id.clone()).await;
    assert!(rig.client.connection_state().is_none());
    assert!(rig.store.state.lock().unwrap().pins.is_empty());
    assert!(!rig.client.list_hosts().await.unwrap()[0].paired);

    // Repairing makes a new device key and dials again.
    let stores = rig.store.state.lock().unwrap().identity_stores;
    rig.client.repair_host(host_id).await.unwrap();
    assert_eq!(rig.store.state.lock().unwrap().identity_deletes, 1);
    until("a new device key", || {
        rig.store.state.lock().unwrap().identity_stores > stores
    })
    .await;
    assert!(rig.client.connection_state().is_some());
}

#[tokio::test(start_paused = true)]
async fn invalid_arguments_are_refused_before_anything_is_sent() {
    let rig = rig();
    assert!(matches!(
        rig.client.connect(String::new()).await,
        Err(RemoteFfiError::InvalidArgument { .. })
    ));
    assert!(matches!(
        rig.client.repair_host(String::new()).await,
        Err(RemoteFfiError::InvalidArgument { .. })
    ));
    assert_eq!(rig.store.state.lock().unwrap().identity_deletes, 0);
    assert_eq!(
        rig.client.list_models().await,
        Err(RemoteFfiError::NotConnected)
    );
    assert!(rig.client.sessions().is_none());
}

#[tokio::test]
async fn delivery_drops_previous_connections_and_detached_session_callbacks() {
    let (out, queue) = mpsc::unbounded_channel();
    let current = Arc::new(AtomicU64::new(2));
    let recorder = Arc::new(Recorder::default());
    let update = || {
        Box::new(session_update(
            &mut SentMarker::new(),
            &state_with(SessionStatus::Live, 0, 0, &[]),
        ))
    };
    out.send(Out::Sessions(1, "old-host".into(), vec![]))
        .unwrap();
    out.send(Out::Session(1, Arc::new(AtomicBool::new(true)), update()))
        .unwrap();
    let detached = Arc::new(AtomicBool::new(true));
    out.send(Out::Session(2, Arc::clone(&detached), update()))
        .unwrap();
    detached.store(false, Ordering::SeqCst);
    out.send(Out::Session(2, Arc::new(AtomicBool::new(true)), update()))
        .unwrap();
    drop(out);
    deliver(queue, recorder.clone(), current).await;
    let seen = recorder.all();
    assert_eq!(seen.len(), 1);
    assert!(matches!(seen[0], Seen::Session(_)));
}

// Block connection construction at its push-id read. On the broken code the
// generation is already advanced, but active has not yet been installed; a
// disconnect can finish and the old constructor then resurrects the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_cannot_be_undone_by_an_inflight_connection_install() {
    let rig = rig();
    let client = Arc::new(rig.client);
    let gated = Arc::clone(&client);
    let (ready, waiting) = std::sync::mpsc::channel();
    let gate = std::thread::spawn(move || {
        let push_id = gated.core.push_installation_id.lock().unwrap();
        ready.send(()).unwrap();
        while gated.core.current.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        // The atomic implementation holds active for construction: let that
        // transaction finish so disconnect can follow. The old one leaves it
        // unlocked, allowing disconnect to advance the generation first.
        while gated.core.current.load(Ordering::SeqCst) == 1 {
            match gated.core.active.try_lock() {
                Ok(active) => drop(active),
                Err(std::sync::TryLockError::WouldBlock) => break,
                Err(error) => panic!("active mutex poisoned: {error}"),
            }
            std::thread::yield_now();
        }
        drop(push_id);
    });
    waiting.recv().unwrap();
    let starting = Arc::clone(&client);
    let runtime = tokio::runtime::Handle::current();
    let connect = std::thread::spawn(move || {
        let _runtime = runtime.enter();
        starting.start_connection("second-host").unwrap();
    });
    while client.core.current.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    client.disconnect().await;
    connect.join().unwrap();
    gate.join().unwrap();
    assert!(
        client.connection_state().is_none(),
        "disconnect must end an already-started connection transaction"
    );
}

struct PausedResetKeys {
    inner: Arc<dyn ViewerKeys>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct PausedHostKeys {
    inner: Arc<dyn ViewerKeys>,
    pause_pin: bool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct PausedDeleteKeys {
    inner: Arc<dyn ViewerKeys>,
    pause: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl ViewerKeys for PausedDeleteKeys {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        self.inner.identity().await
    }
    async fn reset_identity(&self) -> Result<(), KeysError> {
        self.inner.reset_identity().await
    }
    async fn pinned_host(&self, id: &str) -> Option<PinnedHost> {
        self.inner.pinned_host(id).await
    }
    async fn pin(&self, host: PinnedHost) {
        self.inner.pin(host).await;
    }
    async fn unpair(&self, id: &str) {
        if self.pause.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.unpair(id).await;
    }
}

fn paused_delete_keys(rig: &Rig) -> Arc<PausedDeleteKeys> {
    Arc::new(PausedDeleteKeys {
        inner: rig.client.core.parts.keys.clone(),
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    })
}

#[async_trait]
impl ViewerKeys for PausedHostKeys {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        self.inner.identity().await
    }
    async fn reset_identity(&self) -> Result<(), KeysError> {
        self.inner.reset_identity().await
    }
    async fn pinned_host(&self, id: &str) -> Option<PinnedHost> {
        self.inner.pinned_host(id).await
    }
    async fn pin(&self, host: PinnedHost) {
        if self.pause_pin {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.pin(host).await;
    }
    async fn unpair(&self, id: &str) {
        if !self.pause_pin {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.unpair(id).await;
    }
}

fn client_with_keys(rig: &Rig, keys: Arc<dyn ViewerKeys>) -> Arc<RemoteControlCore> {
    let parts = &rig.client.core.parts;
    Arc::new(RemoteControlCore::from_parts(
        Parts {
            uid: parts.uid.clone(),
            rtdb: parts.rtdb.clone(),
            keys,
            coordinator: parts.coordinator.clone(),
            clock: parts.clock.clone(),
            random: parts.random.clone(),
            peers: parts.peers.clone(),
            media_offer_handler: parts.media_offer_handler.clone(),
            device_name: parts.device_name.clone(),
            app_version: parts.app_version.clone(),
        },
        rig.recorder.clone(),
    ))
}

fn pin_for(host: &str) -> PinnedHost {
    PinnedHost {
        host_id: host.into(),
        host_key: "key".into(),
        name: "Mac".into(),
        paired_at: 1,
    }
}

fn connection_keys(client: &RemoteControlCore, host: &str) -> Arc<ConnectionKeys> {
    let active = client.core.active.lock().unwrap();
    let active = active.as_ref().unwrap();
    assert_eq!(active.host_id, host);
    let gate = client.core.parts.coordinator.host_gates.get(host);
    Arc::new(ConnectionKeys {
        inner: client.core.parts.keys.clone(),
        host_id: host.into(),
        trust_epoch: AtomicU64::new(gate.trust_epoch.load(Ordering::SeqCst)),
        gate,
        coordinator: client.core.parts.coordinator.clone(),
        root_id: active.root.id.clone(),
    })
}

#[tokio::test]
async fn ordinary_switch_preserves_queued_approval_and_revocation_writes() {
    let rig = rig();
    let client = &rig.client;
    client.connect("a".into()).await.unwrap();
    let keys = connection_keys(client, "a");
    let gate = keys.gate.clone();
    let held = gate.lock.lock().await;
    let pin = keys.pin(pin_for("a"));
    tokio::pin!(pin);
    assert_pending(pin.as_mut()).await;
    client.connect("b".into()).await.unwrap();
    drop(held);
    pin.await;
    assert!(client.core.parts.keys.pinned_host("a").await.is_some());

    client.connect("a".into()).await.unwrap();
    let keys = connection_keys(client, "a");
    let gate = keys.gate.clone();
    let held = gate.lock.lock().await;
    let unpair = keys.unpair("a");
    tokio::pin!(unpair);
    assert_pending(unpair.as_mut()).await;
    client.disconnect().await;
    drop(held);
    unpair.await;
    assert!(client.core.parts.keys.pinned_host("a").await.is_none());
}

#[tokio::test]
async fn repair_rejects_old_pairing_writes_after_new_pairing() {
    let rig = rig();
    let client = &rig.client;
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(client, "a");
    client.disconnect().await;
    client.repair_host("a".into()).await.unwrap();
    let new_keys = connection_keys(client, "a");
    let mut new_pin = pin_for("a");
    new_pin.host_key = "new key".into();
    new_keys.pin(new_pin).await;
    old_keys.unpair("a").await;
    old_keys.pin(pin_for("a")).await;
    assert_eq!(
        client
            .core
            .parts
            .keys
            .pinned_host("a")
            .await
            .unwrap()
            .host_key,
        "new key"
    );
}

#[tokio::test]
async fn repair_on_an_active_non_revoked_host_is_a_noop() {
    let rig = rig();
    let client = &rig.client;
    client.connect("a".into()).await.unwrap();
    let keys = connection_keys(client, "a");
    let generation = client.core.current.load(Ordering::SeqCst);
    client.repair_host("a".into()).await.unwrap();
    assert_eq!(client.core.current.load(Ordering::SeqCst), generation);
    assert_eq!(
        keys.gate.trust_epoch.load(Ordering::SeqCst),
        keys.trust_epoch.load(Ordering::SeqCst)
    );
    assert_eq!(rig.store.state.lock().unwrap().identity_deletes, 0);
}

#[tokio::test(start_paused = true)]
async fn repair_keeps_existing_session_watchers() {
    use remote_control_client::signaling::AnswerRejectReason;
    let rig = rig();
    rig.host
        .configure(|config| config.reject = Some(AnswerRejectReason::Revoked));
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    let other = client_with_keys(&rig, rig.client.core.parts.keys.clone());
    let unrelated = client_with_keys(&rig, rig.client.core.parts.keys.clone());
    rig.client.connect(host.clone()).await.unwrap();
    rig.client.attach_session("s1".into()).await.unwrap();
    other.connect(host.clone()).await.unwrap();
    unrelated.connect("different-host".into()).await.unwrap();
    until("revoked", || {
        rig.client.connection_state().is_some_and(|state| {
            state.phase == RemoteConnectionPhaseFfi::Failed
                && state
                    .failure
                    .is_some_and(|failure| failure.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    until("other revoked", || {
        other.connection_state().is_some_and(|state| {
            state.phase == RemoteConnectionPhaseFfi::Failed
                && state
                    .failure
                    .is_some_and(|failure| failure.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    rig.client.repair_host(host).await.unwrap();
    let active = rig.client.core.active.lock().unwrap();
    assert!(active.as_ref().unwrap().watchers.contains_key("s1"));
    drop(active);
    assert!(other.connection_state().is_none());
    assert_eq!(
        unrelated.connection_state().unwrap().host_id,
        "different-host"
    );
}

#[tokio::test]
async fn attempt_scope_remains_old_after_in_place_successful_repair() {
    let rig = rig();
    let client = &rig.client;
    client.connect("a".into()).await.unwrap();
    let root = connection_keys(client, "a");
    let old = root.for_attempt().unwrap();
    root.reset_identity().await.unwrap();
    let fresh = root.for_attempt().unwrap();
    let mut pin = pin_for("a");
    pin.host_key = "new key".into();
    fresh.pin(pin).await;
    old.unpair("a").await;
    old.pin(pin_for("a")).await;
    assert_eq!(
        client
            .core
            .parts
            .keys
            .pinned_host("a")
            .await
            .unwrap()
            .host_key,
        "new key"
    );
}

#[tokio::test]
async fn forget_serializes_same_host_and_discards_old_epoch_writes() {
    let rig = rig();
    let keys = Arc::new(PausedHostKeys {
        inner: rig.client.core.parts.keys.clone(),
        pause_pin: false,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = client_with_keys(&rig, keys.clone());
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&client, "a");
    let forgetting = client.clone();
    let forget = tokio::spawn(async move { forgetting.forget_host("a".into()).await });
    keys.entered.notified().await;

    // Another host is independent even while A's persistent deletion is paused.
    client.connect("b".into()).await.unwrap();
    assert_eq!(client.connection_state().unwrap().host_id, "b");
    let late_pin = old_keys.pin(pin_for("a"));
    tokio::pin!(late_pin);
    assert_pending(late_pin.as_mut()).await;
    let reconnect = client.connect("a".into());
    tokio::pin!(reconnect);
    assert_pending(reconnect.as_mut()).await;
    assert_eq!(client.connection_state().unwrap().host_id, "b");
    keys.release.notify_one();
    forget.await.unwrap();
    late_pin.await;
    reconnect.await.unwrap();
    assert!(keys.inner.pinned_host("a").await.is_none());
    assert_eq!(client.connection_state().unwrap().host_id, "a");
    let new_keys = connection_keys(&client, "a");
    new_keys.pin(pin_for("a")).await;
    old_keys.unpair("a").await;
    // An old viewer must not obtain a fresh trust epoch by starting an attempt
    // after the forget, even if its dial task was scheduled late.
    old_keys.for_attempt().unwrap().unpair("a").await;
    assert!(keys.inner.pinned_host("a").await.is_some());
}

#[tokio::test]
async fn forget_waits_for_inflight_pin_then_removes_it() {
    let rig = rig();
    let keys = Arc::new(PausedHostKeys {
        inner: rig.client.core.parts.keys.clone(),
        pause_pin: true,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = client_with_keys(&rig, keys.clone());
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&client, "a");
    let pin = tokio::spawn(async move { old_keys.pin(pin_for("a")).await });
    keys.entered.notified().await;
    let forget = client.forget_host("a".into());
    tokio::pin!(forget);
    assert_pending(forget.as_mut()).await;
    keys.release.notify_one();
    pin.await.unwrap();
    forget.await;
    assert!(keys.inner.pinned_host("a").await.is_none());
    assert!(client.connection_state().is_none());
}

#[tokio::test]
async fn replacement_core_forget_invalidates_old_core_pin_and_cached_read() {
    let rig = rig();
    let coordinator = rig.client.core.parts.coordinator.clone();
    let replacement_store: Arc<dyn ViewerKeys> = Arc::new(KeyStoreBridge::with_coordinator(
        rig.store.boxed(),
        coordinator,
    ));
    let paused = Arc::new(PausedHostKeys {
        inner: replacement_store.clone(),
        pause_pin: false,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let replacement = client_with_keys(&rig, paused.clone());
    rig.client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&rig.client, "a");
    old_keys.pin(pin_for("a")).await;
    assert!(rig.client.core.parts.keys.pinned_host("a").await.is_some());

    let forgetting = replacement.clone();
    let forget = tokio::spawn(async move { forgetting.forget_host("a".into()).await });
    paused.entered.notified().await;
    let delayed_pin = old_keys.pin(pin_for("a"));
    tokio::pin!(delayed_pin);
    assert_pending(delayed_pin.as_mut()).await;
    paused.release.notify_one();
    forget.await.unwrap();
    delayed_pin.await;
    assert!(rig.client.core.parts.keys.pinned_host("a").await.is_none());
    assert!(replacement_store.pinned_host("a").await.is_none());
    assert!(!rig.store.state.lock().unwrap().pins.contains_key("a"));
}

#[tokio::test]
async fn replacement_core_forget_waits_for_an_old_core_inflight_pin() {
    let rig = rig();
    let paused = Arc::new(PausedHostKeys {
        inner: rig.client.core.parts.keys.clone(),
        pause_pin: true,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let old_client = client_with_keys(&rig, paused.clone());
    let replacement_store: Arc<dyn ViewerKeys> = Arc::new(KeyStoreBridge::with_coordinator(
        rig.store.boxed(),
        rig.client.core.parts.coordinator.clone(),
    ));
    let replacement = client_with_keys(&rig, replacement_store.clone());
    old_client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&old_client, "a");
    let pin = tokio::spawn(async move { old_keys.pin(pin_for("a")).await });
    paused.entered.notified().await;
    let forget = replacement.forget_host("a".into());
    tokio::pin!(forget);
    assert_pending(forget.as_mut()).await;
    paused.release.notify_one();
    pin.await.unwrap();
    forget.await;
    assert!(replacement_store.pinned_host("a").await.is_none());
    assert!(old_client.core.parts.keys.pinned_host("a").await.is_none());
    assert!(!rig.store.state.lock().unwrap().pins.contains_key("a"));
}

#[tokio::test(start_paused = true)]
async fn replacement_forget_stops_old_live_viewer_and_reconnect_persists_pin() {
    let rig = rig();
    let replacement = client_with_keys(
        &rig,
        Arc::new(KeyStoreBridge::with_coordinator(
            rig.store.boxed(),
            rig.client.core.parts.coordinator.clone(),
        )),
    );
    let unrelated = client_with_keys(&rig, rig.client.core.parts.keys.clone());
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    rig.client.connect(host.clone()).await.unwrap();
    until("first pairing", || {
        rig.client
            .connection_state()
            .is_some_and(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    rig.host.approve(&rig.host.conns()[0].conn_id).await;
    until("first ready", || {
        rig.client
            .connection_state()
            .is_some_and(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    replacement.connect(host.clone()).await.unwrap();
    until("replacement ready", || {
        replacement
            .connection_state()
            .is_some_and(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    unrelated.connect("different-host".into()).await.unwrap();
    replacement.forget_host(host.clone()).await;
    assert!(rig.client.connection_state().is_none());
    assert!(replacement.connection_state().is_none());
    assert!(matches!(
        rig.client.list_apps().await,
        Err(RemoteFfiError::NotConnected)
    ));
    assert_eq!(
        unrelated.connection_state().unwrap().host_id,
        "different-host"
    );

    rig.client.connect(host.clone()).await.unwrap();
    until("reconnected and pinned", || {
        rig.client
            .connection_state()
            .is_some_and(|s| s.phase == RemoteConnectionPhaseFfi::Ready)
            && rig.store.state.lock().unwrap().pins.contains_key(&host)
    })
    .await;
}

#[tokio::test]
async fn invalidating_a_collected_old_root_does_not_stop_its_replacement() {
    let rig = rig();
    rig.client.connect("a".into()).await.unwrap();
    let old = rig
        .client
        .core
        .active
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .root
        .clone();
    rig.client.connect("b".into()).await.unwrap();
    old.invalidate();
    assert_eq!(rig.client.connection_state().unwrap().host_id, "b");
}

#[tokio::test]
async fn forgetting_an_old_host_does_not_disconnect_its_replacement() {
    let rig = rig();
    rig.client.connect("a".into()).await.unwrap();
    rig.client.connect("b".into()).await.unwrap();
    rig.client.forget_host("a".into()).await;
    assert_eq!(rig.client.connection_state().unwrap().host_id, "b");
}

#[async_trait]
impl ViewerKeys for PausedResetKeys {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        self.inner.identity().await
    }
    async fn reset_identity(&self) -> Result<(), KeysError> {
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

struct FailedResetKeys(Arc<dyn ViewerKeys>);

#[async_trait]
impl ViewerKeys for FailedResetKeys {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        self.0.identity().await
    }
    async fn reset_identity(&self) -> Result<(), KeysError> {
        Err(KeysError("reset failed".into()))
    }
    async fn pinned_host(&self, id: &str) -> Option<PinnedHost> {
        self.0.pinned_host(id).await
    }
    async fn pin(&self, host: PinnedHost) {
        self.0.pin(host).await;
    }
    async fn unpair(&self, id: &str) {
        self.0.unpair(id).await;
    }
}

#[tokio::test]
async fn failed_repair_does_not_invalidate_the_existing_trust_epoch() {
    let rig = rig();
    let client = client_with_keys(
        &rig,
        Arc::new(FailedResetKeys(rig.client.core.parts.keys.clone())),
    );
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&client, "a");
    client.disconnect().await;
    assert!(matches!(
        client.repair_host("a".into()).await,
        Err(RemoteFfiError::KeyStore { .. })
    ));
    old_keys.pin(pin_for("a")).await;
    assert!(client.core.parts.keys.pinned_host("a").await.is_some());
    assert!(client.connection_state().is_none());
}

#[tokio::test(start_paused = true)]
async fn failed_in_place_repair_preserves_connection_and_trust() {
    use remote_control_client::signaling::AnswerRejectReason;
    let rig = rig();
    let client = client_with_keys(
        &rig,
        Arc::new(FailedResetKeys(rig.client.core.parts.keys.clone())),
    );
    rig.host
        .configure(|config| config.reject = Some(AnswerRejectReason::Revoked));
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    client.connect(host.clone()).await.unwrap();
    let old_keys = connection_keys(&client, &host);
    until("revoked", || {
        client.connection_state().is_some_and(|s| {
            s.phase == RemoteConnectionPhaseFfi::Failed
                && s.failure
                    .is_some_and(|f| f.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    let generation = client.core.current.load(Ordering::SeqCst);
    client.repair_host(host.clone()).await.unwrap();
    assert_eq!(client.core.current.load(Ordering::SeqCst), generation);
    old_keys.pin(pin_for(&host)).await;
    assert!(client.core.parts.keys.pinned_host(&host).await.is_some());
    assert_eq!(
        client.connection_state().unwrap().phase,
        RemoteConnectionPhaseFfi::Failed
    );
}

#[tokio::test(start_paused = true)]
async fn retry_and_connect_wait_for_in_place_repair_reset() {
    use remote_control_client::signaling::AnswerRejectReason;
    let rig = rig();
    let keys = Arc::new(PausedResetKeys {
        inner: rig.client.core.parts.keys.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = client_with_keys(&rig, keys.clone());
    rig.host
        .configure(|config| config.reject = Some(AnswerRejectReason::Revoked));
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    client.connect(host.clone()).await.unwrap();
    until("revoked", || {
        client.connection_state().is_some_and(|s| {
            s.phase == RemoteConnectionPhaseFfi::Failed
                && s.failure
                    .is_some_and(|f| f.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    let old_keys = connection_keys(&client, &host);
    keys.inner.pin(pin_for(&host)).await;
    let repairing = client.clone();
    let repairing_host = host.clone();
    let repair = tokio::spawn(async move { repairing.repair_host(repairing_host).await });
    keys.entered.notified().await;
    let old_unpair = old_keys.unpair(&host);
    tokio::pin!(old_unpair);
    assert_pending(old_unpair.as_mut()).await;
    let retry = client.retry();
    tokio::pin!(retry);
    assert_pending(retry.as_mut()).await;
    let connect = client.connect(host.clone());
    tokio::pin!(connect);
    assert_pending(connect.as_mut()).await;
    assert_eq!(
        client.connection_state().unwrap().phase,
        RemoteConnectionPhaseFfi::Failed
    );
    keys.release.notify_one();
    repair.await.unwrap().unwrap();
    assert!(
        keys.inner.pinned_host(&host).await.is_none(),
        "successful repair must clear the revoked pin before discarding the old unpair"
    );
    old_unpair.await;
    retry.await;
    connect.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_active_repair_finishes_delete_and_invalidates_old_scope() {
    use remote_control_client::signaling::AnswerRejectReason;
    let rig = rig();
    let keys = paused_delete_keys(&rig);
    let client = client_with_keys(&rig, keys.clone());
    let replacement = client_with_keys(&rig, keys.inner.clone());
    rig.host
        .configure(|config| config.reject = Some(AnswerRejectReason::Revoked));
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    client.connect(host.clone()).await.unwrap();
    client.attach_session("s1".into()).await.unwrap();
    until("revoked", || {
        client.connection_state().is_some_and(|s| {
            s.phase == RemoteConnectionPhaseFfi::Failed
                && s.failure
                    .is_some_and(|f| f.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    let old_keys = connection_keys(&client, &host);
    keys.inner.pin(pin_for(&host)).await;
    keys.pause.store(true, Ordering::SeqCst);
    let repairing = client.clone();
    let repair_host = host.clone();
    let repair = tokio::spawn(async move { repairing.repair_host(repair_host).await });
    keys.entered.notified().await;
    repair.abort();
    assert!(repair.await.unwrap_err().is_cancelled());
    let stale_pin = old_keys.pin(pin_for(&host));
    tokio::pin!(stale_pin);
    assert_pending(stale_pin.as_mut()).await;
    let connect = replacement.connect(host.clone());
    tokio::pin!(connect);
    assert_pending(connect.as_mut()).await;
    rig.host.configure(|config| config.reject = None);
    keys.pause.store(false, Ordering::SeqCst);
    keys.release.notify_one();
    stale_pin.await;
    connect.await.unwrap();
    assert!(keys.inner.pinned_host(&host).await.is_none());
    assert!(client
        .core
        .active
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .watchers
        .contains_key("s1"));
    let fresh = connection_keys(&replacement, &host);
    fresh.pin(pin_for(&host)).await;
    assert!(keys.inner.pinned_host(&host).await.is_some());
}

#[tokio::test]
async fn cancelled_inactive_repair_finishes_before_new_connection() {
    let rig = rig();
    let keys = paused_delete_keys(&rig);
    let client = client_with_keys(&rig, keys.clone());
    let replacement = client_with_keys(&rig, keys.inner.clone());
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&client, "a");
    client.disconnect().await;
    keys.inner.pin(pin_for("a")).await;
    keys.pause.store(true, Ordering::SeqCst);
    let repairing = client.clone();
    let repair = tokio::spawn(async move { repairing.repair_host("a".into()).await });
    keys.entered.notified().await;
    repair.abort();
    assert!(repair.await.unwrap_err().is_cancelled());
    let stale_pin = old_keys.pin(pin_for("a"));
    tokio::pin!(stale_pin);
    assert_pending(stale_pin.as_mut()).await;
    let connect = replacement.connect("a".into());
    tokio::pin!(connect);
    assert_pending(connect.as_mut()).await;
    keys.release.notify_one();
    stale_pin.await;
    connect.await.unwrap();
    assert!(keys.inner.pinned_host("a").await.is_none());
    connection_keys(&replacement, "a").pin(pin_for("a")).await;
    assert!(keys.inner.pinned_host("a").await.is_some());
}

#[tokio::test]
async fn cancelled_forget_keeps_lock_until_delete_finishes() {
    let rig = rig();
    let keys = paused_delete_keys(&rig);
    let client = client_with_keys(&rig, keys.clone());
    let replacement = client_with_keys(&rig, keys.inner.clone());
    client.connect("a".into()).await.unwrap();
    let old_keys = connection_keys(&client, "a");
    keys.inner.pin(pin_for("a")).await;
    keys.pause.store(true, Ordering::SeqCst);
    let forgetting = client.clone();
    let forget = tokio::spawn(async move { forgetting.forget_host("a".into()).await });
    keys.entered.notified().await;
    forget.abort();
    assert!(forget.await.unwrap_err().is_cancelled());
    let stale_pin = old_keys.pin(pin_for("a"));
    tokio::pin!(stale_pin);
    assert_pending(stale_pin.as_mut()).await;
    let connect = replacement.connect("a".into());
    tokio::pin!(connect);
    assert_pending(connect.as_mut()).await;
    keys.release.notify_one();
    stale_pin.await;
    connect.await.unwrap();
    assert!(keys.inner.pinned_host("a").await.is_none());
    connection_keys(&replacement, "a").pin(pin_for("a")).await;
    assert!(keys.inner.pinned_host("a").await.is_some());
}

#[tokio::test(start_paused = true)]
async fn dropping_owner_during_cancelled_forget_stops_unrelated_live_connection() {
    let rig = rig();
    let keys = paused_delete_keys(&rig);
    let client = client_with_keys(&rig, keys.clone());
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    client.connect(host.clone()).await.unwrap();
    until("pairing", || {
        client
            .connection_state()
            .is_some_and(|state| state.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    rig.host.approve(&rig.host.conns()[0].conn_id).await;
    until("ready", || {
        client
            .connection_state()
            .is_some_and(|state| state.phase == RemoteConnectionPhaseFfi::Ready)
    })
    .await;
    let connection = client
        .core
        .active
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .connection
        .clone();
    let weak_core = Arc::downgrade(&client.core);
    client.watch_hosts().await;
    let file_task = Arc::new(FileTask::default());
    client
        .core
        .file_tasks
        .lock()
        .unwrap()
        .insert("file".into(), file_task.clone());
    let generation = client.core.current.load(Ordering::SeqCst);
    keys.inner.pin(pin_for("forgotten-host")).await;
    keys.pause.store(true, Ordering::SeqCst);
    let forgetting = client.clone();
    let forget = tokio::spawn(async move { forgetting.forget_host("forgotten-host".into()).await });
    keys.entered.notified().await;
    forget.abort();
    assert!(forget.await.unwrap_err().is_cancelled());
    drop(client);
    assert_eq!(
        connection.state().phase,
        ConnectionPhase::Idle,
        "dropping the owner must close the unrelated live connection before the delete finishes"
    );
    let retained = weak_core.upgrade().unwrap();
    assert!(retained.active.lock().unwrap().is_none());
    assert!(retained.current.load(Ordering::SeqCst) > generation);
    assert!(retained.hosts_task.lock().unwrap().is_none());
    assert!(file_task.cancel.is_cancelled());
    until("delivery stopped", || retained.delivery.is_finished()).await;
    keys.release.notify_one();
    until("forgotten pin removed", || {
        !rig.store
            .state
            .lock()
            .unwrap()
            .pins
            .contains_key("forgotten-host")
    })
    .await;
}

#[tokio::test]
async fn dropping_owner_during_cancelled_inactive_repair_prevents_reconnect() {
    let rig = rig();
    let keys = Arc::new(PausedResetKeys {
        inner: rig.client.core.parts.keys.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = client_with_keys(&rig, keys.clone());
    let weak_core = Arc::downgrade(&client.core);
    let repairing = client.clone();
    let repair = tokio::spawn(async move { repairing.repair_host("repair-host".into()).await });
    keys.entered.notified().await;
    repair.abort();
    assert!(repair.await.unwrap_err().is_cancelled());
    drop(client);
    let retained = weak_core.upgrade().unwrap();
    let closed_generation = retained.current.load(Ordering::SeqCst);
    keys.release.notify_one();
    until("identity deleted", || {
        rig.store.state.lock().unwrap().identity_deletes == 1
    })
    .await;
    until("repair cleanup finished", || {
        retained.repairing.try_lock().is_ok()
    })
    .await;
    assert_eq!(
        retained.current.load(Ordering::SeqCst),
        closed_generation,
        "detached repair must not start a connection after the owner is dropped"
    );
    assert!(retained.active.lock().unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn dropping_owner_during_cancelled_active_repair_finishes_cleanup_without_reconnecting() {
    use remote_control_client::signaling::AnswerRejectReason;

    let rig = rig();
    let keys = paused_delete_keys(&rig);
    let client = client_with_keys(&rig, keys.clone());
    rig.host
        .configure(|config| config.reject = Some(AnswerRejectReason::Revoked));
    rig.host.publish_presence().await;
    let host = rig.host.host_id.clone();
    client.connect(host.clone()).await.unwrap();
    until("revoked", || {
        client.connection_state().is_some_and(|state| {
            state.phase == RemoteConnectionPhaseFfi::Failed
                && state
                    .failure
                    .is_some_and(|failure| failure.kind == RemoteFailureKindFfi::Revoked)
        })
    })
    .await;
    let old_keys = connection_keys(&client, &host);
    let epoch = old_keys.gate.trust_epoch.load(Ordering::SeqCst);
    keys.inner.pin(pin_for(&host)).await;
    let connection = client
        .core
        .active
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .connection
        .clone();
    let weak_core = Arc::downgrade(&client.core);
    keys.pause.store(true, Ordering::SeqCst);
    let repairing = client.clone();
    let repair_host = host.clone();
    let repair = tokio::spawn(async move { repairing.repair_host(repair_host).await });
    keys.entered.notified().await;
    repair.abort();
    assert!(repair.await.unwrap_err().is_cancelled());
    drop(client);
    assert_eq!(connection.state().phase, ConnectionPhase::Idle);
    let retained = weak_core.upgrade().unwrap();
    let generation = retained.current.load(Ordering::SeqCst);
    keys.release.notify_one();
    until("active repair cleanup finished", || {
        retained.repairing.try_lock().is_ok()
    })
    .await;
    assert_eq!(rig.store.state.lock().unwrap().identity_deletes, 1);
    assert!(keys.inner.pinned_host(&host).await.is_none());
    assert_eq!(old_keys.gate.trust_epoch.load(Ordering::SeqCst), epoch + 1);
    assert_eq!(retained.current.load(Ordering::SeqCst), generation);
    assert!(retained.active.lock().unwrap().is_none());
    assert_eq!(connection.state().phase, ConnectionPhase::Idle);
}

#[tokio::test]
async fn cancelling_forget_before_admission_has_no_effect() {
    let rig = rig();
    rig.client.connect("a".into()).await.unwrap();
    let keys = connection_keys(&rig.client, "a");
    keys.pin(pin_for("a")).await;
    let gate = keys.gate.clone();
    let held = gate.lock.lock().await;
    let mut forget = Box::pin(rig.client.forget_host("a".into()));
    assert_pending(forget.as_mut()).await;
    drop(forget);
    drop(held);
    assert!(rig.client.core.parts.keys.pinned_host("a").await.is_some());
    assert_eq!(rig.client.connection_state().unwrap().host_id, "a");
}

#[tokio::test(flavor = "current_thread")]
async fn forget_invalidates_live_roots_before_cleanup_task_first_poll() {
    let rig = rig();
    let other = client_with_keys(&rig, rig.client.core.parts.keys.clone());
    rig.client.connect("a".into()).await.unwrap();
    other.connect("a".into()).await.unwrap();
    rig.client.core.parts.keys.pin(pin_for("a")).await;
    let gate = rig.client.core.parts.coordinator.host_gates.get("a");
    let mut forget = Box::pin(rig.client.forget_host("a".into()));
    // This first poll admits the operation and spawns cleanup. On the
    // current-thread runtime the spawned task cannot poll in this call.
    assert_pending(forget.as_mut()).await;
    assert!(rig.client.connection_state().is_none());
    assert!(other.connection_state().is_none());
    assert!(rig.store.state.lock().unwrap().pins.contains_key("a"));
    drop(forget);
    let _guard = gate.lock.lock().await;
    assert!(!rig.store.state.lock().unwrap().pins.contains_key("a"));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_forget_retains_host_gate_without_any_live_connection() {
    let rig = rig();
    let keys = paused_delete_keys(&rig);
    keys.pause.store(true, Ordering::SeqCst);
    let client = client_with_keys(&rig, keys.clone());
    let mut forget = Box::pin(client.forget_host("a".into()));
    assert_pending(forget.as_mut()).await;
    drop(forget);
    let mut connect = Box::pin(client.connect("a".into()));
    assert_pending(connect.as_mut()).await;
    keys.entered.notified().await;
    keys.release.notify_one();
    connect.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn repair_snapshots_generation_before_transaction_task_first_poll() {
    let rig = rig();
    let mut repair = Box::pin(rig.client.repair_host("a".into()));
    // Poll the caller through admission, before the spawned task can run.
    assert_pending(repair.as_mut()).await;
    rig.client.connect("b".into()).await.unwrap();
    assert_eq!(rig.store.state.lock().unwrap().identity_deletes, 0);
    repair.await.unwrap();
    assert_eq!(rig.client.connection_state().unwrap().host_id, "b");
}

#[tokio::test]
async fn repair_cannot_replace_another_host_connected_during_identity_reset() {
    let rig = rig();
    let keys = Arc::new(PausedResetKeys {
        inner: rig.client.core.parts.keys.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = client_with_keys(&rig, keys.clone());
    let repairing = client.clone();
    let repair = tokio::spawn(async move { repairing.repair_host("a".into()).await });
    keys.entered.notified().await;
    client.connect("b".into()).await.unwrap();
    keys.release.notify_one();
    repair.await.unwrap().unwrap();
    assert_eq!(client.connection_state().unwrap().host_id, "b");
}

#[tokio::test(start_paused = true)]
async fn disconnect_during_repair_without_an_active_host_prevents_a_late_connect() {
    let rig = rig();
    let parts = &rig.client.core.parts;
    let keys = Arc::new(PausedResetKeys {
        inner: parts.keys.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let client = Arc::new(RemoteControlCore::from_parts(
        Parts {
            uid: parts.uid.clone(),
            rtdb: parts.rtdb.clone(),
            keys: keys.clone(),
            coordinator: parts.coordinator.clone(),
            clock: parts.clock.clone(),
            random: parts.random.clone(),
            peers: parts.peers.clone(),
            media_offer_handler: parts.media_offer_handler.clone(),
            device_name: parts.device_name.clone(),
            app_version: parts.app_version.clone(),
        },
        rig.recorder.clone(),
    ));
    let repairing = client.clone();
    let repair = tokio::spawn(async move { repairing.repair_host("second-host".into()).await });
    keys.entered.notified().await;
    client.disconnect().await;
    keys.release.notify_one();
    repair.await.unwrap().unwrap();
    assert!(
        client.connection_state().is_none(),
        "a cancelled repair cannot reconnect a closed client"
    );
}
