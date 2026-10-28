//! Host tests of `remote.rs`: conversions, the delta rule, the callback bridges,
//! and a whole pairing / attach / prompt flow against the viewer crate's fake desktop.

use super::*;
use remote_control_client::clock::ManualClock;
use remote_control_client::rpc::RemoteErrorCode;
use remote_control_client::rtdb::MemoryRtdb;
use remote_control_client::session::SessionError;
use remote_control_client::testing::FakePeerNetwork;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

// The fake desktop of the viewer crate's own integration tests (it only uses that
// crate's public API), so the flow below talks to a real tunnel and handshake.
#[path = "../../../remote-control-client/tests/support/host.rs"]
mod fake_host;
use fake_host::FakeHost;

const UID: &str = "user-1";

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

#[derive(Debug, Clone, PartialEq)]
enum SeenBots {
    Bots(String, RemoteBotsStateFfi),
    Threads(String, String, RemoteBotThreadsStateFfi),
}

/// The app's Bots listener; clones share what was heard.
#[derive(Default, Clone)]
struct BotsRecorder {
    seen: Arc<Mutex<Vec<SeenBots>>>,
}

impl BotsRecorder {
    fn all(&self) -> Vec<SeenBots> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl PlatformRemoteBotsListener for BotsRecorder {
    async fn on_bots(&self, host_id: String, state: RemoteBotsStateFfi) {
        self.seen
            .lock()
            .unwrap()
            .push(SeenBots::Bots(host_id, state));
    }
    async fn on_bot_threads(
        &self,
        host_id: String,
        bot_id: String,
        state: RemoteBotThreadsStateFfi,
    ) {
        self.seen
            .lock()
            .unwrap()
            .push(SeenBots::Threads(host_id, bot_id, state));
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
        bot_id: None,
    };
    let ffi = session_summary_to_ffi(&summary);
    assert_eq!(ffi.status, RemoteSessionStatusFfi::RequiresAction);
    assert_eq!(
        ffi.pending_action,
        Some(RemotePendingActionFfi::ComputerAccess)
    );
    assert_eq!((ffi.viewers, ffi.model.as_deref()), (2, Some("m")));
}

fn a_bot(id: &str, status: RemoteBotStatus) -> RemoteBotSummary {
    RemoteBotSummary {
        bot_id: id.into(),
        name: format!("Bot {id}"),
        description: format!("{id} helps"),
        status,
        waiting_threads: 2,
        last_activity_at: Some(1_790_000_000_000),
    }
}

#[test]
fn bots_map_status_waiting_chats_and_activity() {
    let bot = a_bot("b1", RemoteBotStatus::RequiresAction);
    assert_eq!(
        bot_summary_to_ffi(&bot),
        RemoteBotSummaryFfi {
            bot_id: "b1".into(),
            name: "Bot b1".into(),
            description: "b1 helps".into(),
            status: RemoteBotStatusFfi::RequiresAction,
            waiting_threads: 2,
            last_activity_at_ms: Some(1_790_000_000_000),
        }
    );
    for (status, expected) in [
        (RemoteBotStatus::Idle, RemoteBotStatusFfi::Idle),
        (RemoteBotStatus::Running, RemoteBotStatusFfi::Running),
        (
            RemoteBotStatus::RequiresAction,
            RemoteBotStatusFfi::RequiresAction,
        ),
    ] {
        let quiet = RemoteBotSummary {
            last_activity_at: None,
            ..a_bot("b1", status)
        };
        let ffi = bot_summary_to_ffi(&quiet);
        assert_eq!((ffi.status, ffi.last_activity_at_ms), (expected, None));
    }
}

#[test]
fn the_bots_state_keeps_unknown_unsupported_and_no_bots_apart() {
    assert_eq!(
        bots_state_to_ffi(&BotsState::Unknown),
        RemoteBotsStateFfi::Unknown
    );
    assert_eq!(
        bots_state_to_ffi(&BotsState::Unsupported),
        RemoteBotsStateFfi::Unsupported
    );
    assert_eq!(
        bots_state_to_ffi(&BotsState::Bots(Vec::new())),
        RemoteBotsStateFfi::Bots { bots: Vec::new() }
    );
    let state = BotsState::Bots(vec![
        a_bot("b2", RemoteBotStatus::Running),
        a_bot("b1", RemoteBotStatus::Idle),
    ]);
    match bots_state_to_ffi(&state) {
        RemoteBotsStateFfi::Bots { bots } => assert_eq!(
            bots.iter()
                .map(|bot| (bot.bot_id.as_str(), bot.status))
                .collect::<Vec<_>>(),
            [
                ("b2", RemoteBotStatusFfi::Running),
                ("b1", RemoteBotStatusFfi::Idle)
            ],
            "in the computer's order"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bots_chats_map_like_sessions_and_keep_why_a_listing_failed() {
    let chat = RemoteSessionSummary {
        session_id: "t1".into(),
        title: "Reading list".into(),
        project_label: "Researcher".into(),
        status: RemoteSessionStatus::Idle,
        pending_action: None,
        model: None,
        viewers: 0,
        bot_id: Some("b1".into()),
    };
    assert_eq!(
        bot_threads_state_to_ffi(&BotThreadsState {
            threads: Some(vec![chat.clone()]),
            error: None,
        }),
        RemoteBotThreadsStateFfi {
            threads: Some(vec![session_summary_to_ffi(&chat)]),
            error: None,
        }
    );
    assert_eq!(
        bot_threads_state_to_ffi(&BotThreadsState {
            threads: None,
            error: Some(SessionError {
                code: "not_remote_enabled".into(),
                message: "off".into(),
            }),
        }),
        RemoteBotThreadsStateFfi {
            threads: None,
            error: Some(RemoteSessionErrorFfi {
                code: "not_remote_enabled".into(),
                message: "off".into(),
            }),
        }
    );
    assert_eq!(
        bot_threads_state_to_ffi(&BotThreadsState::default()),
        RemoteBotThreadsStateFfi::default()
    );
    assert_eq!(
        RemoteBotThreadsStateFfi::default(),
        RemoteBotThreadsStateFfi {
            threads: None,
            error: None
        },
        "nothing listed is not an empty list"
    );
}

#[test]
fn a_prompt_a_bot_handed_over_still_reads_as_scheduled_in_its_record() {
    // The foreign enum cannot gain a variant: the Bot comes from `prompt_origin_bot_name`.
    let record = record_json(json!({
        "type": "prompt", "turn_id": 5, "text": "do it", "origin": "bot", "from_bot_name": "Researcher"
    }));
    match record_to_ffi(&record) {
        RemoteRecordFfi::Prompt {
            origin,
            device_name,
            ..
        } => {
            assert_eq!(origin, RemotePromptOriginFfi::Scheduled);
            assert_eq!(device_name, None);
        }
        other => panic!("{other:?}"),
    }
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
    let parts = Parts {
        uid: UID.into(),
        rtdb: rtdb as _,
        keys: Arc::new(KeyStoreBridge::new(store.boxed())),
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
async fn bots_calls_without_a_connection_say_so_and_empty_arguments_are_refused() {
    let rig = rig();
    let client = &rig.client;
    assert_eq!(client.bots_state(), RemoteBotsStateFfi::Unknown);
    assert_eq!(
        client.bot_threads_state("b1".into()),
        RemoteBotThreadsStateFfi::default()
    );
    assert_eq!(
        client.activate_bot_threads("b1".into()).await,
        Err(RemoteFfiError::NotConnected)
    );
    assert_eq!(
        client.create_bot_thread("b1".into()).await,
        Err(RemoteFfiError::NotConnected)
    );
    client.deactivate_bot_threads("b1".into());
    assert_eq!(client.bot_of_thread("t1".into()), None);
    assert_eq!(client.prompt_origin_bot_name("s1".into(), 1), None);
    assert!(matches!(
        client.activate_bot_threads(String::new()).await,
        Err(RemoteFfiError::InvalidArgument { .. })
    ));
    assert!(matches!(
        client.create_bot_thread(String::new()).await,
        Err(RemoteFfiError::InvalidArgument { .. })
    ));
    // The listener may be set and cleared at any time.
    client.set_bots_listener(Some(Box::new(BotsRecorder::default())));
    client.set_bots_listener(None);
}

#[tokio::test(start_paused = true)]
async fn bots_their_chats_and_the_bot_of_a_prompt_over_the_tunnel() {
    let rig = rig();
    let client = &rig.client;
    let heard = BotsRecorder::default();
    rig.host.configure(|config| {
        config.respond = Some(Arc::new(|method: &str, params: &Value| match method {
            "sessions.watch" => Some(Ok(json!({ "sessions": [] }))),
            "session.attach" => Some(Ok(json!({ "mode": "snapshot", "epoch": "e1" }))),
            "bots.watch" => Some(Ok(json!({ "bots": [
                { "bot_id": "b1", "name": "Researcher", "description": "Reads the web",
                  "status": "idle", "waiting_threads": 0 },
                { "bot_id": "b2", "name": "Writer", "description": "",
                  "status": "requires_action", "waiting_threads": 1,
                  "last_activity_at": 1_790_000_000_000_u64 },
            ] }))),
            "bot.threads.list" => Some(Ok(json!({ "sessions": [{
                "session_id": "t1", "title": "Reading list", "project_label": "Researcher",
                "status": "idle", "viewers": 0, "bot_id": params["bot_id"],
            }] }))),
            "bot.thread.create" => Some(Ok(json!({ "session_id": "t2" }))),
            _ => None,
        }));
    });
    client.set_bots_listener(Some(Box::new(heard.clone())));
    rig.host.publish_presence().await;
    let host_id = rig.host.host_id.clone();
    client.connect(host_id.clone()).await.unwrap();
    until("the pairing code", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.approve(&conn_id).await;

    // The Bots arrive by themselves once ready, and the desktop was told that this viewer knows them.
    until("the Bots", || {
        heard.all().iter().any(|seen| {
            matches!(seen, SeenBots::Bots(h, RemoteBotsStateFfi::Bots { bots })
                if *h == host_id && bots.len() == 2)
        })
    })
    .await;
    assert_eq!(
        requests_of(&rig.host, "initialize")[0]["app"]["features"],
        json!(["bots"])
    );
    let RemoteBotsStateFfi::Bots { bots: listed } = client.bots_state() else {
        panic!("the Bots are not held");
    };
    assert_eq!(
        listed
            .iter()
            .map(|bot| bot.bot_id.as_str())
            .collect::<Vec<_>>(),
        ["b1", "b2"]
    );
    assert_eq!(listed[0].last_activity_at_ms, None);
    assert_eq!(listed[1].status, RemoteBotStatusFfi::RequiresAction);
    assert_eq!(
        (listed[1].waiting_threads, listed[1].last_activity_at_ms),
        (1, Some(1_790_000_000_000))
    );

    // A Bot's chats are listed when a screen shows them.
    assert!(requests_of(&rig.host, "bot.threads.list").is_empty());
    assert_eq!(
        client.bot_threads_state("b1".into()),
        RemoteBotThreadsStateFfi::default()
    );
    client.activate_bot_threads("b1".into()).await.unwrap();
    until("the chats of b1", || {
        heard.all().iter().any(|seen| {
            matches!(seen, SeenBots::Threads(h, bot, state)
                if *h == host_id && bot == "b1" && state.threads.is_some())
        })
    })
    .await;
    assert_eq!(
        requests_of(&rig.host, "bot.threads.list"),
        [json!({ "bot_id": "b1" })]
    );
    let chats = client.bot_threads_state("b1".into()).threads.unwrap();
    assert_eq!(
        chats
            .iter()
            .map(|chat| (chat.session_id.as_str(), chat.title.as_str()))
            .collect::<Vec<_>>(),
        [("t1", "Reading list")]
    );
    assert_eq!(client.bot_of_thread("t1".into()).as_deref(), Some("b1"));
    assert_eq!(client.bot_of_thread("t9".into()), None);

    // A new chat is asked for with a fresh request id, and its Bot is remembered.
    assert_eq!(client.create_bot_thread("b2".into()).await.unwrap(), "t2");
    assert_eq!(client.bot_of_thread("t2".into()).as_deref(), Some("b2"));
    let created = &requests_of(&rig.host, "bot.thread.create")[0];
    assert_eq!(created["bot_id"], "b2");
    assert!(!created["client_request_id"].as_str().unwrap().is_empty());

    // The record of a Bot's prompt reads as scheduled; the lookup tells which Bot.
    client.attach_session("t1".into()).await.unwrap();
    until("the attach", || {
        !requests_of(&rig.host, "session.attach").is_empty()
    })
    .await;
    for part in [
        json!({ "phase": "begin", "session_id": "t1", "epoch": "e1", "first_message_index": 0 }),
        json!({ "phase": "records", "session_id": "t1", "records": [
            { "seq": 1, "record": { "type": "prompt", "turn_id": 1, "text": "summarise",
                "origin": "bot", "from_bot_name": "Re\u{202e}searcher" } },
            { "seq": 2, "record": { "type": "prompt", "turn_id": 2, "text": "again",
                "origin": "scheduled" } },
        ] }),
        json!({ "phase": "end", "session_id": "t1", "cursor": { "epoch": "e1", "seq": 2 },
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
    assert_eq!(snapshot.records.len(), 2);
    assert!(snapshot.records.iter().all(|item| matches!(
        item.record,
        RemoteRecordFfi::Prompt {
            origin: RemotePromptOriginFfi::Scheduled,
            ..
        }
    )));
    assert_eq!(
        client.prompt_origin_bot_name("t1".into(), 1).as_deref(),
        Some("Researcher")
    );
    assert_eq!(client.prompt_origin_bot_name("t1".into(), 2), None);
    assert_eq!(client.prompt_origin_bot_name("t1".into(), 99), None);
    assert_eq!(client.prompt_origin_bot_name("t2".into(), 1), None);

    // The Bot's activity changes while the screen shows its chats: they are listed again.
    let changed = |activity: u64| {
        json!({ "bots": [
            { "bot_id": "b1", "name": "Researcher", "description": "Reads the web",
              "status": "idle", "waiting_threads": 0, "last_activity_at": activity },
            { "bot_id": "b2", "name": "Writer", "description": "",
              "status": "requires_action", "waiting_threads": 1,
              "last_activity_at": 1_790_000_000_000_u64 },
        ] })
    };
    rig.host.notify(&conn_id, "bots.changed", changed(5)).await;
    until("the second listing", || {
        requests_of(&rig.host, "bot.threads.list").len() == 2
    })
    .await;
    until("the changed Bots", || {
        matches!(client.bots_state(), RemoteBotsStateFfi::Bots { bots }
            if bots[0].last_activity_at_ms == Some(5))
    })
    .await;

    // Once the screen leaves, the Bots are still reported but the chats are not listed again.
    client.deactivate_bot_threads("b1".into());
    rig.host.notify(&conn_id, "bots.changed", changed(6)).await;
    until("the Bots heard", || {
        heard.all().iter().any(|seen| {
            matches!(seen, SeenBots::Bots(_, RemoteBotsStateFfi::Bots { bots })
                if bots[0].last_activity_at_ms == Some(6))
        })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(requests_of(&rig.host, "bot.threads.list").len(), 2);

    // With no listener the state is still held, and nothing is called.
    client.set_bots_listener(None);
    let calls = heard.all().len();
    rig.host.notify(&conn_id, "bots.changed", changed(7)).await;
    until("the Bots held", || {
        matches!(client.bots_state(), RemoteBotsStateFfi::Bots { bots }
            if bots[0].last_activity_at_ms == Some(7))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(heard.all().len(), calls);

    // Disconnecting ends it all.
    client.disconnect().await;
    assert_eq!(client.bots_state(), RemoteBotsStateFfi::Unknown);
    assert_eq!(client.bot_of_thread("t1".into()), None);
    assert_eq!(client.prompt_origin_bot_name("t1".into(), 1), None);
}

#[tokio::test(start_paused = true)]
async fn a_computer_from_before_bots_reads_as_unsupported_and_is_asked_nothing_more() {
    let rig = rig();
    let client = &rig.client;
    let heard = BotsRecorder::default();
    rig.host.configure(|config| {
        config.respond = Some(Arc::new(|method: &str, _params: &Value| match method {
            "bots.watch" | "bot.threads.list" | "bot.thread.create" => Some(Err(RpcError::new(
                RemoteErrorCode::InvalidParams,
                format!("unknown method {method}"),
            ))),
            "sessions.watch" => Some(Ok(json!({ "sessions": [] }))),
            _ => None,
        }));
    });
    client.set_bots_listener(Some(Box::new(heard.clone())));
    rig.host.publish_presence().await;
    let host_id = rig.host.host_id.clone();
    client.connect(host_id.clone()).await.unwrap();
    until("the pairing code", || {
        rig.recorder
            .states()
            .iter()
            .any(|s| s.phase == RemoteConnectionPhaseFfi::Pairing)
    })
    .await;
    let conn_id = rig.host.conns()[0].conn_id.clone();
    rig.host.approve(&conn_id).await;
    until("Unsupported", || {
        heard.all().contains(&SeenBots::Bots(
            host_id.clone(),
            RemoteBotsStateFfi::Unsupported,
        ))
    })
    .await;
    assert_eq!(client.bots_state(), RemoteBotsStateFfi::Unsupported);

    // A screen may still ask; nothing reaches the computer, and a new chat fails at once.
    client.activate_bot_threads("b1".into()).await.unwrap();
    match client.create_bot_thread("b1".into()).await {
        Err(RemoteFfiError::Rpc { code, .. }) => assert_eq!(code, "unknown_session"),
        other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(requests_of(&rig.host, "bots.watch").len(), 1);
    assert!(requests_of(&rig.host, "bot.threads.list").is_empty());
    assert!(requests_of(&rig.host, "bot.thread.create").is_empty());
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
    deliver(queue, recorder.clone(), current, Arc::default()).await;
    let seen = recorder.all();
    assert_eq!(seen.len(), 1);
    assert!(matches!(seen[0], Seen::Session(_)));
}

#[tokio::test]
async fn bots_calls_reach_the_listener_in_order_and_old_connections_are_dropped() {
    let (out, queue) = mpsc::unbounded_channel();
    let current = Arc::new(AtomicU64::new(2));
    let recorder = Arc::new(Recorder::default());
    let heard = BotsRecorder::default();
    let slot = BotsListenerSlot::default();
    *slot.lock().unwrap() = Some(Arc::new(heard.clone()));
    let chats = RemoteBotThreadsStateFfi {
        threads: Some(Vec::new()),
        error: None,
    };
    out.send(Out::Bots(
        1,
        "old-host".into(),
        RemoteBotsStateFfi::Unsupported,
    ))
    .unwrap();
    out.send(Out::BotThreads(
        1,
        "old-host".into(),
        "b1".into(),
        chats.clone(),
    ))
    .unwrap();
    out.send(Out::Bots(2, "host".into(), RemoteBotsStateFfi::Unsupported))
        .unwrap();
    out.send(Out::Sessions(2, "host".into(), vec![])).unwrap();
    out.send(Out::BotThreads(
        2,
        "host".into(),
        "b1".into(),
        chats.clone(),
    ))
    .unwrap();
    drop(out);
    deliver(queue, recorder.clone(), current, slot).await;
    assert_eq!(
        heard.all(),
        [
            SeenBots::Bots("host".into(), RemoteBotsStateFfi::Unsupported),
            SeenBots::Threads("host".into(), "b1".into(), chats)
        ]
    );
    let seen = recorder.all();
    assert_eq!(seen.len(), 1, "the session list is the other listener's");
    assert!(matches!(seen[0], Seen::Sessions(..)));
}

#[tokio::test]
async fn bots_calls_are_dropped_while_no_listener_is_set_and_follow_a_replaced_one() {
    let (out, queue) = mpsc::unbounded_channel();
    let current = Arc::new(AtomicU64::new(1));
    let first = BotsRecorder::default();
    let second = BotsRecorder::default();
    let slot = BotsListenerSlot::default();
    let deliver_task = tokio::spawn(deliver(
        queue,
        Arc::new(Recorder::default()),
        current,
        Arc::clone(&slot),
    ));
    let send = |state| out.send(Out::Bots(1, "host".into(), state)).unwrap();
    let settle = || async {
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
    };

    send(RemoteBotsStateFfi::Unknown);
    settle().await;
    *slot.lock().unwrap() = Some(Arc::new(first.clone()));
    send(RemoteBotsStateFfi::Unsupported);
    settle().await;
    *slot.lock().unwrap() = Some(Arc::new(second.clone()));
    send(RemoteBotsStateFfi::Bots { bots: Vec::new() });
    settle().await;
    *slot.lock().unwrap() = None;
    send(RemoteBotsStateFfi::Unknown);
    settle().await;

    assert_eq!(
        first.all(),
        [SeenBots::Bots(
            "host".into(),
            RemoteBotsStateFfi::Unsupported
        )]
    );
    assert_eq!(
        second.all(),
        [SeenBots::Bots(
            "host".into(),
            RemoteBotsStateFfi::Bots { bots: Vec::new() }
        )]
    );
    drop(out);
    deliver_task.await.unwrap();
}

#[test]
fn the_relay_forwards_bots_and_their_chats_only_while_its_connection_is_current() {
    let (out, mut queue) = mpsc::unbounded_channel();
    let current = Arc::new(AtomicU64::new(3));
    let relay = SessionsRelay {
        out,
        current: Arc::clone(&current),
        generation: 3,
        host_id: "host".into(),
    };
    relay.on_bots(&BotsState::Unsupported);
    relay.on_bot_threads("b1", &BotThreadsState::default());
    current.store(4, Ordering::SeqCst);
    relay.on_bots(&BotsState::Unknown);
    relay.on_bot_threads("b2", &BotThreadsState::default());

    match queue.try_recv() {
        Ok(Out::Bots(generation, host_id, state)) => {
            assert_eq!(generation, 3);
            assert_eq!(host_id, "host");
            assert_eq!(state, RemoteBotsStateFfi::Unsupported);
        }
        _ => panic!("the Bots were not forwarded first"),
    }
    match queue.try_recv() {
        Ok(Out::BotThreads(generation, host_id, bot_id, state)) => {
            assert_eq!(generation, 3);
            assert_eq!((host_id.as_str(), bot_id.as_str()), ("host", "b1"));
            assert_eq!(state, RemoteBotThreadsStateFfi::default());
        }
        _ => panic!("the chats were not forwarded second"),
    }
    assert!(
        queue.try_recv().is_err(),
        "nothing after the connection was replaced"
    );
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
