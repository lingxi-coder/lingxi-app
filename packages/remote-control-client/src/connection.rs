//! The viewer's side of a Remote Control connection to one computer: it dials,
//! proves who it is, falls back to the RTDB tunnel when no direct path works, runs
//! the `initialize` handshake (including pairing), and keeps the link up.
//!
//! This is the state machine of `apps/web/src/remote/viewerConnection.ts`.
//! Everything with side effects is injected (database, clock, WebRTC, key store),
//! so a test can stand a fake desktop on the other end of a `MemoryRtdb`.
//! [`ViewerConnection::start`] begins, [`ViewerConnection::stop`] ends; in between
//! [`ViewerConnection::state`] says where things stand and the
//! [`ConnectionListener`]s hear about changes, in order, from one task.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::clock::{system_clock, Clock};
use crate::device::{DeviceIdentity, PinnedHost, TunnelMemory, ViewerKeys};
use crate::direct::DirectLink;
use crate::encoding::{b64u_decode, b64u_encode, random_bytes, random_id, random_unit};
use crate::keys::EphemeralKey;
use crate::media::{
    RemoteMediaAnswer, RemoteMediaConnection, RemoteMediaControlChannel, RemoteMediaControlRequest,
    RemoteMediaOffer, RemoteMediaOfferHandler, RemoteMediaPlaybackReceipt, MEDIA_CONTROL_METHOD,
    MEDIA_OFFER_METHOD, MEDIA_PLAYBACK_RECEIPT_METHOD,
};
use crate::messages::{
    methods, AuthApprovedParams, AuthDeniedParams, InitializeParams, InitializeResult,
    RemoteAppInfo, RemoteHostInfo,
};
use crate::pairing::pairing_code_for;
use crate::peer::{PeerAnswer, PeerConfig, PeerConnector, PeerHandle};
use crate::rpc::{
    KeepaliveKind, NotificationHandler, RemoteErrorCode, RequestHandler, RpcError, RpcOptions,
    RpcPeer,
};
use crate::rtdb::{ChangeKind, ListenItem, Listener, Rtdb, RtdbError, RtdbListenError, RtdbMirror};
use crate::signaling::{
    answer_path, down_path, host_path, offer_path, parse_host_record, sign_offer,
    to_host_conn_path, to_viewer_conn_path, up_path, verify_answer, AnswerRecord,
    AnswerRejectReason, AnswerStatus, DevicePlatform, OfferDevice, OfferDraft, SigningContext,
    TransportKind, VerifyAnswerOptions, VerifyFailure,
};
use crate::text::{display_text, DEVICE_NAME_MAX_CHARS};
use crate::transport::{Link, LinkEvent};
use crate::tunnel::{TunnelLink, TunnelLinkOptions};
use crate::tunnel_crypto::derive_tunnel_keys;
use crate::version::{
    client_protocol_major, DATA_CHANNEL_LABEL, DATA_CHANNEL_OPEN_TIMEOUT, DEFAULT_STUN_URLS,
    ICE_GATHER_TIMEOUT, OFFER_ANSWER_TIMEOUT, REMOTE_PROTOCOL_MAJOR, SDP_MAX_CHARS,
    SSE_BACKOFF_MAX, SSE_BACKOFF_MIN, SSE_BACKOFF_RESET,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Where a connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionPhase {
    /// Nothing running: not started, or stopped.
    Idle,
    /// Dialling: writing the offer, waiting for the answer, opening the channel, handshaking.
    Connecting,
    /// The desktop is asking its user whether to allow this device; the pairing code is shown.
    AwaitingApproval,
    /// Connected and approved.
    Ready,
    /// Connected before and lost it; waiting out the backoff, or dialling again.
    Reconnecting,
    /// Stopped trying; the failure says why and whether the user can try again.
    Failed,
}

/// Why a connection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// This device cannot create or use its keys.
    Unsupported,
    /// First pairing, but `hosts/{host_id}` is missing or malformed.
    UnknownHost,
    /// The Realtime Database could not be reached, or refused.
    Network,
    /// The desktop did not answer within 30 s.
    NoAnswer,
    /// The link ended. The detail is the machine-readable reason.
    Closed,
    /// The desktop has too many connections.
    Capacity,
    /// The desktop is busy.
    Busy,
    /// The desktop no longer trusts this device. The pin is already cleared.
    Revoked,
    /// Remote control is switched off on the desktop.
    Disabled,
    /// The protocol major differs.
    UpgradeRequired,
    /// The user said no on the desktop.
    Denied,
    /// The desktop thinks this device's clock is off.
    Expired,
    /// Another reason the desktop gave for refusing.
    Rejected,
    /// The answer did not verify; the detail says how.
    Verification,
    /// The desktop computed a different pairing code, so someone is in the middle.
    PairingCodeMismatch,
    /// The desktop broke the protocol.
    Protocol,
}

impl FailureKind {
    /// The TypeScript spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::UnknownHost => "unknown_host",
            Self::Network => "network",
            Self::NoAnswer => "no_answer",
            Self::Closed => "closed",
            Self::Capacity => "capacity",
            Self::Busy => "busy",
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
            Self::UpgradeRequired => "upgrade_required",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Rejected => "rejected",
            Self::Verification => "verification",
            Self::PairingCodeMismatch => "pairing_code_mismatch",
            Self::Protocol => "protocol",
        }
    }
}

/// A failure and whether the connection retries by itself (with backoff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFailure {
    /// What failed.
    pub kind: FailureKind,
    /// Machine-readable detail.
    pub detail: Option<String>,
    /// Whether the connection retries by itself.
    pub auto: bool,
}

impl ConnectionFailure {
    fn new(kind: FailureKind, detail: Option<String>, auto: bool) -> Self {
        Self { kind, detail, auto }
    }
}

/// A snapshot of the connection.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionState {
    /// The phase.
    pub phase: ConnectionPhase,
    /// The computer being dialled.
    pub host_id: String,
    /// How the current, or the last, connection travels.
    pub transport: Option<TransportKind>,
    /// While awaiting approval: the code the desktop's dialog shows as well.
    pub pairing_code: Option<String>,
    /// In `Failed`, and in `Reconnecting` as the reason the link dropped.
    pub failure: Option<ConnectionFailure>,
    /// Consecutive failed attempts since the connection was last stable.
    pub attempt: u32,
    /// When the next attempt starts (wall clock, ms), while waiting to reconnect.
    pub retry_at: Option<u64>,
    /// The desktop's description of itself, once approved.
    pub host: Option<RemoteHostInfo>,
    /// Whether the connection has been ready at least once.
    pub ever_ready: bool,
}

/// Hears about a connection, in order, from one task. Implementations must not block.
pub trait ConnectionListener: Send + Sync {
    /// The state changed.
    fn on_state(&self, _state: &ConnectionState) {}
    /// A link became ready, including after a reconnect.
    fn on_ready(&self) {}
    /// A host notification from the ready link (the handshake's own `auth.*` stay inside).
    fn on_notification(&self, _method: &str, _params: &Value) {}
}

/// Identifies a registered listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

/// Everything a [`ViewerConnection`] needs from its surroundings.
#[derive(Clone)]
pub struct ViewerConnectionDeps {
    /// The user's Firebase uid.
    pub uid: String,
    /// The computer to dial.
    pub host_id: String,
    /// The database.
    pub rtdb: Arc<dyn Rtdb>,
    /// Identity and pinned hosts.
    pub keys: Arc<dyn ViewerKeys>,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Jitter source in `[0, 1)`.
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
    /// Absent: this build has no WebRTC, so every connection uses the tunnel.
    pub peers: Option<Arc<dyn PeerConnector>>,
    /// STUN servers for direct attempts.
    pub stun_urls: Vec<String>,
    /// Always use the tunnel (the debug switch / `LINGXI_REMOTE_FORCE_TUNNEL`).
    pub force_tunnel: bool,
    /// Which hosts recently needed the tunnel; shared across connections.
    pub tunnel_memory: Option<Arc<TunnelMemory>>,
    /// Name shown next to the pairing code on the desktop.
    pub device_name: String,
    /// App version.
    pub app_version: String,
    /// Kind of app.
    pub platform: DevicePlatform,
    /// Push registration of this install.
    pub push_installation_id: Option<String>,
    /// Product-layer answerer for host-initiated media negotiation. It is called
    /// only after the signed Remote Control connection is approved.
    pub media_offer_handler: Option<Arc<dyn RemoteMediaOfferHandler>>,
}

impl std::fmt::Debug for ViewerConnectionDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerConnectionDeps")
            .field("host_id", &self.host_id)
            .finish_non_exhaustive()
    }
}

impl ViewerConnectionDeps {
    /// Dependencies with defaults for everything optional: the system clock and
    /// randomness, no WebRTC, the default STUN servers.
    pub fn new(
        uid: impl Into<String>,
        host_id: impl Into<String>,
        rtdb: Arc<dyn Rtdb>,
        keys: Arc<dyn ViewerKeys>,
        platform: DevicePlatform,
    ) -> Self {
        Self {
            uid: uid.into(),
            host_id: host_id.into(),
            rtdb,
            keys,
            clock: system_clock(),
            random: Arc::new(random_unit),
            peers: None,
            stun_urls: DEFAULT_STUN_URLS.iter().map(|s| (*s).to_owned()).collect(),
            force_tunnel: false,
            tunnel_memory: None,
            device_name: match platform {
                DevicePlatform::Web => "Browser · Web".into(),
                DevicePlatform::Android => "Android".into(),
                DevicePlatform::Ios => "iOS".into(),
            },
            app_version: "dev".into(),
            platform,
            push_installation_id: None,
            media_offer_handler: None,
        }
    }
}

/// Backoff before retry number `failures` (1-based): 1 s doubling to 30 s, scaled by 0.5 to 1.0.
pub fn backoff_ms(failures: u32, random: &dyn Fn() -> f64) -> u64 {
    let exponent = failures.saturating_sub(1).min(16);
    let base = (SSE_BACKOFF_MIN.as_millis() as u64)
        .saturating_mul(1u64 << exponent)
        .min(SSE_BACKOFF_MAX.as_millis() as u64);
    (base as f64 * (0.5 + 0.5 * random())).round() as u64
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// How long `initialize` may take; while pairing it answers at once, the wait for the user comes after.
const HANDSHAKE_TIMEOUT: Duration = DATA_CHANNEL_OPEN_TIMEOUT;
/// Failed attempts in a row after which the connection stops retrying on its own.
const MAX_AUTO_ATTEMPTS: u32 = 8;
/// Most notifications kept while a connection is still finishing its handshake.
const HELD_NOTIFICATIONS_MAX: usize = 4096;

enum Fail {
    /// `stop()` ended the attempt.
    Cancelled,
    /// The attempt failed. `direct` says the direct path is to blame, so the tunnel is worth a try.
    Failed {
        failure: ConnectionFailure,
        direct: bool,
    },
}

fn failed(kind: FailureKind, detail: Option<&str>, auto: bool) -> Fail {
    Fail::Failed {
        failure: ConnectionFailure::new(kind, detail.map(str::to_owned), auto),
        direct: false,
    }
}

fn failed_direct(kind: FailureKind, detail: &str, auto: bool) -> Fail {
    Fail::Failed {
        failure: ConnectionFailure::new(kind, Some(detail.to_owned()), auto),
        direct: true,
    }
}

fn network_failure(error: &RtdbError) -> Fail {
    // 401 and 403 mean the security rules refuse this account; asking again will not change that.
    failed(
        FailureKind::Network,
        Some(&if error.status == 0 {
            error.message.clone()
        } else {
            error.status.to_string()
        }),
        error.status != 401 && error.status != 403,
    )
}

fn verification_failure(reason: VerifyFailure, detail: Option<&str>) -> Fail {
    let text = match detail {
        None => reason.as_str().to_owned(),
        Some(detail) => format!("{}: {detail}", reason.as_str()),
    };
    failed(FailureKind::Verification, Some(&text), false)
}

/// What a signed `rejected` answer means for the user.
fn rejection_failure(reason: Option<AnswerRejectReason>) -> ConnectionFailure {
    match reason {
        Some(AnswerRejectReason::Revoked) => {
            ConnectionFailure::new(FailureKind::Revoked, None, false)
        }
        Some(AnswerRejectReason::Disabled) => {
            ConnectionFailure::new(FailureKind::Disabled, None, false)
        }
        Some(AnswerRejectReason::UpgradeRequired) => {
            ConnectionFailure::new(FailureKind::UpgradeRequired, None, false)
        }
        Some(AnswerRejectReason::Expired) => {
            ConnectionFailure::new(FailureKind::Expired, None, false)
        }
        Some(AnswerRejectReason::Capacity) => {
            ConnectionFailure::new(FailureKind::Capacity, None, true)
        }
        Some(AnswerRejectReason::Busy) => ConnectionFailure::new(FailureKind::Busy, None, true),
        Some(other) => {
            ConnectionFailure::new(FailureKind::Rejected, Some(other.as_str().into()), false)
        }
        None => ConnectionFailure::new(FailureKind::Rejected, None, false),
    }
}

type Approval = Result<RemoteHostInfo, ConnectionFailure>;

#[derive(Default)]
struct AttemptData {
    conn_id: String,
    device_id: String,
    offer_written: bool,
    kind: Option<TransportKind>,
    peer: Option<Arc<dyn PeerHandle>>,
    direct: Option<Arc<DirectLink>>,
    files: Option<Arc<crate::files::FileLane>>,
    tunnel: Option<TunnelLink>,
    rpc: Option<RpcPeer>,
    /// Set once `initialize` succeeded and, if needed, the user approved.
    ready: bool,
    /// Set once the connection was published as live: notifications flow directly.
    published: bool,
    close_reason: Option<String>,
    disposed: bool,
    /// Why the desktop is ending an approved connection (`auth.denied` after approval).
    notice: Option<String>,
    /// The desktop asked for the user's approval; nothing it says counts until then.
    awaiting: bool,
    /// Notifications that arrived after the handshake answer but before the connection
    /// was published (the read loop can be a message ahead of the task that finishes the
    /// handshake).
    held: Vec<(String, Value)>,
    approval_tx: Option<oneshot::Sender<Approval>>,
}

/// The resources of one attempt, so that a failure or `stop()` can release all of them.
struct Attempt {
    cancel: CancellationToken,
    data: Mutex<AttemptData>,
}

impl Attempt {
    fn kind(&self) -> TransportKind {
        self.data
            .lock()
            .expect("attempt")
            .kind
            .unwrap_or(TransportKind::Tunnel)
    }
}

enum AttemptResult {
    Ready(Arc<Attempt>, RemoteHostInfo),
    Failed {
        failure: ConnectionFailure,
        direct: bool,
    },
    Cancelled,
}

struct Core {
    current: ConnectionState,
    generation: u64,
    dialing: Option<Arc<Attempt>>,
    live: Option<Arc<Attempt>>,
    failures: u32,
    ready_at: Option<Instant>,
    retry: Option<JoinHandle<()>>,
}

enum Delivery {
    State(ConnectionState),
    Ready,
    Notification(String, Value),
}

type Listeners = Mutex<Vec<(u64, Arc<dyn ConnectionListener>)>>;

struct Inner {
    deps: ViewerConnectionDeps,
    tunnel_memory: Arc<TunnelMemory>,
    core: Mutex<Core>,
    listeners: Arc<Listeners>,
    next_listener: AtomicU64,
    delivery: mpsc::UnboundedSender<Delivery>,
    state_tx: watch::Sender<ConnectionState>,
    repairing: tokio::sync::Mutex<()>,
}

/// The viewer's connection to one computer. Cheap to clone.
#[derive(Clone)]
pub struct ViewerConnection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ViewerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerConnection")
            .field("host_id", &self.inner.deps.host_id)
            .finish_non_exhaustive()
    }
}

struct AttemptNotifications {
    inner: std::sync::Weak<Inner>,
    attempt: Arc<Attempt>,
}

impl NotificationHandler for AttemptNotifications {
    fn handle(&self, method: &str, params: Value) {
        if let Some(inner) = self.inner.upgrade() {
            inner.receive(&self.attempt, method, params);
        }
    }
}

struct AttemptRequests {
    inner: std::sync::Weak<Inner>,
    attempt: Arc<Attempt>,
}

#[async_trait::async_trait]
impl RequestHandler for AttemptRequests {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if !matches!(method, MEDIA_OFFER_METHOD | MEDIA_CONTROL_METHOD) {
            return Err(RpcError::new(
                RemoteErrorCode::InvalidParams,
                "unsupported host request",
            ));
        }
        let Some(inner) = self.inner.upgrade() else {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        };
        match method {
            MEDIA_OFFER_METHOD => inner.accept_media_offer(&self.attempt, params).await,
            MEDIA_CONTROL_METHOD => inner.control_media(&self.attempt, params).await,
            _ => Err(RpcError::new(
                RemoteErrorCode::InvalidParams,
                "unsupported host request",
            )),
        }
    }
}

struct AttemptMediaControl {
    inner: std::sync::Weak<Inner>,
    attempt: std::sync::Weak<Attempt>,
}

#[async_trait::async_trait]
impl RemoteMediaControlChannel for AttemptMediaControl {
    async fn playback_receipt(&self, receipt: RemoteMediaPlaybackReceipt) -> Result<(), RpcError> {
        receipt.validate()?;
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| RpcError::code(RemoteErrorCode::Closed))?;
        let attempt = self
            .attempt
            .upgrade()
            .ok_or_else(|| RpcError::code(RemoteErrorCode::Closed))?;
        let rpc = {
            let data = attempt.data.lock().expect("attempt");
            if !data.ready || data.disposed {
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            data.rpc
                .clone()
                .ok_or_else(|| RpcError::code(RemoteErrorCode::Closed))?
        };
        if inner
            .core
            .lock()
            .expect("core")
            .live
            .as_ref()
            .is_none_or(|live| !Arc::ptr_eq(live, &attempt))
        {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        let params = serde_json::to_value(receipt).map_err(|_| {
            RpcError::new(
                RemoteErrorCode::Internal,
                "media receipt could not be encoded",
            )
        })?;
        rpc.notify(MEDIA_PLAYBACK_RECEIPT_METHOD, Some(params))
            .await
            .map_err(|error| RpcError::new(RemoteErrorCode::Closed, error.to_string()))
    }
}

impl ViewerConnection {
    /// Create a connection (not started). Must be called inside a Tokio runtime.
    pub fn new(deps: ViewerConnectionDeps) -> Self {
        let tunnel_memory = deps
            .tunnel_memory
            .clone()
            .unwrap_or_else(|| Arc::new(TunnelMemory::new(Arc::clone(&deps.clock))));
        let initial = ConnectionState {
            phase: ConnectionPhase::Idle,
            host_id: deps.host_id.clone(),
            transport: None,
            pairing_code: None,
            failure: None,
            attempt: 0,
            retry_at: None,
            host: None,
            ever_ready: false,
        };
        let (state_tx, _) = watch::channel(initial.clone());
        let (delivery, mut receiver) = mpsc::unbounded_channel();
        let listeners: Arc<Listeners> = Arc::default();
        let hearers = Arc::clone(&listeners);
        tokio::spawn(async move {
            while let Some(item) = receiver.recv().await {
                let snapshot: Vec<Arc<dyn ConnectionListener>> = hearers
                    .lock()
                    .expect("listeners")
                    .iter()
                    .map(|(_, l)| Arc::clone(l))
                    .collect();
                for listener in snapshot {
                    match &item {
                        Delivery::State(state) => listener.on_state(state),
                        Delivery::Ready => listener.on_ready(),
                        Delivery::Notification(method, params) => {
                            listener.on_notification(method, params);
                        }
                    }
                }
            }
        });
        Self {
            inner: Arc::new(Inner {
                deps,
                tunnel_memory,
                core: Mutex::new(Core {
                    current: initial,
                    generation: 0,
                    dialing: None,
                    live: None,
                    failures: 0,
                    ready_at: None,
                    retry: None,
                }),
                listeners,
                next_listener: AtomicU64::new(1),
                delivery,
                state_tx,
                repairing: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The current state.
    pub fn state(&self) -> ConnectionState {
        self.inner.core.lock().expect("core").current.clone()
    }

    /// A receiver of the latest state (for `await`-style consumers).
    pub fn watch_state(&self) -> watch::Receiver<ConnectionState> {
        self.inner.state_tx.subscribe()
    }

    /// The computer being dialled.
    pub fn host_id(&self) -> &str {
        &self.inner.deps.host_id
    }

    /// Register a listener. Listeners are called in order from one task.
    pub fn add_listener(&self, listener: Arc<dyn ConnectionListener>) -> ListenerId {
        let id = self.inner.next_listener.fetch_add(1, Ordering::SeqCst);
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .push((id, listener));
        ListenerId(id)
    }

    /// Unregister a listener.
    pub fn remove_listener(&self, id: ListenerId) {
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .retain(|(existing, _)| *existing != id.0);
    }

    /// Independent binary file transport, available only on an approved direct connection.
    pub fn file_transport(&self) -> Result<(Arc<crate::files::FileLane>, RpcPeer), String> {
        let live = self
            .inner
            .core
            .lock()
            .expect("core")
            .live
            .clone()
            .ok_or("not connected")?;
        let data = live.data.lock().expect("attempt");
        if !data.ready || data.kind != Some(TransportKind::Webrtc) {
            return Err("files require WebRTC".into());
        }
        Ok((
            data.files.clone().ok_or("file channel unavailable")?,
            data.rpc.clone().ok_or("control channel unavailable")?,
        ))
    }

    /// Send a request on the ready link. Fails with `closed` when there is none.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        match self.inner.live_rpc() {
            Some(rpc) if !rpc.is_closed() => rpc.request(method, params, timeout).await,
            _ => Err(RpcError::new(RemoteErrorCode::Closed, "not connected")),
        }
    }

    async fn local_app_request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, RpcError> {
        let attempt = self.inner.core.lock().expect("core").live.clone()
            .ok_or_else(|| RpcError::code(RemoteErrorCode::Closed))?;
        let rpc = {
            let data = attempt.data.lock().expect("attempt");
            if !data.ready || data.disposed { return Err(RpcError::code(RemoteErrorCode::Closed)); }
            data.rpc.clone().ok_or_else(|| RpcError::code(RemoteErrorCode::Closed))?
        };
        let value = rpc.request(method, Some(params), timeout).await?;
        let core = self.inner.core.lock().expect("core");
        if core.live.as_ref().is_none_or(|live| !Arc::ptr_eq(live, &attempt)) {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        Ok(value)
    }

    /// List Local Apps explicitly shared by this desktop.
    pub async fn list_apps(&self) -> Result<Vec<crate::local_apps::RemoteLocalApp>, RpcError> {
        let value = self.local_app_request("apps.list", serde_json::json!({}), None).await?;
        crate::local_apps::parse_list(value).ok_or_else(|| RpcError::new(RemoteErrorCode::Internal, "invalid apps.list result"))
    }

    /// Obtain the single-use browser bootstrap for a shared Local App.
    pub async fn open_app(&self, app_id: &str) -> Result<crate::local_apps::RemoteLocalAppOpen, RpcError> {
        if !crate::local_apps::valid_id(app_id) {
            return Err(RpcError::new(RemoteErrorCode::InvalidParams, "invalid app id"));
        }
        let value = self.local_app_request("apps.open", serde_json::json!({"app_id": app_id}), Some(Duration::from_secs(930))).await?;
        crate::local_apps::parse_open(value, app_id).ok_or_else(|| RpcError::new(RemoteErrorCode::Internal, "invalid apps.open result"))
    }

    /// Send a notification on the ready link; dropped when there is none.
    pub async fn notify(&self, method: &str, params: Option<Value>) {
        if let Some(rpc) = self.inner.live_rpc() {
            let _ = rpc.notify(method, params).await;
        }
    }

    /// Start dialling. Does nothing while already connecting, connected or waiting to reconnect.
    pub fn start(&self) {
        if self.state().phase == ConnectionPhase::Idle {
            self.inner.begin(false);
        }
    }

    /// Try again after a failure, at the user's request.
    pub fn retry(&self) {
        let (phase, ever_ready) = {
            let state = self.state();
            (state.phase, state.ever_ready)
        };
        if phase != ConnectionPhase::Failed && phase != ConnectionPhase::Idle {
            return;
        }
        self.inner.core.lock().expect("core").failures = 0;
        self.inner.begin(ever_ready);
    }

    /// Called when the app returns to the foreground or the network comes back: skip
    /// the rest of a backoff, and revive a connection that gave up on its own.
    pub fn nudge(&self) {
        let state = self.state();
        let waiting = self.inner.core.lock().expect("core").retry.is_some();
        if state.phase == ConnectionPhase::Reconnecting && waiting {
            self.inner.begin(true);
        } else if state.phase == ConnectionPhase::Failed
            && state.failure.as_ref().is_some_and(|f| f.auto)
        {
            self.inner.core.lock().expect("core").failures = 0;
            self.inner.begin(state.ever_ready);
        }
    }

    /// After `revoked`: the old device key is useless, so make a new one and ask again.
    pub async fn repair(&self) {
        let Ok(_repair) = self.inner.repairing.try_lock() else {
            return;
        };
        let (generation, reconnecting) = {
            let core = self.inner.core.lock().expect("core");
            if core.current.phase != ConnectionPhase::Failed
                || core.current.failure.as_ref().map(|failure| failure.kind)
                    != Some(FailureKind::Revoked)
            {
                return;
            }
            (core.generation, core.current.ever_ready)
        };
        if self.inner.deps.keys.reset_identity().await.is_ok() {
            self.inner
                .begin_if_generation(reconnecting, Some(generation));
        }
    }

    /// End everything: cancel the dial, close the link, clean up the database.
    pub fn stop(&self) {
        let (dialing, live) = {
            let mut core = self.inner.core.lock().expect("core");
            core.generation += 1;
            if let Some(retry) = core.retry.take() {
                retry.abort();
            }
            (core.dialing.take(), core.live.take())
        };
        if let Some(dialing) = dialing {
            self.inner.dispose(&dialing, "cancelled");
        }
        if let Some(live) = live {
            self.inner.dispose(&live, "cancelled");
        }
        if self.state().phase != ConnectionPhase::Idle {
            self.inner.update(|state| {
                state.phase = ConnectionPhase::Idle;
                state.pairing_code = None;
                state.failure = None;
                state.retry_at = None;
            });
        }
    }
}

impl Inner {
    fn live_rpc(&self) -> Option<RpcPeer> {
        let live = self.core.lock().expect("core").live.clone()?;
        let rpc = live.data.lock().expect("attempt").rpc.clone();
        rpc
    }

    async fn accept_media_offer(
        self: &Arc<Self>,
        attempt: &Arc<Attempt>,
        params: Value,
    ) -> Result<Value, RpcError> {
        let invalid = || RpcError::new(RemoteErrorCode::InvalidParams, "invalid media offer");
        let offer: RemoteMediaOffer = serde_json::from_value(params).map_err(|_| invalid())?;
        offer.validate()?;

        let (handler, connection) = {
            let data = attempt.data.lock().expect("attempt");
            if !data.ready || data.disposed || data.conn_id.is_empty() || data.device_id.is_empty()
            {
                return Err(RpcError::code(RemoteErrorCode::NotApproved));
            }
            let handler =
                self.deps.media_offer_handler.clone().ok_or_else(|| {
                    RpcError::new(RemoteErrorCode::Forbidden, "media is unavailable")
                })?;
            let connection = RemoteMediaConnection {
                account_uid: self.deps.uid.clone(),
                host_id: self.deps.host_id.clone(),
                device_id: data.device_id.clone(),
                connection_id: data.conn_id.clone(),
            };
            (handler, connection)
        };

        let control: Arc<dyn RemoteMediaControlChannel> = Arc::new(AttemptMediaControl {
            inner: Arc::downgrade(self),
            attempt: Arc::downgrade(attempt),
        });
        let answer: RemoteMediaAnswer = handler
            .accept_offer(&connection, offer.clone(), control)
            .await?;
        answer.validate_for(&offer)?;

        // A close or revocation can race the native permission prompt or peer
        // negotiation. Do not return an answer for an ended authenticated owner.
        let still_approved = {
            let data = attempt.data.lock().expect("attempt");
            data.ready && !data.disposed && data.conn_id == connection.connection_id
        };
        if !still_approved {
            handler.connection_closed(&connection).await;
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }

        serde_json::to_value(answer).map_err(|_| {
            RpcError::new(
                RemoteErrorCode::Internal,
                "media answer could not be encoded",
            )
        })
    }

    async fn control_media(
        self: &Arc<Self>,
        attempt: &Arc<Attempt>,
        params: Value,
    ) -> Result<Value, RpcError> {
        let invalid = || {
            RpcError::new(
                RemoteErrorCode::InvalidParams,
                "invalid media control request",
            )
        };
        let request: RemoteMediaControlRequest =
            serde_json::from_value(params).map_err(|_| invalid())?;
        request.validate()?;
        let (handler, connection) = {
            let data = attempt.data.lock().expect("attempt");
            if !data.ready || data.disposed || data.conn_id.is_empty() || data.device_id.is_empty()
            {
                return Err(RpcError::code(RemoteErrorCode::NotApproved));
            }
            let handler =
                self.deps.media_offer_handler.clone().ok_or_else(|| {
                    RpcError::new(RemoteErrorCode::Forbidden, "media is unavailable")
                })?;
            let connection = RemoteMediaConnection {
                account_uid: self.deps.uid.clone(),
                host_id: self.deps.host_id.clone(),
                device_id: data.device_id.clone(),
                connection_id: data.conn_id.clone(),
            };
            (handler, connection)
        };
        let result = handler.control(&connection, request.clone()).await?;
        result.validate_for(&request)?;
        let still_approved = {
            let data = attempt.data.lock().expect("attempt");
            data.ready && !data.disposed && data.conn_id == connection.connection_id
        };
        if !still_approved {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        serde_json::to_value(result).map_err(|_| {
            RpcError::new(
                RemoteErrorCode::Internal,
                "media control result could not be encoded",
            )
        })
    }

    fn ever_ready(&self) -> bool {
        self.core.lock().expect("core").current.ever_ready
    }

    fn update(&self, change: impl FnOnce(&mut ConnectionState)) {
        let snapshot = {
            let mut core = self.core.lock().expect("core");
            change(&mut core.current);
            core.current.attempt = core.failures;
            core.current.clone()
        };
        self.state_tx.send_replace(snapshot.clone());
        let _ = self.delivery.send(Delivery::State(snapshot));
    }

    fn begin(self: &Arc<Self>, reconnecting: bool) {
        self.begin_if_generation(reconnecting, None);
    }

    fn begin_if_generation(self: &Arc<Self>, reconnecting: bool, expected: Option<u64>) {
        let generation = {
            let mut core = self.core.lock().expect("core");
            if expected.is_some_and(|generation| generation != core.generation) {
                return;
            }
            if let Some(retry) = core.retry.take() {
                retry.abort();
            }
            core.generation += 1;
            if expected.is_some() {
                core.failures = 0;
            }
            core.current.phase = if reconnecting {
                ConnectionPhase::Reconnecting
            } else {
                ConnectionPhase::Connecting
            };
            core.current.pairing_code = None;
            core.current.retry_at = None;
            core.current.attempt = core.failures;
            let snapshot = core.current.clone();
            self.state_tx.send_replace(snapshot.clone());
            let _ = self.delivery.send(Delivery::State(snapshot));
            core.generation
        };
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.dial(generation).await });
    }

    fn fail(self: &Arc<Self>, failure: ConnectionFailure) {
        let failures = {
            let mut core = self.core.lock().expect("core");
            core.failures += 1;
            core.failures
        };
        if failure.auto && failures <= MAX_AUTO_ATTEMPTS {
            let delay = backoff_ms(failures, self.deps.random.as_ref());
            let retry_at = self.deps.clock.now_ms() + delay;
            self.update(|state| {
                state.phase = ConnectionPhase::Reconnecting;
                state.failure = Some(failure);
                state.pairing_code = None;
                state.retry_at = Some(retry_at);
            });
            let inner = Arc::clone(self);
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                // Detach before `begin`, which would otherwise abort this very task.
                drop(inner.core.lock().expect("core").retry.take());
                inner.begin(true);
            });
            self.core.lock().expect("core").retry = Some(handle);
        } else {
            self.update(|state| {
                state.phase = ConnectionPhase::Failed;
                state.failure = Some(failure);
                state.pairing_code = None;
                state.retry_at = None;
            });
        }
    }

    fn current_generation(&self) -> u64 {
        self.core.lock().expect("core").generation
    }

    // -----------------------------------------------------------------------
    // Dialling
    // -----------------------------------------------------------------------

    async fn dial(self: Arc<Self>, generation: u64) {
        let host_id = self.deps.host_id.clone();
        let skip_direct = self.deps.force_tunnel
            || self.deps.peers.is_none()
            || self.tunnel_memory.prefers(&host_id);
        let first = if skip_direct {
            TransportKind::Tunnel
        } else {
            TransportKind::Webrtc
        };
        let mut result = self.attempt(generation, first).await;
        if matches!(result, AttemptResult::Failed { direct: true, .. })
            && generation == self.current_generation()
        {
            // The direct path did not work: remember that, and dial again through
            // the tunnel on a new connection id.
            self.tunnel_memory.remember(&host_id);
            result = self.attempt(generation, TransportKind::Tunnel).await;
        }
        if generation != self.current_generation() {
            if let AttemptResult::Ready(attempt, _) = result {
                self.dispose(&attempt, "cancelled");
            }
            return;
        }
        match result {
            AttemptResult::Cancelled => {}
            AttemptResult::Failed { failure, .. } => self.fail(failure),
            AttemptResult::Ready(attempt, host) => self.publish(generation, attempt, host),
        }
    }

    fn publish(self: &Arc<Self>, generation: u64, attempt: Arc<Attempt>, host: RemoteHostInfo) {
        let kind = attempt.kind();
        let mut core = self.core.lock().expect("core");
        if generation != core.generation || attempt.cancel.is_cancelled() {
            drop(core);
            self.dispose(&attempt, "cancelled");
            return;
        }
        core.live = Some(Arc::clone(&attempt));
        core.ready_at = Some(Instant::now());
        // Held notifications go out first, in order, before anything newer.
        {
            let mut data = attempt.data.lock().expect("attempt");
            if let Some(reason) = data.rpc.as_ref().and_then(RpcPeer::closed_reason) {
                drop(data);
                drop(core);
                self.link_closed(&attempt, &reason);
                return;
            }
            data.published = true;
            let held = std::mem::take(&mut data.held);
            core.current.phase = ConnectionPhase::Ready;
            core.current.transport = Some(kind);
            core.current.host = Some(host);
            core.current.pairing_code = None;
            core.current.failure = None;
            core.current.retry_at = None;
            core.current.ever_ready = true;
            core.current.attempt = core.failures;
            let snapshot = core.current.clone();
            self.state_tx.send_replace(snapshot.clone());
            let _ = self.delivery.send(Delivery::State(snapshot));
            for (method, params) in held {
                let _ = self.delivery.send(Delivery::Notification(method, params));
            }
            let _ = self.delivery.send(Delivery::Ready);
        }
    }

    /// One dial: write an offer, get the answer, bring the link up, handshake.
    async fn attempt(self: &Arc<Self>, generation: u64, requested: TransportKind) -> AttemptResult {
        let attempt = Arc::new(Attempt {
            cancel: CancellationToken::new(),
            data: Mutex::new(AttemptData::default()),
        });
        {
            let mut core = self.core.lock().expect("core");
            if core.generation != generation {
                return AttemptResult::Cancelled;
            }
            core.dialing = Some(Arc::clone(&attempt));
        }
        let outcome = self.run_attempt(&attempt, requested).await;
        let current = self.current_generation();
        match outcome {
            Ok(host) => {
                if generation != current || attempt.cancel.is_cancelled() {
                    self.dispose(&attempt, "cancelled");
                    return AttemptResult::Cancelled;
                }
                self.core.lock().expect("core").dialing = None;
                AttemptResult::Ready(attempt, host)
            }
            Err(error) => {
                let reason = match &error {
                    Fail::Failed { failure, .. } => failure.kind.as_str(),
                    Fail::Cancelled => "failed",
                };
                self.dispose(&attempt, reason);
                {
                    let mut core = self.core.lock().expect("core");
                    if core
                        .dialing
                        .as_ref()
                        .is_some_and(|d| Arc::ptr_eq(d, &attempt))
                    {
                        core.dialing = None;
                    }
                }
                match error {
                    Fail::Cancelled => AttemptResult::Cancelled,
                    _ if generation != current => AttemptResult::Cancelled,
                    Fail::Failed { failure, direct } => AttemptResult::Failed { failure, direct },
                }
            }
        }
    }

    async fn raced<T>(
        &self,
        attempt: &Attempt,
        future: impl std::future::Future<Output = T>,
    ) -> Result<T, Fail> {
        tokio::select! {
            () = attempt.cancel.cancelled() => Err(Fail::Cancelled),
            value = future => Ok(value),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_attempt(
        self: &Arc<Self>,
        a: &Arc<Attempt>,
        requested: TransportKind,
    ) -> Result<RemoteHostInfo, Fail> {
        let deps = &self.deps;
        let identity: DeviceIdentity = match self.raced(a, deps.keys.identity()).await? {
            Ok(identity) => identity,
            Err(error) => return Err(failed(FailureKind::Unsupported, Some(&error.0), false)),
        };
        a.data.lock().expect("attempt").device_id = identity.id.clone();
        let pin = self.raced(a, deps.keys.pinned_host(&deps.host_id)).await?;
        let mut eph = Some(EphemeralKey::generate());
        let expected_host_key = match &pin {
            Some(pin) => pin.host_key.clone(),
            None => self.advertised_host_key(a).await?,
        };

        let conn_id = random_id();
        a.data.lock().expect("attempt").conn_id = conn_id.clone();
        let nonce = b64u_encode(&random_bytes::<16>());

        // A direct offer carries the complete SDP; if one cannot be made, the offer is
        // simply a tunnel offer.
        let mut offered = requested;
        let mut sdp: Option<String> = None;
        let mut direct_parts: Option<(Arc<DirectLink>, mpsc::UnboundedReceiver<LinkEvent>)> = None;
        if requested == TransportKind::Webrtc {
            match self.prepare_direct(a).await? {
                Some((offer_sdp, parts)) => {
                    sdp = Some(offer_sdp);
                    direct_parts = Some(parts);
                }
                None => {
                    offered = TransportKind::Tunnel;
                    self.tunnel_memory.remember(&deps.host_id);
                }
            }
        }
        a.data.lock().expect("attempt").kind = Some(offered);

        let draft = OfferDraft {
            v: 1,
            transport: offered,
            sdp,
            device: OfferDevice {
                id: identity.id.clone(),
                key: b64u_encode(&identity.public_key),
                name: display_text(&deps.device_name, DEVICE_NAME_MAX_CHARS),
                platform: deps.platform,
                app_version: {
                    let version = display_text(&deps.app_version, 64);
                    if version.is_empty() {
                        "dev".into()
                    } else {
                        version
                    }
                },
            },
            eph: b64u_encode(&eph.as_ref().expect("ephemeral key").public_key()),
            nonce: nonce.clone(),
        };
        let ctx = SigningContext {
            uid: deps.uid.clone(),
            host_id: deps.host_id.clone(),
            conn_id: conn_id.clone(),
        };
        let offer = match self
            .raced(a, sign_offer(&ctx, identity.signer.as_ref(), draft.clone()))
            .await?
        {
            Ok(offer) => offer,
            Err(error) => {
                return Err(failed(
                    FailureKind::Unsupported,
                    Some(&error.to_string()),
                    false,
                ))
            }
        };

        // Listen before writing, so that even an instant refusal cannot be missed.
        let answers_path = to_viewer_conn_path(&deps.uid, &deps.host_id, &conn_id)
            .map_err(|e| failed(FailureKind::Protocol, Some(&e.0), false))?;
        let mut answers = deps.rtdb.listen(&answers_path);
        let offer_at = offer_path(&deps.uid, &deps.host_id, &conn_id)
            .map_err(|e| failed(FailureKind::Protocol, Some(&e.0), false))?;
        let written = self
            .raced(a, deps.rtdb.put(&offer_at, &offer.to_value()))
            .await?;
        // It may have landed even though the reply did not.
        a.data.lock().expect("attempt").offer_written = true;
        if let Err(error) = written {
            return Err(network_failure(&error));
        }

        let answer = self
            .wait_for_answer(a, &mut answers, &ctx, &nonce, &expected_host_key)
            .await?;
        drop(answers);
        if let Ok(path) = answer_path(&deps.uid, &deps.host_id, &conn_id) {
            let rtdb = Arc::clone(&deps.rtdb);
            tokio::spawn(async move {
                let _ = rtdb.remove(&path).await;
            });
        }

        if answer.draft.status == AnswerStatus::Rejected {
            let failure = rejection_failure(answer.draft.reason);
            // The desktop vouched for the refusal with its own signature, so a revoked
            // pairing really is over.
            if failure.kind == FailureKind::Revoked {
                deps.keys.unpair(&deps.host_id).await;
            }
            return Err(Fail::Failed {
                failure,
                direct: false,
            });
        }
        let chosen = match answer.draft.transport {
            Some(chosen)
                if !(offered == TransportKind::Tunnel && chosen != TransportKind::Tunnel) =>
            {
                chosen
            }
            _ => {
                return Err(failed(
                    FailureKind::Protocol,
                    Some("answer transport does not fit the offer"),
                    false,
                ))
            }
        };

        // Both sides compute the code from the same records; it is only ever shown from
        // our own computation.
        let pairing_code = pairing_code_for(
            &draft.device.key,
            &draft.nonce,
            &answer.draft.host.key,
            &answer.draft.nonce,
        )
        .map_err(|e| failed(FailureKind::Protocol, Some(&e.0), false))?;

        let link: Link = if chosen == TransportKind::Webrtc {
            let parts = direct_parts.take();
            self.open_direct(a, &answer, parts).await?
        } else {
            // The desktop chose the tunnel for a direct offer: same connection id, no new offer.
            let peer = {
                let mut data = a.data.lock().expect("attempt");
                let had = data.peer.take();
                let direct = data.direct.take();
                if let Some(direct) = &direct {
                    use crate::transport::FragmentLink as _;
                    direct.close("downgraded");
                }
                had
            };
            if let Some(peer) = peer {
                self.tunnel_memory.remember(&deps.host_id);
                peer.close();
            }
            drop(direct_parts.take());
            self.open_tunnel(a, eph.take().expect("ephemeral key"), &answer, &conn_id)?
        };
        a.data.lock().expect("attempt").kind = Some(chosen);

        let rpc = RpcPeer::start(
            link,
            RpcOptions {
                timeout: None,
                // Only the offer negotiation and bounded audio flush/interrupt
                // control are allowed after approval. No generic host command
                // surface is exposed to the remote device.
                requests: Some(Arc::new(AttemptRequests {
                    inner: Arc::downgrade(self),
                    attempt: Arc::clone(a),
                })),
                notifications: Some(Arc::new(AttemptNotifications {
                    inner: Arc::downgrade(self),
                    attempt: Arc::clone(a),
                })),
            },
        );
        a.data.lock().expect("attempt").rpc = Some(rpc.clone());
        {
            let inner = Arc::clone(self);
            let attempt = Arc::clone(a);
            let watcher = rpc.clone();
            tokio::spawn(async move {
                let reason = watcher.closed().await;
                inner.link_closed(&attempt, &reason);
            });
        }
        rpc.start_keepalive(if chosen == TransportKind::Webrtc {
            KeepaliveKind::DataChannel
        } else {
            KeepaliveKind::Tunnel
        });

        self.handshake(a, &rpc, chosen, &pairing_code, &expected_host_key, pin)
            .await
    }

    /// `hosts/{host_id}.host_key`, the key a first pairing expects the answer to be signed with.
    async fn advertised_host_key(&self, a: &Attempt) -> Result<String, Fail> {
        let deps = &self.deps;
        let path = host_path(&deps.uid, &deps.host_id)
            .map_err(|e| failed(FailureKind::UnknownHost, Some(&e.0), false))?;
        let raw = match self.raced(a, deps.rtdb.get(&path)).await? {
            Ok(raw) => raw,
            Err(error) => return Err(network_failure(&error)),
        };
        match parse_host_record(&raw) {
            Ok(record) => Ok(record.host_key),
            Err(detail) => Err(failed(FailureKind::UnknownHost, Some(&detail), false)),
        }
    }

    /// Create the connection, the DataChannel and the complete offer SDP. Returns
    /// `None` when none can be made (no WebRTC, no usable SDP), which sends the
    /// caller to the tunnel without a failed attempt.
    async fn prepare_direct(
        &self,
        a: &Attempt,
    ) -> Result<
        Option<(
            String,
            (Arc<DirectLink>, mpsc::UnboundedReceiver<LinkEvent>),
        )>,
        Fail,
    > {
        let Some(connector) = &self.deps.peers else {
            return Ok(None);
        };
        let config = PeerConfig {
            stun_urls: self.deps.stun_urls.clone(),
            channel_label: DATA_CHANNEL_LABEL.to_owned(),
            gather_timeout: ICE_GATHER_TIMEOUT,
        };
        let Ok(offerer) = self.raced(a, connector.create_offerer(config)).await? else {
            return Ok(None);
        };
        let peer = offerer.peer;
        if let Some(files) = peer.take_file_channel() {
            a.data.lock().expect("attempt").files = Some(crate::files::FileLane::start(files));
        }
        {
            a.data.lock().expect("attempt").peer = Some(Arc::clone(&peer));
        }
        let bound = ICE_GATHER_TIMEOUT + Duration::from_secs(5);
        let sdp = match self
            .raced(a, tokio::time::timeout(bound, peer.offer_sdp()))
            .await
        {
            Ok(Ok(Ok(sdp))) => sdp,
            Err(cancelled) => {
                peer.close();
                return Err(cancelled);
            }
            Ok(_) => {
                peer.close();
                a.data.lock().expect("attempt").peer = None;
                return Ok(None);
            }
        };
        if sdp.is_empty() || sdp.encode_utf16().count() > SDP_MAX_CHARS {
            peer.close();
            a.data.lock().expect("attempt").peer = None;
            return Ok(None);
        }
        let (link, events) = DirectLink::start(Arc::clone(&peer), offerer.events);
        a.data.lock().expect("attempt").direct = Some(Arc::clone(&link));
        Ok(Some((sdp, (link, events))))
    }

    /// Resolve with the verified answer once it appears under `to_viewer/{host}/{conn}/answer`.
    async fn wait_for_answer(
        &self,
        a: &Attempt,
        listener: &mut Listener,
        ctx: &SigningContext,
        offer_nonce: &str,
        expected_host_key: &str,
    ) -> Result<AnswerRecord, Fail> {
        let ever_ready = self.ever_ready();
        let wait = async {
            let mut mirror = RtdbMirror::new();
            loop {
                match listener.recv().await {
                    None => return Err(failed(FailureKind::Network, Some("stream_closed"), true)),
                    Some(ListenItem::Error(error)) => {
                        return Err(match error {
                            RtdbListenError::Http { status, .. } => failed(
                                FailureKind::Network,
                                Some(&status.to_string()),
                                status != 401 && status != 403,
                            ),
                            RtdbListenError::Cancel => {
                                failed(FailureKind::Network, Some("cancel"), true)
                            }
                        })
                    }
                    Some(ListenItem::Event(event)) => {
                        for change in mirror.apply(&event).at(1) {
                            if change.path[0] != "answer" || change.kind == ChangeKind::Removed {
                                continue;
                            }
                            let options = VerifyAnswerOptions {
                                ctx,
                                record: &change.value,
                                offer_nonce,
                                expected_host_key,
                                now_ms: self.deps.clock.now_ms(),
                                max_age_ms: None,
                            };
                            return verify_answer(&options)
                                .map_err(|e| verification_failure(e.reason, e.detail.as_deref()));
                        }
                    }
                }
            }
        };
        match self
            .raced(a, tokio::time::timeout(OFFER_ANSWER_TIMEOUT, wait))
            .await?
        {
            Ok(result) => result,
            Err(_elapsed) => Err(failed(FailureKind::NoAnswer, None, ever_ready)),
        }
    }

    /// Finish the direct connection: apply the answer and wait for the DataChannel to open.
    async fn open_direct(
        &self,
        a: &Attempt,
        answer: &AnswerRecord,
        parts: Option<(Arc<DirectLink>, mpsc::UnboundedReceiver<LinkEvent>)>,
    ) -> Result<Link, Fail> {
        let peer = a.data.lock().expect("attempt").peer.clone();
        let (Some(peer), Some((link, events)), Some(sdp)) =
            (peer, parts, answer.draft.sdp.as_deref())
        else {
            return Err(failed(
                FailureKind::Protocol,
                Some("direct answer without a direct offer"),
                false,
            ));
        };
        let ever_ready = self.ever_ready();
        // Applying the answer waits for the channel (at most 15 s) in the real session;
        // a link that ends meanwhile (ICE failed) ends the wait at once.
        let apply = async {
            match peer.apply_answer(sdp).await {
                Err(_) => Err("setRemoteDescription".to_owned()),
                Ok(PeerAnswer::TimedOut) => Err("data_channel_timeout".to_owned()),
                Ok(PeerAnswer::Ready) => link.wait_open().await,
            }
        };
        let opened = async {
            tokio::select! {
                result = apply => result,
                Err(reason) = link.wait_open() => Err(reason),
            }
        };
        match self
            .raced(a, tokio::time::timeout(DATA_CHANNEL_OPEN_TIMEOUT, opened))
            .await?
        {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return Err(failed_direct(FailureKind::Closed, &reason, ever_ready)),
            Err(_elapsed) => {
                return Err(failed_direct(
                    FailureKind::Closed,
                    "data_channel_timeout",
                    ever_ready,
                ))
            }
        }
        Ok(Link { link, events })
    }

    /// Finish the tunnel: derive the traffic keys from the two ephemeral keys and
    /// listen for the host's frames.
    fn open_tunnel(
        &self,
        a: &Attempt,
        eph: EphemeralKey,
        answer: &AnswerRecord,
        conn_id: &str,
    ) -> Result<Link, Fail> {
        let deps = &self.deps;
        let protocol = |detail: &str| failed(FailureKind::Protocol, Some(detail), false);
        let Some(host_eph) = answer.draft.eph.as_deref() else {
            return Err(protocol("tunnel answer without a key"));
        };
        let peer_public = b64u_decode(host_eph).map_err(|e| protocol(&e.0))?;
        let shared = eph.agree(&peer_public).map_err(|e| protocol(&e.0))?;
        let keys = derive_tunnel_keys(shared.as_bytes(), conn_id).map_err(|e| protocol(&e.0))?;
        let send_path = up_path(&deps.uid, &deps.host_id, conn_id).map_err(|e| protocol(&e.0))?;
        let recv_path = down_path(&deps.uid, &deps.host_id, conn_id).map_err(|e| protocol(&e.0))?;
        let (tunnel, events) = TunnelLink::new(TunnelLinkOptions {
            rtdb: Arc::clone(&deps.rtdb),
            conn_id: conn_id.to_owned(),
            send_path,
            send_dir: crate::tunnel_crypto::TunnelDir::Up,
            send_key: keys.up,
            recv_path,
            recv_dir: crate::tunnel_crypto::TunnelDir::Down,
            recv_key: keys.down,
            subscribe: true,
        });
        a.data.lock().expect("attempt").tunnel = Some(tunnel.clone());
        Ok(tunnel.into_link(events))
    }

    // -----------------------------------------------------------------------
    // Handshake
    // -----------------------------------------------------------------------

    async fn handshake(
        &self,
        a: &Arc<Attempt>,
        rpc: &RpcPeer,
        chosen: TransportKind,
        pairing_code: &str,
        expected_host_key: &str,
        pin: Option<PinnedHost>,
    ) -> Result<RemoteHostInfo, Fail> {
        let deps = &self.deps;
        let ever_ready = self.ever_ready();
        let (approval_tx, approval_rx) = oneshot::channel();
        a.data.lock().expect("attempt").approval_tx = Some(approval_tx);

        let params = serde_json::to_value(InitializeParams {
            protocol: REMOTE_PROTOCOL_MAJOR,
            client_protocol_version: client_protocol_major(),
            app: RemoteAppInfo {
                version: {
                    let version = display_text(&deps.app_version, 64);
                    if version.is_empty() {
                        "dev".into()
                    } else {
                        version
                    }
                },
                platform: deps.platform,
                media_capabilities: deps
                    .media_offer_handler
                    .as_ref()
                    .and_then(|handler| handler.capabilities())
                    .filter(|capabilities| capabilities.validate().is_ok()),
            },
            push_installation_id: deps.push_installation_id.clone(),
        })
        .expect("initialize params serialize");
        // Before `initialize` has an answer, a dead direct link is the direct path's
        // fault; afterwards it is not.
        let direct = chosen == TransportKind::Webrtc;
        let init = self
            .raced(
                a,
                rpc.request(methods::INITIALIZE, Some(params), Some(HANDSHAKE_TIMEOUT)),
            )
            .await?;
        let init = match init {
            Ok(value) => value,
            Err(error) => {
                let failure = match error.code {
                    RemoteErrorCode::UpgradeRequired => {
                        ConnectionFailure::new(FailureKind::UpgradeRequired, None, false)
                    }
                    RemoteErrorCode::Capacity => {
                        ConnectionFailure::new(FailureKind::Capacity, None, true)
                    }
                    RemoteErrorCode::Timeout => ConnectionFailure::new(
                        FailureKind::Closed,
                        Some("handshake_timeout".into()),
                        ever_ready,
                    ),
                    RemoteErrorCode::Closed => ConnectionFailure::new(
                        FailureKind::Closed,
                        a.data
                            .lock()
                            .expect("attempt")
                            .close_reason
                            .clone()
                            .or(Some(error.message)),
                        ever_ready,
                    ),
                    other => ConnectionFailure::new(
                        FailureKind::Protocol,
                        Some(other.as_str().into()),
                        false,
                    ),
                };
                let direct = direct
                    && matches!(
                        error.code,
                        RemoteErrorCode::Timeout | RemoteErrorCode::Closed
                    );
                return Err(Fail::Failed { failure, direct });
            }
        };
        let Ok(init) = serde_json::from_value::<InitializeResult>(init) else {
            return Err(failed(
                FailureKind::Protocol,
                Some("unknown initialize status"),
                false,
            ));
        };

        let host = match init {
            InitializeResult::Approved { host } => host,
            InitializeResult::PendingApproval {
                pairing_code: theirs,
            } => {
                if theirs != pairing_code {
                    return Err(failed(FailureKind::PairingCodeMismatch, None, false));
                }
                {
                    let mut data = a.data.lock().expect("attempt");
                    data.awaiting = true;
                    data.held.clear();
                }
                let transport = chosen;
                let code = pairing_code.to_owned();
                self.update(|state| {
                    state.phase = ConnectionPhase::AwaitingApproval;
                    state.transport = Some(transport);
                    state.pairing_code = Some(code);
                });
                tokio::select! {
                    // The desktop sends `auth.denied` and then closes, so both can be
                    // ready at once: the decision must win over the close it caused.
                    biased;
                    () = a.cancel.cancelled() => return Err(Fail::Cancelled),
                    outcome = approval_rx => match outcome {
                        Ok(Ok(host)) => host,
                        Ok(Err(failure)) => return Err(Fail::Failed { failure, direct: false }),
                        Err(_) => return Err(Fail::Cancelled),
                    },
                    reason = rpc.closed() => {
                        return Err(failed(FailureKind::Closed, Some(&reason), false));
                    }
                }
            }
        };

        // The computer has accepted this device: remember its key for every later answer.
        let host_name = {
            let name = display_text(&host.name, 64);
            if name.is_empty() {
                deps.host_id.clone()
            } else {
                name
            }
        };
        if pin.as_ref().map(|p| p.name.as_str()) != Some(host_name.as_str()) {
            // Not awaited: notifications may already be arriving, and a slow store must
            // not hold them up.
            let keys = Arc::clone(&deps.keys);
            let record = PinnedHost {
                host_id: deps.host_id.clone(),
                host_key: expected_host_key.to_owned(),
                name: host_name,
                paired_at: pin.map_or_else(|| deps.clock.now_ms(), |p| p.paired_at),
            };
            tokio::spawn(async move { keys.pin(record).await });
        }
        let mut data = a.data.lock().expect("attempt");
        // Approval may arrive before this task consumes the initialize reply.
        // The pending branch must not leave its later `awaiting = true` behind.
        data.awaiting = false;
        data.ready = true;
        Ok(host)
    }

    fn receive(self: &Arc<Self>, a: &Arc<Attempt>, method: &str, params: Value) {
        match method {
            methods::AUTH_APPROVED => {
                let host = serde_json::from_value::<AuthApprovedParams>(params)
                    .ok()
                    .map(|p| p.host);
                let mut data = a.data.lock().expect("attempt");
                data.awaiting = false;
                if let Some(tx) = data.approval_tx.take() {
                    let _ = tx.send(match host {
                        Some(host) => Ok(host),
                        None => Err(ConnectionFailure::new(
                            FailureKind::Protocol,
                            Some("auth.approved without host".into()),
                            false,
                        )),
                    });
                }
            }
            methods::AUTH_DENIED => {
                let reason = serde_json::from_value::<AuthDeniedParams>(params)
                    .ok()
                    .map(|p| display_text(&p.reason, 40))
                    .filter(|r| !r.is_empty());
                let mut data = a.data.lock().expect("attempt");
                if data.ready {
                    // An approved connection is being ended (the device was revoked,
                    // remote control was switched off).
                    let notice = reason.unwrap_or_else(|| "denied".into());
                    data.notice = Some(notice.clone());
                    let rpc = data.rpc.clone();
                    drop(data);
                    if let Some(rpc) = rpc {
                        rpc.close(&format!("notice:{notice}"));
                    }
                } else if let Some(tx) = data.approval_tx.take() {
                    let _ = tx.send(Err(ConnectionFailure::new(
                        FailureKind::Denied,
                        reason,
                        false,
                    )));
                }
            }
            _ => {
                let mut data = a.data.lock().expect("attempt");
                if data.awaiting || data.disposed {
                    return;
                }
                if data.published {
                    let _ = self
                        .delivery
                        .send(Delivery::Notification(method.to_owned(), params));
                } else if data.held.len() < HELD_NOTIFICATIONS_MAX {
                    data.held.push((method.to_owned(), params));
                }
            }
        }
    }

    fn link_closed(self: &Arc<Self>, a: &Arc<Attempt>, reason: &str) {
        let notice = {
            let mut data = a.data.lock().expect("attempt");
            data.close_reason = Some(reason.to_owned());
            data.notice.clone()
        };
        {
            let mut core = self.core.lock().expect("core");
            if !core.live.as_ref().is_some_and(|live| Arc::ptr_eq(live, a)) {
                return;
            }
            // A link that was up: clean up and reconnect.
            core.live = None;
        }
        self.dispose(a, reason);
        if let Some(notice) = notice {
            let failure = match notice.as_str() {
                "revoked" => ConnectionFailure::new(FailureKind::Revoked, None, false),
                "disabled" => ConnectionFailure::new(FailureKind::Disabled, None, false),
                _ => ConnectionFailure::new(FailureKind::Denied, Some(notice), false),
            };
            if failure.kind == FailureKind::Revoked {
                let keys = Arc::clone(&self.deps.keys);
                let host_id = self.deps.host_id.clone();
                tokio::spawn(async move { keys.unpair(&host_id).await });
            }
            self.fail(failure);
            return;
        }
        {
            let mut core = self.core.lock().expect("core");
            if core
                .ready_at
                .is_some_and(|at| at.elapsed() >= SSE_BACKOFF_RESET)
            {
                core.failures = 0;
            }
        }
        self.fail(ConnectionFailure::new(
            FailureKind::Closed,
            Some(reason.to_owned()),
            true,
        ));
    }

    // -----------------------------------------------------------------------
    // Cleanup
    // -----------------------------------------------------------------------

    /// Release everything an attempt holds and remove its database nodes. Safe to call twice.
    fn dispose(self: &Arc<Self>, a: &Arc<Attempt>, reason: &str) {
        let (rpc, tunnel, direct, peer, conn_id, offer_written, device_id, was_ready) = {
            let mut data = a.data.lock().expect("attempt");
            if data.disposed {
                return;
            }
            data.disposed = true;
            data.approval_tx = None;
            (
                data.rpc.clone(),
                data.tunnel.clone(),
                data.direct.clone(),
                data.peer.clone(),
                data.conn_id.clone(),
                data.offer_written,
                data.device_id.clone(),
                data.ready,
            )
        };
        a.cancel.cancel();
        {
            use crate::transport::FragmentLink as _;
            if let Some(rpc) = &rpc {
                rpc.close(reason);
            } else if let Some(tunnel) = &tunnel {
                tunnel.close(reason);
            } else if let Some(direct) = &direct {
                direct.close(reason);
            }
        }
        if let Some(peer) = peer {
            peer.close();
        }
        if was_ready && !conn_id.is_empty() {
            if let Some(handler) = self.deps.media_offer_handler.clone() {
                let connection = RemoteMediaConnection {
                    account_uid: self.deps.uid.clone(),
                    host_id: self.deps.host_id.clone(),
                    device_id,
                    connection_id: conn_id.clone(),
                };
                tokio::spawn(async move { handler.connection_closed(&connection).await });
            }
        }
        if conn_id.is_empty() {
            return;
        }
        let deps = &self.deps;
        let paths = [
            to_host_conn_path(&deps.uid, &deps.host_id, &conn_id),
            to_viewer_conn_path(&deps.uid, &deps.host_id, &conn_id),
        ];
        let rtdb = Arc::clone(&deps.rtdb);
        let remove = async move {
            for path in paths.into_iter().flatten() {
                let _ = rtdb.remove(&path).await;
            }
        };
        // The tunnel flushes its close notice and deletes its own frames first; the
        // rest is removed after.
        if let Some(tunnel) = tunnel {
            tokio::spawn(async move {
                tunnel.when_closed().await;
                remove.await;
            });
        } else if offer_written {
            tokio::spawn(remove);
        }
    }
}
