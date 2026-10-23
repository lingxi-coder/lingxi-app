//! Remote Control (`lingxi-rc/1`) viewer for the iOS app.
//!
//! [`IosRemoteControlClient`] lets the phone list the user's computers, dial
//! one over Firebase RTDB signaling, pair with it, and follow and drive its
//! sessions. The protocol, state machines and crypto live in
//! `packages/remote-control-client`; this module only adapts them to UniFFI:
//!
//! - three foreign callback interfaces: [`IosRemoteTokenProvider`] (Firebase ID
//!   token), [`IosRemoteKeyStore`] (the device key and the pinned host keys, which
//!   the app persists — Rust writes no files) and [`IosRemoteListener`];
//! - flat records for connection state, hosts and session summaries;
//! - session content as [`RemoteSessionUpdateFfi`] deltas whose records embed the
//!   UniFFI `ClientEvent` the rest of the app already renders.
//!
//! The client runs on the runtime UniFFI polls its async exports on
//! (`async_runtime = "tokio"`), like the other async exports of this crate, so
//! the object is built by an async function and every method that may start work
//! is async. Listener callbacks are never made from the caller's thread: one
//! delivery task per client drains a queue, so their order is the order of events.
//!
//! WebRTC is not wired yet: [`webrtc_peer_connector`] is the single seam where a
//! `PeerConnector` will be plugged in; until then every connection uses the
//! end-to-end encrypted RTDB tunnel.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use client::protocol::ask_user_question::AskUserQuestionRequestDto;
use client::protocol::commands::ImageRefDto;
use client::protocol::computer_access::ComputerAccessRequestDto;
use client::protocol::controls::{ConversationControlsDto, ReasoningSelectionDto};
use client::protocol::events::ClientEvent;
use client::protocol::listings::ModelDetailsDto;
use client::protocol::message::MessageDto;
use client::protocol::permission::PermissionRequest;
use remote_control_client::clock::{system_clock, Clock};
use remote_control_client::connection::{
    ConnectionFailure, ConnectionListener, ConnectionPhase, ConnectionState, FailureKind,
    ViewerConnection, ViewerConnectionDeps,
};
use remote_control_client::decode::DecodedEvent;
use remote_control_client::device::{
    DeviceIdentity, KeysError, PinnedHost, TunnelMemory, ViewerKeys,
};
use remote_control_client::encoding::{b64_decode, b64_encode, random_unit};
use remote_control_client::keys::IdentityKey;
use remote_control_client::messages::{
    PermissionModeId, RemoteImageRef, RemotePending, RemotePendingAction, RemotePromptOrigin,
    RemoteRecord, RemoteSeqRecord, RemoteSessionStatus, RemoteSessionSummary, RemoteTurnState,
};
use remote_control_client::peer::PeerConnector;
use remote_control_client::rpc::RpcError;
use remote_control_client::rtdb::{
    ListenItem, Rtdb, RtdbClient, RtdbClientOptions, RtdbMirror, TokenError, TokenProvider,
};
use remote_control_client::session::{
    OutgoingPrompt, OutgoingStatus, RemoteClient, RemoteClientListener, SessionController,
    SessionListener, SessionState, SessionStatus,
};
use remote_control_client::signaling::{
    hosts_path, parse_hosts_listing, DevicePlatform, HostPresence, HostRecord, TransportKind,
};
use remote_control_client::text::display_text;
use remote_control_client::version::HEARTBEAT_INTERVAL;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Largest image the desktop accepts in a prompt.
const MAX_PROMPT_IMAGE_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything the remote client reports to the app, including what the app's own
/// callbacks (token provider, key store) report back through it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum RemoteFfiError {
    /// No computer is connected.
    #[error("not connected")]
    NotConnected,
    /// An argument was rejected before anything was sent.
    #[error("invalid argument: {message}")]
    InvalidArgument {
        /// What is wrong.
        message: String,
    },
    /// The desktop (or the link) failed the request; `code` is the protocol's error code
    /// (`not_approved`, `not_pending`, `forbidden`, `closed`, `timeout`, ...).
    #[error("remote error {code}: {detail}")]
    Rpc {
        /// Protocol error code.
        code: String,
        /// A message safe to show.
        detail: String,
    },
    /// The Firebase ID token could not be obtained.
    #[error("token unavailable: {message}")]
    Token {
        /// Detail from the app.
        message: String,
    },
    /// The device key store failed.
    #[error("key store failed: {message}")]
    KeyStore {
        /// Detail.
        message: String,
    },
    /// Anything else (the database could not be read, ...).
    #[error("remote control failed: {message}")]
    Internal {
        /// Detail.
        message: String,
    },
}

fn rpc_error(error: RpcError) -> RemoteFfiError {
    RemoteFfiError::Rpc {
        code: error.code.as_str().to_owned(),
        detail: display_text(&error.message, 200),
    }
}

// ---------------------------------------------------------------------------
// Foreign callbacks
// ---------------------------------------------------------------------------

/// Supplies Firebase ID tokens (the Swift app asks `User.getIDToken`).
/// Called for every database request, so implementations should cache;
/// `force_refresh` is set on the single retry after a 401.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait IosRemoteTokenProvider: Send + Sync {
    /// A valid ID token, minted afresh when `force_refresh` is set.
    async fn id_token(&self, force_refresh: bool) -> Result<String, RemoteFfiError>;
}

/// A computer this device has paired with. `host_key` is the `b64u` of the host's
/// raw Ed25519 key, which every later answer must be signed with.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemotePinnedHostFfi {
    /// The host's id.
    pub host_id: String,
    /// Pinned host key.
    pub host_key: String,
    /// Display name.
    pub name: String,
    /// When pairing completed (ms since the epoch).
    pub paired_at_ms: u64,
}

/// Persistent device state, implemented by the Swift app over the
/// Keychain. The device identity crosses as its PKCS#8 encoding
/// (48 bytes: the 16-byte Ed25519 header and the seed), which is what the other
/// clients store. Treat it as a secret.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait IosRemoteKeyStore: Send + Sync {
    /// The stored device key, if one was created before.
    async fn load_identity(&self) -> Result<Option<Vec<u8>>, RemoteFfiError>;
    /// Keep the device key.
    async fn store_identity(&self, pkcs8: Vec<u8>) -> Result<(), RemoteFfiError>;
    /// Throw the device key away (a missing key is not an error).
    async fn delete_identity(&self) -> Result<(), RemoteFfiError>;
    /// The pinned key of a computer, if this device has paired with it.
    async fn load_pinned_host(
        &self,
        host_id: String,
    ) -> Result<Option<RemotePinnedHostFfi>, RemoteFfiError>;
    /// Remember a computer's key once the user approved this device on it.
    async fn store_pinned_host(&self, host: RemotePinnedHostFfi) -> Result<(), RemoteFfiError>;
    /// Forget a computer (a missing entry is not an error).
    async fn delete_pinned_host(&self, host_id: String) -> Result<(), RemoteFfiError>;
}

/// Hears about the remote client. Implementations enqueue onto the UI's state and
/// return promptly; calls arrive one at a time, in the order things happened.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait IosRemoteListener: Send + Sync {
    /// The connection to the current computer changed state.
    async fn on_connection_state(&self, state: RemoteConnectionStateFfi);
    /// The list of the user's computers changed (only while watching hosts).
    async fn on_hosts(&self, hosts: Vec<RemoteHostFfi>);
    /// Watching the computers ended because the database refused or cancelled it.
    async fn on_hosts_failed(&self, message: String);
    /// The current computer's list of shared sessions changed.
    async fn on_sessions(&self, host_id: String, sessions: Vec<RemoteSessionSummaryFfi>);
    /// An attached session changed.
    async fn on_session_update(&self, update: RemoteSessionUpdateFfi);
}

struct TokenBridge {
    inner: Box<dyn IosRemoteTokenProvider>,
}

#[async_trait]
impl TokenProvider for TokenBridge {
    async fn id_token(&self, force_refresh: bool) -> Result<String, TokenError> {
        self.inner
            .id_token(force_refresh)
            .await
            .map_err(|error| TokenError(error.to_string()))
    }
}

/// Adapts the app's [`IosRemoteKeyStore`] to the client's [`ViewerKeys`]. What
/// was read or written is also kept in memory: the client asks for the identity
/// and the pins often, and a pin the store failed to persist still holds for this run.
struct KeyStoreBridge {
    inner: Box<dyn IosRemoteKeyStore>,
    identity: tokio::sync::Mutex<Option<Arc<IdentityKey>>>,
    pins: Mutex<HashMap<String, PinnedHost>>,
}

impl KeyStoreBridge {
    fn new(inner: Box<dyn IosRemoteKeyStore>) -> Self {
        Self {
            inner,
            identity: tokio::sync::Mutex::new(None),
            pins: Mutex::new(HashMap::new()),
        }
    }
}

fn keys_error(error: RemoteFfiError) -> KeysError {
    KeysError(error.to_string())
}

#[async_trait]
impl ViewerKeys for KeyStoreBridge {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        let mut cached = self.identity.lock().await;
        if let Some(key) = cached.as_ref() {
            return Ok(DeviceIdentity::from_signer(Arc::clone(key) as _));
        }
        let key = match self.inner.load_identity().await.map_err(keys_error)? {
            // A stored key that does not parse is reported, never replaced: a
            // new identity would silently unpair every computer.
            Some(pkcs8) => IdentityKey::from_pkcs8(&pkcs8)
                .map_err(|error| KeysError(format!("stored device key: {error}")))?,
            None => {
                let key = IdentityKey::generate();
                self.inner
                    .store_identity(key.to_pkcs8().to_vec())
                    .await
                    .map_err(keys_error)?;
                key
            }
        };
        let key = Arc::new(key);
        *cached = Some(Arc::clone(&key));
        Ok(DeviceIdentity::from_signer(key))
    }

    async fn reset_identity(&self) -> Result<(), KeysError> {
        let mut cached = self.identity.lock().await;
        self.inner.delete_identity().await.map_err(keys_error)?;
        *cached = None;
        Ok(())
    }

    async fn pinned_host(&self, host_id: &str) -> Option<PinnedHost> {
        if let Some(host) = self.pins.lock().expect("pins").get(host_id) {
            return Some(host.clone());
        }
        let stored = self
            .inner
            .load_pinned_host(host_id.to_owned())
            .await
            .ok()
            .flatten()?;
        let host = PinnedHost {
            host_id: stored.host_id,
            host_key: stored.host_key,
            name: stored.name,
            paired_at: stored.paired_at_ms,
        };
        self.pins
            .lock()
            .expect("pins")
            .insert(host_id.to_owned(), host.clone());
        Some(host)
    }

    async fn pin(&self, host: PinnedHost) {
        self.pins
            .lock()
            .expect("pins")
            .insert(host.host_id.clone(), host.clone());
        // The trait cannot report a failure; the pin holds for this run, and the
        // next run pairs again if the store lost it.
        let _ = self
            .inner
            .store_pinned_host(RemotePinnedHostFfi {
                host_id: host.host_id,
                host_key: host.host_key,
                name: host.name,
                paired_at_ms: host.paired_at,
            })
            .await;
    }

    async fn unpair(&self, host_id: &str) {
        self.pins.lock().expect("pins").remove(host_id);
        let _ = self.inner.delete_pinned_host(host_id.to_owned()).await;
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A computer of the user's, as the database lists it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteHostFfi {
    /// The host's id.
    pub host_id: String,
    /// Display name.
    pub name: String,
    /// Operating-system family.
    pub platform: String,
    /// Desktop app version.
    pub app_version: String,
    /// Whether its heartbeat is recent.
    pub online: bool,
    /// Server time of its last heartbeat (ms since the epoch).
    pub last_seen_ms: u64,
    /// Whether this device is paired with it.
    pub paired: bool,
}

/// Where a connection stands. `Pairing` shows the code the desktop's dialog shows;
/// `Ready` means approved and connected; `Reconnecting` is "offline, trying again";
/// `Idle` is "closed" (not started, or stopped); `Failed` carries the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteConnectionPhaseFfi {
    /// Nothing running.
    Idle,
    /// Dialling: offer, answer, channel, handshake.
    Connecting,
    /// Waiting for the user to allow this device on the desktop.
    Pairing,
    /// Connected and approved.
    Ready,
    /// Connected before and lost it; waiting out the backoff, or dialling again.
    Reconnecting,
    /// Stopped trying.
    Failed,
}

/// How a connection travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteTransportFfi {
    /// A WebRTC DataChannel.
    Webrtc,
    /// The encrypted RTDB tunnel.
    Tunnel,
}

/// Why a connection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteFailureKindFfi {
    /// This device cannot create or use its keys.
    Unsupported,
    /// First pairing, but the computer is not listed.
    UnknownHost,
    /// The database could not be reached.
    Network,
    /// The desktop did not answer.
    NoAnswer,
    /// The link ended.
    Closed,
    /// The desktop has too many connections.
    Capacity,
    /// The desktop is busy.
    Busy,
    /// The desktop no longer trusts this device; pair again with a new key.
    Revoked,
    /// Remote control is switched off on the desktop.
    Disabled,
    /// The protocol major differs; one of the apps must be updated.
    UpgradeRequired,
    /// The user said no on the desktop.
    Denied,
    /// The desktop thinks this device's clock is off.
    Expired,
    /// Another reason the desktop gave.
    Rejected,
    /// The answer did not verify.
    Verification,
    /// The pairing codes differ: someone may be in the middle.
    PairingCodeMismatch,
    /// The desktop broke the protocol.
    Protocol,
}

/// A failure and whether the client retries by itself.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteFailureFfi {
    /// What failed.
    pub kind: RemoteFailureKindFfi,
    /// Machine-readable detail.
    pub detail: Option<String>,
    /// Whether the client retries by itself (with backoff).
    pub auto_retry: bool,
}

/// The desktop's description of itself, once approved.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteHostInfoFfi {
    /// Display name.
    pub name: String,
    /// Operating-system family.
    pub platform: String,
    /// Desktop app version.
    pub app_version: String,
}

/// A snapshot of the connection to the current computer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteConnectionStateFfi {
    /// The computer being dialled.
    pub host_id: String,
    /// The phase.
    pub phase: RemoteConnectionPhaseFfi,
    /// How the current, or the last, connection travels.
    pub transport: Option<RemoteTransportFfi>,
    /// While `Pairing`: the 6-digit code to compare with the desktop's dialog.
    pub pairing_code: Option<String>,
    /// In `Failed`, and in `Reconnecting` as the reason the link dropped.
    pub failure: Option<RemoteFailureFfi>,
    /// Consecutive failed attempts since the connection was last stable.
    pub attempt: u32,
    /// While reconnecting: when the next attempt starts (ms since the epoch).
    pub retry_at_ms: Option<u64>,
    /// The desktop, once approved.
    pub host: Option<RemoteHostInfoFfi>,
    /// Whether the connection has been ready at least once.
    pub ever_ready: bool,
}

/// Where a shared session stands on the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteSessionStatusFfi {
    /// A turn is running.
    Running,
    /// Waiting for a decision.
    RequiresAction,
    /// Open and idle.
    Idle,
    /// Not loaded.
    Dormant,
}

/// What a session waits on while it requires action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemotePendingActionFfi {
    /// A permission request.
    Permission,
    /// A question.
    Question,
    /// Computer access (display only; it can be granted on the desktop alone).
    ComputerAccess,
}

/// One session the desktop shares.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteSessionSummaryFfi {
    /// Session id.
    pub session_id: String,
    /// Title.
    pub title: String,
    /// Basename of the project directory.
    pub project_label: String,
    /// Status.
    pub status: RemoteSessionStatusFfi,
    /// What it waits on, if anything.
    pub pending_action: Option<RemotePendingActionFfi>,
    /// Active model.
    pub model: Option<String>,
    /// Viewers attached right now.
    pub viewers: u32,
}

/// An image the desktop keeps; fetch it with `fetch_image`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteImageRefFfi {
    /// The reference to pass to `fetch_image`.
    pub image_ref: String,
    /// MIME type.
    pub media_type: String,
}

/// Who started a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemotePromptOriginFfi {
    /// Typed on the desktop.
    Desktop,
    /// Sent from a remote device.
    Device,
    /// A scheduled run.
    Scheduled,
}

/// One entry of a session's record stream.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum RemoteRecordFfi {
    /// An engine event, the same `ClientEvent` a local engine delivers.
    Event {
        /// The event.
        event: ClientEvent,
        /// Large fields were cut by the desktop.
        truncated: bool,
    },
    /// A user prompt (the engine's event stream does not carry them).
    Prompt {
        /// Turn id.
        turn_id: u64,
        /// Prompt text.
        text: String,
        /// Attached images.
        images: Vec<RemoteImageRefFfi>,
        /// Who sent it.
        origin: RemotePromptOriginFfi,
        /// Name of the sending device when `origin` is `Device`.
        device_name: Option<String>,
    },
    /// A permission request.
    PermissionRequest {
        /// The request, answered with `respond_permission`.
        request: PermissionRequest,
    },
    /// Shown only; computer access can be granted on the desktop alone.
    ComputerAccessRequest {
        /// The request.
        request: ComputerAccessRequestDto,
    },
    /// An event too large to send.
    Omitted {
        /// The omitted event's type.
        event_type: Option<String>,
        /// Its size in bytes.
        size_bytes: Option<u64>,
    },
    /// An event or record kind a newer desktop sends that this app does not know.
    Unsupported {
        /// Its `type` tag.
        type_name: String,
    },
}

/// A record with its position in the stream.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RemoteSeqRecordFfi {
    /// Position.
    pub seq: u64,
    /// The record.
    pub record: RemoteRecordFfi,
}

/// What a session waits for an answer to; always the complete set.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct RemotePendingFfi {
    /// Permission requests.
    pub permissions: Vec<PermissionRequest>,
    /// Questions.
    pub questions: Vec<AskUserQuestionRequestDto>,
    /// Display only.
    pub computer_access: Vec<ComputerAccessRequestDto>,
}

/// Whether a turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct RemoteTurnFfi {
    /// A turn is running.
    pub running: bool,
    /// Its id, when known.
    pub turn_id: Option<u64>,
}

/// Where a followed session stands on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteSessionPhaseFfi {
    /// Attaching, or waiting for the snapshot.
    Loading,
    /// Up to date and following.
    Live,
    /// The connection is down; what is held is what was last received.
    Offline,
    /// The desktop refused the attach.
    Error,
    /// The session is no longer shared.
    Closed,
}

/// An error code and a message safe to show.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteSessionErrorFfi {
    /// The protocol's error code.
    pub code: String,
    /// The message.
    pub message: String,
}

/// Whether a prompt sent from this device is still on its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteOutgoingStatusFfi {
    /// Waiting for the desktop to accept it.
    Sending,
    /// The desktop did not take it; retry or dismiss it.
    Failed,
}

/// A prompt this device sent that its own record has not come back for yet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteOutgoingPromptFfi {
    /// Also the `client_request_id`.
    pub id: String,
    /// The text.
    pub text: String,
    /// Attached images.
    pub images: Vec<ImageRefDto>,
    /// State.
    pub status: RemoteOutgoingStatusFfi,
    /// The turn the desktop assigned.
    pub turn_id: Option<u64>,
    /// Error code of a failure.
    pub error: Option<String>,
}

/// A change of a followed session. The meta fields (`phase` through
/// `answered_elsewhere`) are always the current values. Content is a delta:
///
/// - `replace`: `messages` and `records` are the complete base, discard what you
///   hold (first load, a new snapshot, a reconnect);
/// - otherwise `records` are appended and, when a page of older history was
///   loaded, `older_messages` are prepended (`first_message_index` is the index
///   of the new first message).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RemoteSessionUpdateFfi {
    /// The session.
    pub session_id: String,
    /// Where it stands.
    pub phase: RemoteSessionPhaseFfi,
    /// Why the attach failed.
    pub error: Option<RemoteSessionErrorFfi>,
    /// The content below replaces everything held.
    pub replace: bool,
    /// The base conversation (only with `replace`).
    pub messages: Vec<MessageDto>,
    /// Older history to put in front of the held messages.
    pub older_messages: Vec<MessageDto>,
    /// Index of the first held message in the desktop's conversation.
    pub first_message_index: u64,
    /// New records, ordered by `seq` (with `replace`: all of them).
    pub records: Vec<RemoteSeqRecordFfi>,
    /// What the session waits for an answer to.
    pub pending: RemotePendingFfi,
    /// Whether a turn runs.
    pub turn: RemoteTurnFfi,
    /// Model, reasoning and permission controls.
    pub controls: Option<ConversationControlsDto>,
    /// Prompts on their way.
    pub outgoing: Vec<RemoteOutgoingPromptFfi>,
    /// Whether older messages can be fetched.
    pub has_older: bool,
    /// Whether they are being fetched.
    pub loading_older: bool,
    /// An answer lost the race to another device (cleared by the next change).
    pub answered_elsewhere: bool,
}

/// An image of the desktop, decoded.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteImageFfi {
    /// MIME type.
    pub media_type: String,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// An image to send with a prompt (at most 4, each 2 MiB or less; the app shrinks them).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemotePromptImageFfi {
    /// MIME type (`image/png`, `image/jpeg`, `image/gif`, `image/webp`).
    pub media_type: String,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// Permission modes a remote device may select (never bypass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemotePermissionModeFfi {
    /// Ask for everything not allowed.
    Default,
    /// Accept edits automatically.
    AcceptEdits,
    /// Plan only.
    Plan,
    /// Engine-decided automatic mode.
    Auto,
    /// Never ask; deny what is not allowed.
    DontAsk,
}

/// The models the desktop offers.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RemoteModelsFfi {
    /// Model ids.
    pub models: Vec<String>,
    /// The current model.
    pub current: String,
    /// Details, when the desktop sends them.
    pub details: Vec<ModelDetailsDto>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn failure_kind(kind: FailureKind) -> RemoteFailureKindFfi {
    match kind {
        FailureKind::Unsupported => RemoteFailureKindFfi::Unsupported,
        FailureKind::UnknownHost => RemoteFailureKindFfi::UnknownHost,
        FailureKind::Network => RemoteFailureKindFfi::Network,
        FailureKind::NoAnswer => RemoteFailureKindFfi::NoAnswer,
        FailureKind::Closed => RemoteFailureKindFfi::Closed,
        FailureKind::Capacity => RemoteFailureKindFfi::Capacity,
        FailureKind::Busy => RemoteFailureKindFfi::Busy,
        FailureKind::Revoked => RemoteFailureKindFfi::Revoked,
        FailureKind::Disabled => RemoteFailureKindFfi::Disabled,
        FailureKind::UpgradeRequired => RemoteFailureKindFfi::UpgradeRequired,
        FailureKind::Denied => RemoteFailureKindFfi::Denied,
        FailureKind::Expired => RemoteFailureKindFfi::Expired,
        FailureKind::Rejected => RemoteFailureKindFfi::Rejected,
        FailureKind::Verification => RemoteFailureKindFfi::Verification,
        FailureKind::PairingCodeMismatch => RemoteFailureKindFfi::PairingCodeMismatch,
        FailureKind::Protocol => RemoteFailureKindFfi::Protocol,
    }
}

fn failure_to_ffi(failure: &ConnectionFailure) -> RemoteFailureFfi {
    RemoteFailureFfi {
        kind: failure_kind(failure.kind),
        detail: failure.detail.clone(),
        auto_retry: failure.auto,
    }
}

fn connection_state_to_ffi(state: &ConnectionState) -> RemoteConnectionStateFfi {
    RemoteConnectionStateFfi {
        host_id: state.host_id.clone(),
        phase: match state.phase {
            ConnectionPhase::Idle => RemoteConnectionPhaseFfi::Idle,
            ConnectionPhase::Connecting => RemoteConnectionPhaseFfi::Connecting,
            ConnectionPhase::AwaitingApproval => RemoteConnectionPhaseFfi::Pairing,
            ConnectionPhase::Ready => RemoteConnectionPhaseFfi::Ready,
            ConnectionPhase::Reconnecting => RemoteConnectionPhaseFfi::Reconnecting,
            ConnectionPhase::Failed => RemoteConnectionPhaseFfi::Failed,
        },
        transport: state.transport.map(|transport| match transport {
            TransportKind::Webrtc => RemoteTransportFfi::Webrtc,
            TransportKind::Tunnel => RemoteTransportFfi::Tunnel,
        }),
        pairing_code: state.pairing_code.clone(),
        failure: state.failure.as_ref().map(failure_to_ffi),
        attempt: state.attempt,
        retry_at_ms: state.retry_at,
        host: state.host.as_ref().map(|host| RemoteHostInfoFfi {
            name: host.name.clone(),
            platform: host.platform.clone(),
            app_version: host.app_version.clone(),
        }),
        ever_ready: state.ever_ready,
    }
}

fn session_summary_to_ffi(summary: &RemoteSessionSummary) -> RemoteSessionSummaryFfi {
    RemoteSessionSummaryFfi {
        session_id: summary.session_id.clone(),
        title: summary.title.clone(),
        project_label: summary.project_label.clone(),
        status: match summary.status {
            RemoteSessionStatus::Running => RemoteSessionStatusFfi::Running,
            RemoteSessionStatus::RequiresAction => RemoteSessionStatusFfi::RequiresAction,
            RemoteSessionStatus::Idle => RemoteSessionStatusFfi::Idle,
            RemoteSessionStatus::Dormant => RemoteSessionStatusFfi::Dormant,
        },
        pending_action: summary.pending_action.map(|action| match action {
            RemotePendingAction::Permission => RemotePendingActionFfi::Permission,
            RemotePendingAction::Question => RemotePendingActionFfi::Question,
            RemotePendingAction::ComputerAccess => RemotePendingActionFfi::ComputerAccess,
        }),
        model: summary.model.clone(),
        viewers: summary.viewers,
    }
}

fn image_ref_to_ffi(image: &RemoteImageRef) -> RemoteImageRefFfi {
    RemoteImageRefFfi {
        image_ref: image.image_ref.clone(),
        media_type: image.media_type.clone(),
    }
}

/// A record as the app sees it: a known event embeds the UniFFI `ClientEvent`, an
/// event or record kind this build does not know becomes `Unsupported`.
fn record_to_ffi(record: &RemoteRecord) -> RemoteRecordFfi {
    match record {
        RemoteRecord::Event { event, truncated } => match event {
            DecodedEvent::Event(event) => RemoteRecordFfi::Event {
                event: event.clone(),
                truncated: *truncated,
            },
            DecodedEvent::Unsupported { event_type, .. } => RemoteRecordFfi::Unsupported {
                type_name: event_type.clone(),
            },
        },
        RemoteRecord::Prompt {
            turn_id,
            text,
            images,
            origin,
            device_name,
        } => RemoteRecordFfi::Prompt {
            turn_id: *turn_id,
            text: text.clone(),
            images: images.iter().map(image_ref_to_ffi).collect(),
            origin: match origin {
                RemotePromptOrigin::Desktop => RemotePromptOriginFfi::Desktop,
                RemotePromptOrigin::Device => RemotePromptOriginFfi::Device,
                RemotePromptOrigin::Scheduled => RemotePromptOriginFfi::Scheduled,
            },
            device_name: device_name.clone(),
        },
        RemoteRecord::PermissionRequest { request } => RemoteRecordFfi::PermissionRequest {
            request: request.clone(),
        },
        RemoteRecord::ComputerAccessRequest { request } => RemoteRecordFfi::ComputerAccessRequest {
            request: request.clone(),
        },
        RemoteRecord::Omitted {
            event_type,
            size_bytes,
        } => RemoteRecordFfi::Omitted {
            event_type: event_type.clone(),
            size_bytes: *size_bytes,
        },
        RemoteRecord::Unsupported { record_type } => RemoteRecordFfi::Unsupported {
            type_name: record_type.clone(),
        },
    }
}

fn seq_record_to_ffi(item: &RemoteSeqRecord) -> RemoteSeqRecordFfi {
    RemoteSeqRecordFfi {
        seq: item.seq,
        record: record_to_ffi(&item.record),
    }
}

fn pending_to_ffi(pending: &RemotePending) -> RemotePendingFfi {
    RemotePendingFfi {
        permissions: pending.permissions.clone(),
        questions: pending.questions.clone(),
        computer_access: pending.computer_access.clone(),
    }
}

fn turn_to_ffi(turn: &RemoteTurnState) -> RemoteTurnFfi {
    RemoteTurnFfi {
        running: turn.running,
        turn_id: turn.turn_id,
    }
}

fn outgoing_to_ffi(prompt: &OutgoingPrompt) -> RemoteOutgoingPromptFfi {
    RemoteOutgoingPromptFfi {
        id: prompt.id.clone(),
        text: prompt.text.clone(),
        images: prompt.images.clone(),
        status: match prompt.status {
            OutgoingStatus::Sending => RemoteOutgoingStatusFfi::Sending,
            OutgoingStatus::Failed => RemoteOutgoingStatusFfi::Failed,
        },
        turn_id: prompt.turn_id,
        error: prompt.error.clone(),
    }
}

fn permission_mode(mode: RemotePermissionModeFfi) -> PermissionModeId {
    match mode {
        RemotePermissionModeFfi::Default => PermissionModeId::Default,
        RemotePermissionModeFfi::AcceptEdits => PermissionModeId::AcceptEdits,
        RemotePermissionModeFfi::Plan => PermissionModeId::Plan,
        RemotePermissionModeFfi::Auto => PermissionModeId::Auto,
        RemotePermissionModeFfi::DontAsk => PermissionModeId::DontAsk,
    }
}

/// What the app was last told about a session's content, so the next state can
/// be sent as a delta.
#[derive(Debug)]
struct SentMarker {
    /// The next content must replace everything (nothing sent yet, or a load happened since).
    replace: bool,
    messages_len: usize,
    first_index: u64,
    records_len: usize,
    last_seq: Option<u64>,
}

impl SentMarker {
    fn new() -> Self {
        Self {
            replace: true,
            messages_len: 0,
            first_index: 0,
            records_len: 0,
            last_seq: None,
        }
    }
}

/// The update that takes the app from what `marker` says it holds to `state`.
///
/// A load (`Loading`) may end in a new base, so the first content afterwards
/// replaces everything; a resumed attach re-sends once, which is rare and keeps
/// the rule simple. Between loads the base only grows: records are appended and
/// a page of older messages is prepended; anything else replaces.
fn session_update(marker: &mut SentMarker, state: &SessionState) -> RemoteSessionUpdateFfi {
    let mut update = RemoteSessionUpdateFfi {
        session_id: state.session_id.clone(),
        phase: match state.status {
            SessionStatus::Loading => RemoteSessionPhaseFfi::Loading,
            SessionStatus::Live => RemoteSessionPhaseFfi::Live,
            SessionStatus::Offline => RemoteSessionPhaseFfi::Offline,
            SessionStatus::Error => RemoteSessionPhaseFfi::Error,
            SessionStatus::Closed => RemoteSessionPhaseFfi::Closed,
        },
        error: state.error.as_ref().map(|error| RemoteSessionErrorFfi {
            code: error.code.clone(),
            message: error.message.clone(),
        }),
        replace: false,
        messages: Vec::new(),
        older_messages: Vec::new(),
        first_message_index: state.first_message_index,
        records: Vec::new(),
        pending: pending_to_ffi(&state.pending),
        turn: turn_to_ffi(&state.turn),
        controls: state.controls.clone(),
        outgoing: state.outgoing.iter().map(outgoing_to_ffi).collect(),
        has_older: state.has_older,
        loading_older: state.loading_older,
        answered_elsewhere: state.answered_elsewhere,
    };
    if state.status == SessionStatus::Loading {
        marker.replace = true;
        return update;
    }
    let older_by = marker.first_index.checked_sub(state.first_message_index);
    let same_base = state.first_message_index == marker.first_index
        && state.messages.len() == marker.messages_len;
    let older_page = older_by.is_some_and(|by| {
        by > 0
            && usize::try_from(by).is_ok_and(|by| state.messages.len() == marker.messages_len + by)
    });
    let records_extend = state.records.len() >= marker.records_len
        && (marker.records_len == 0
            || state.records.get(marker.records_len - 1).map(|r| r.seq) == marker.last_seq);
    if marker.replace || !(same_base || older_page) || !records_extend {
        update.replace = true;
        update.messages = state.messages.clone();
        update.records = state.records.iter().map(seq_record_to_ffi).collect();
    } else {
        if older_page {
            let by = state.messages.len() - marker.messages_len;
            update.older_messages = state.messages[..by].to_vec();
        }
        update.records = state.records[marker.records_len..]
            .iter()
            .map(seq_record_to_ffi)
            .collect();
    }
    marker.replace = false;
    marker.messages_len = state.messages.len();
    marker.first_index = state.first_message_index;
    marker.records_len = state.records.len();
    marker.last_seq = state.records.last().map(|r| r.seq);
    update
}

/// Split the `data:` URL `fetch_image` returns into type and bytes.
fn image_from_data_url(url: &str) -> Result<RemoteImageFfi, RemoteFfiError> {
    let invalid = || RemoteFfiError::Internal {
        message: "the desktop sent a malformed image".into(),
    };
    let (media_type, base64) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
        .ok_or_else(invalid)?;
    Ok(RemoteImageFfi {
        media_type: media_type.to_owned(),
        bytes: b64_decode(base64).map_err(|_| invalid())?,
    })
}

fn prompt_images(images: Vec<RemotePromptImageFfi>) -> Result<Vec<ImageRefDto>, RemoteFfiError> {
    images
        .into_iter()
        .map(|image| {
            if image.bytes.len() > MAX_PROMPT_IMAGE_BYTES {
                return Err(RemoteFfiError::InvalidArgument {
                    message: "an image is larger than 2 MiB".into(),
                });
            }
            Ok(ImageRefDto {
                media_type: image.media_type,
                base64: b64_encode(&image.bytes),
            })
        })
        .collect()
}

fn host_views(
    entries: &[(String, HostRecord)],
    presence: &HostPresence,
    paired: &HashSet<String>,
) -> Vec<RemoteHostFfi> {
    entries
        .iter()
        .map(|(host_id, record)| RemoteHostFfi {
            host_id: host_id.clone(),
            name: record.name.clone(),
            platform: record.platform.clone(),
            app_version: record.app_version.clone(),
            online: presence.is_online(host_id),
            last_seen_ms: record.last_seen,
            paired: paired.contains(host_id),
        })
        .collect()
}

async fn paired_hosts(keys: &dyn ViewerKeys, entries: &[(String, HostRecord)]) -> HashSet<String> {
    let mut paired = HashSet::new();
    for (host_id, _) in entries {
        if keys.pinned_host(host_id).await.is_some() {
            paired.insert(host_id.clone());
        }
    }
    paired
}

// ---------------------------------------------------------------------------
// Delivery to the listener
// ---------------------------------------------------------------------------

enum Out {
    State(RemoteConnectionStateFfi),
    Hosts(Vec<RemoteHostFfi>),
    HostsFailed(String),
    Sessions(String, Vec<RemoteSessionSummaryFfi>),
    Session(Box<RemoteSessionUpdateFfi>),
}

type OutSender = mpsc::UnboundedSender<Out>;

/// Forwards a connection's state while it is the current one.
struct StateRelay {
    out: OutSender,
    current: Arc<AtomicU64>,
    generation: u64,
}

impl ConnectionListener for StateRelay {
    fn on_state(&self, state: &ConnectionState) {
        if self.current.load(Ordering::SeqCst) == self.generation {
            let _ = self.out.send(Out::State(connection_state_to_ffi(state)));
        }
    }
}

/// Forwards the session list while its connection is the current one.
struct SessionsRelay {
    out: OutSender,
    current: Arc<AtomicU64>,
    generation: u64,
    host_id: String,
}

impl RemoteClientListener for SessionsRelay {
    fn on_sessions(&self, sessions: &[RemoteSessionSummary]) {
        if self.current.load(Ordering::SeqCst) == self.generation {
            let _ = self.out.send(Out::Sessions(
                self.host_id.clone(),
                sessions.iter().map(session_summary_to_ffi).collect(),
            ));
        }
    }
}

/// Turns a followed session's states into deltas for the app.
struct SessionWatcher {
    out: OutSender,
    marker: Mutex<SentMarker>,
}

impl SessionListener for SessionWatcher {
    fn on_session_state(&self, state: &SessionState) {
        // Computed and queued under one lock, so deltas leave in the order of the states.
        let mut marker = self.marker.lock().expect("marker");
        let update = session_update(&mut marker, state);
        let _ = self.out.send(Out::Session(Box::new(update)));
    }
}

async fn deliver(mut queue: mpsc::UnboundedReceiver<Out>, listener: Arc<dyn IosRemoteListener>) {
    while let Some(item) = queue.recv().await {
        match item {
            Out::State(state) => listener.on_connection_state(state).await,
            Out::Hosts(hosts) => listener.on_hosts(hosts).await,
            Out::HostsFailed(message) => listener.on_hosts_failed(message).await,
            Out::Sessions(host_id, sessions) => listener.on_sessions(host_id, sessions).await,
            Out::Session(update) => listener.on_session_update(*update).await,
        }
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// The seam for WebRTC. `lingxi-webrtc-core`'s data-only session is not pinned yet;
/// when it is, return its `PeerConnector` adapter here and nowhere else. `None`
/// makes every connection use the end-to-end encrypted RTDB tunnel.
fn webrtc_peer_connector() -> Option<Arc<dyn PeerConnector>> {
    None
}

/// What a client is made of; tests substitute the database and the clock.
struct Parts {
    uid: String,
    rtdb: Arc<dyn Rtdb>,
    keys: Arc<dyn ViewerKeys>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
    peers: Option<Arc<dyn PeerConnector>>,
    device_name: String,
    app_version: String,
}

/// The connection to the current computer and what hangs off it.
struct Active {
    host_id: String,
    connection: ViewerConnection,
    client: RemoteClient,
    sessions_listener: u64,
    watchers: HashMap<String, (Arc<SessionWatcher>, u64)>,
}

impl Active {
    fn shutdown(self) {
        self.client.remove_listener(self.sessions_listener);
        self.client.dispose();
        self.connection.stop();
    }
}

struct Core {
    parts: Parts,
    out: OutSender,
    tunnel_memory: Arc<TunnelMemory>,
    push_installation_id: Mutex<Option<String>>,
    /// Bumped whenever the current connection is replaced or ended; relays of an
    /// older connection go quiet.
    current: Arc<AtomicU64>,
    active: Mutex<Option<Active>>,
    hosts_task: Mutex<Option<JoinHandle<()>>>,
    delivery: JoinHandle<()>,
}

impl Drop for Core {
    fn drop(&mut self) {
        self.delivery.abort();
        if let Some(task) = self.hosts_task.lock().expect("hosts task").take() {
            task.abort();
        }
        if let Some(active) = self.active.lock().expect("active").take() {
            active.shutdown();
        }
    }
}

/// The Remote Control viewer: one instance per signed-in user, connected to one
/// computer at a time.
#[derive(uniffi::Object)]
pub struct IosRemoteControlClient {
    core: Core,
}

/// Build the client. `database_url` is the Realtime Database instance
/// (`https://lingxi-5e4b2-default-rtdb.firebaseio.com`), `uid` the Firebase uid
/// the token belongs to; `device_name` is shown next to the pairing code on the desktop.
#[uniffi::export(async_runtime = "tokio")]
pub async fn build_ios_remote_control_client(
    database_url: String,
    uid: String,
    device_name: String,
    app_version: String,
    tokens: Box<dyn IosRemoteTokenProvider>,
    keys: Box<dyn IosRemoteKeyStore>,
    listener: Box<dyn IosRemoteListener>,
) -> Result<Arc<IosRemoteControlClient>, RemoteFfiError> {
    if uid.is_empty() || database_url.is_empty() {
        return Err(RemoteFfiError::InvalidArgument {
            message: "database_url and uid are required".into(),
        });
    }
    let rtdb = RtdbClient::new(RtdbClientOptions::new(
        database_url,
        Arc::new(TokenBridge { inner: tokens }),
    ));
    let parts = Parts {
        uid,
        rtdb: Arc::new(rtdb),
        keys: Arc::new(KeyStoreBridge::new(keys)),
        clock: system_clock(),
        random: Arc::new(random_unit),
        peers: webrtc_peer_connector(),
        device_name,
        app_version,
    };
    Ok(Arc::new(IosRemoteControlClient::from_parts(
        parts,
        Arc::from(listener),
    )))
}

impl IosRemoteControlClient {
    /// Must be called inside a Tokio runtime.
    fn from_parts(parts: Parts, listener: Arc<dyn IosRemoteListener>) -> Self {
        let (out, queue) = mpsc::unbounded_channel();
        let tunnel_memory = Arc::new(TunnelMemory::new(Arc::clone(&parts.clock)));
        Self {
            core: Core {
                parts,
                out,
                tunnel_memory,
                push_installation_id: Mutex::new(None),
                current: Arc::new(AtomicU64::new(0)),
                active: Mutex::new(None),
                hosts_task: Mutex::new(None),
                delivery: tokio::spawn(deliver(queue, listener)),
            },
        }
    }

    fn client(&self) -> Result<RemoteClient, RemoteFfiError> {
        self.core
            .active
            .lock()
            .expect("active")
            .as_ref()
            .map(|active| active.client.clone())
            .ok_or(RemoteFfiError::NotConnected)
    }

    fn controller(&self, session_id: &str) -> Result<SessionController, RemoteFfiError> {
        Ok(self.client()?.session(session_id))
    }

    fn start_connection(&self, host_id: &str) -> Result<(), RemoteFfiError> {
        let core = &self.core;
        let parts = &core.parts;
        let generation = core.current.fetch_add(1, Ordering::SeqCst) + 1;
        let mut deps = ViewerConnectionDeps::new(
            parts.uid.clone(),
            host_id,
            Arc::clone(&parts.rtdb),
            Arc::clone(&parts.keys),
            DevicePlatform::Ios,
        );
        deps.clock = Arc::clone(&parts.clock);
        deps.random = Arc::clone(&parts.random);
        deps.peers = parts.peers.clone();
        deps.device_name = parts.device_name.clone();
        deps.app_version = parts.app_version.clone();
        deps.tunnel_memory = Some(Arc::clone(&core.tunnel_memory));
        deps.push_installation_id = core.push_installation_id.lock().expect("push id").clone();
        let connection = ViewerConnection::new(deps);
        let client = RemoteClient::for_connection(&connection);
        connection.add_listener(Arc::new(StateRelay {
            out: core.out.clone(),
            current: Arc::clone(&core.current),
            generation,
        }));
        let sessions_listener = client.add_listener(Arc::new(SessionsRelay {
            out: core.out.clone(),
            current: Arc::clone(&core.current),
            generation,
            host_id: host_id.to_owned(),
        }));
        connection.start();
        let previous = core.active.lock().expect("active").replace(Active {
            host_id: host_id.to_owned(),
            connection,
            client,
            sessions_listener,
            watchers: HashMap::new(),
        });
        if let Some(previous) = previous {
            previous.shutdown();
        }
        Ok(())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl IosRemoteControlClient {
    /// The push registration of this install, sent with the next handshake so the
    /// desktop can address and exclude it.
    pub fn set_push_installation_id(&self, id: Option<String>) {
        *self.core.push_installation_id.lock().expect("push id") = id;
    }

    /// Read the user's computers once, with their online state.
    pub async fn list_hosts(&self) -> Result<Vec<RemoteHostFfi>, RemoteFfiError> {
        let parts = &self.core.parts;
        let path = hosts_path(&parts.uid).map_err(|error| RemoteFfiError::Internal {
            message: error.to_string(),
        })?;
        let raw = parts
            .rtdb
            .get(&path)
            .await
            .map_err(|error| RemoteFfiError::Internal {
                message: error.to_string(),
            })?;
        let entries = parse_hosts_listing(&raw);
        let mut presence = HostPresence::new(Arc::clone(&parts.clock));
        for (host_id, record) in &entries {
            presence.observe(host_id, record.clone());
        }
        let paired = paired_hosts(parts.keys.as_ref(), &entries).await;
        Ok(host_views(&entries, &presence, &paired))
    }

    /// Keep the list of computers up to date through `on_hosts` (and re-check
    /// online state on a timer). Replaces an earlier watch.
    pub async fn watch_hosts(&self) {
        let parts = &self.core.parts;
        let task = tokio::spawn(watch_hosts_task(
            parts.uid.clone(),
            Arc::clone(&parts.rtdb),
            Arc::clone(&parts.keys),
            Arc::clone(&parts.clock),
            self.core.out.clone(),
        ));
        if let Some(old) = self
            .core
            .hosts_task
            .lock()
            .expect("hosts task")
            .replace(task)
        {
            old.abort();
        }
    }

    /// Stop `watch_hosts`.
    pub async fn stop_watching_hosts(&self) {
        if let Some(task) = self.core.hosts_task.lock().expect("hosts task").take() {
            task.abort();
        }
    }

    /// Dial a computer, replacing the current connection. States arrive through
    /// `on_connection_state`; the first time, `Pairing` carries the code to compare
    /// with the desktop's dialog. Dialling the computer that is already connected
    /// only nudges it.
    pub async fn connect(&self, host_id: String) -> Result<(), RemoteFfiError> {
        if host_id.is_empty() {
            return Err(RemoteFfiError::InvalidArgument {
                message: "host_id is empty".into(),
            });
        }
        let same = {
            let active = self.core.active.lock().expect("active");
            match active.as_ref() {
                Some(active) if active.host_id == host_id => {
                    let phase = active.connection.state().phase;
                    if phase == ConnectionPhase::Idle {
                        None
                    } else {
                        Some(active.connection.clone())
                    }
                }
                _ => None,
            }
        };
        match same {
            Some(connection) => {
                connection.retry();
                connection.nudge();
                Ok(())
            }
            None => self.start_connection(&host_id),
        }
    }

    /// End the connection: sessions are detached, the link is closed.
    pub async fn disconnect(&self) {
        let (active, current) = {
            let mut active = self.core.active.lock().expect("active");
            let current = active.as_ref().map(|a| a.host_id.clone());
            self.core.current.fetch_add(1, Ordering::SeqCst);
            (active.take(), current)
        };
        if let Some(active) = active {
            active.shutdown();
        }
        if let Some(host_id) = current {
            let _ = self.core.out.send(Out::State(RemoteConnectionStateFfi {
                host_id,
                phase: RemoteConnectionPhaseFfi::Idle,
                transport: None,
                pairing_code: None,
                failure: None,
                attempt: 0,
                retry_at_ms: None,
                host: None,
                ever_ready: false,
            }));
        }
    }

    /// The user asks to try again after a failure.
    pub async fn retry(&self) {
        if let Some(active) = self.core.active.lock().expect("active").as_ref() {
            active.connection.retry();
        }
    }

    /// The app returned to the foreground or the network came back: skip the rest
    /// of a backoff, and revive a connection that gave up on its own.
    pub async fn nudge(&self) {
        if let Some(active) = self.core.active.lock().expect("active").as_ref() {
            active.connection.nudge();
        }
    }

    /// The current state of the connection, if a computer is being dialled.
    pub fn connection_state(&self) -> Option<RemoteConnectionStateFfi> {
        self.core
            .active
            .lock()
            .expect("active")
            .as_ref()
            .map(|active| connection_state_to_ffi(&active.connection.state()))
    }

    /// The current computer's shared sessions; `None` until the first list arrives.
    pub fn sessions(&self) -> Option<Vec<RemoteSessionSummaryFfi>> {
        let client = self.client().ok()?;
        client
            .sessions()
            .map(|sessions| sessions.iter().map(session_summary_to_ffi).collect())
    }

    /// Forget a computer: the pin goes, the desktop still lists this device until the
    /// user removes it there. Disconnects first when it is the current one.
    pub async fn forget_host(&self, host_id: String) {
        let is_current = self
            .core
            .active
            .lock()
            .expect("active")
            .as_ref()
            .is_some_and(|active| active.host_id == host_id);
        if is_current {
            self.disconnect().await;
        }
        self.core.parts.keys.unpair(&host_id).await;
        self.core.tunnel_memory.forget(&host_id);
    }

    /// After a `Revoked` failure: make a new device key and pair again.
    pub async fn repair_host(&self, host_id: String) -> Result<(), RemoteFfiError> {
        let connection = self
            .core
            .active
            .lock()
            .expect("active")
            .as_ref()
            .filter(|active| active.host_id == host_id)
            .map(|active| active.connection.clone());
        match connection {
            Some(connection) => {
                connection.repair().await;
                Ok(())
            }
            None => {
                self.core
                    .parts
                    .keys
                    .reset_identity()
                    .await
                    .map_err(|error| RemoteFfiError::KeyStore {
                        message: error.to_string(),
                    })?;
                self.connect(host_id).await
            }
        }
    }

    /// Follow a session: attach now if connected, and again after every reconnect
    /// (resuming from the last position held). Changes arrive through
    /// `on_session_update`, starting with the state held so far.
    pub async fn attach_session(&self, session_id: String) -> Result<(), RemoteFfiError> {
        let mut active = self.core.active.lock().expect("active");
        let active = active.as_mut().ok_or(RemoteFfiError::NotConnected)?;
        let controller = active.client.session(&session_id);
        if let Entry::Vacant(slot) = active.watchers.entry(session_id) {
            let watcher = Arc::new(SessionWatcher {
                out: self.core.out.clone(),
                marker: Mutex::new(SentMarker::new()),
            });
            let id = controller.add_listener(Arc::clone(&watcher) as _);
            slot.insert((Arc::clone(&watcher), id));
            watcher.on_session_state(&controller.state());
        }
        controller.activate();
        Ok(())
    }

    /// Stop following a session; the desktop is told. Its position is kept for a later attach.
    pub async fn detach_session(&self, session_id: String) -> Result<(), RemoteFfiError> {
        let mut active = self.core.active.lock().expect("active");
        let active = active.as_mut().ok_or(RemoteFfiError::NotConnected)?;
        let controller = active.client.session(&session_id);
        if let Some((_, id)) = active.watchers.remove(&session_id) {
            controller.remove_listener(id);
        }
        controller.deactivate();
        Ok(())
    }

    /// Tell the desktop whether the screen shows the session (it uses this for notifications).
    pub async fn set_session_foreground(
        &self,
        session_id: String,
        foreground: bool,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?.set_foreground(foreground);
        Ok(())
    }

    /// Fetch the page of messages before the ones held; they arrive as
    /// `older_messages` of a session update.
    pub async fn load_older_messages(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?.load_older().await;
        Ok(())
    }

    /// The bytes of an image a record refers to.
    pub async fn fetch_image(
        &self,
        session_id: String,
        image_ref: String,
    ) -> Result<RemoteImageFfi, RemoteFfiError> {
        let url = self
            .controller(&session_id)?
            .fetch_image(&image_ref)
            .await
            .map_err(rpc_error)?;
        image_from_data_url(&url)
    }

    /// Send a prompt. It shows up in `outgoing` until its own record comes back;
    /// a failure shows there too (see `retry_prompt`). While a turn runs only text is sent.
    pub async fn send_prompt(
        &self,
        session_id: String,
        text: String,
        images: Vec<RemotePromptImageFfi>,
    ) -> Result<(), RemoteFfiError> {
        let images = prompt_images(images)?;
        self.controller(&session_id)?
            .send_prompt(&text, images)
            .await;
        Ok(())
    }

    /// Send a failed prompt again with the same request id.
    pub async fn retry_prompt(
        &self,
        session_id: String,
        prompt_id: String,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?.retry_prompt(&prompt_id).await;
        Ok(())
    }

    /// Drop a prompt from the outgoing list.
    pub async fn dismiss_prompt(
        &self,
        session_id: String,
        prompt_id: String,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?.dismiss_prompt(&prompt_id);
        Ok(())
    }

    /// Cancel the running turn.
    pub async fn cancel_turn(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .cancel_turn()
            .await
            .map_err(rpc_error)
    }

    /// Allow once or deny a permission request.
    pub async fn respond_permission(
        &self,
        session_id: String,
        request_id: u64,
        allow: bool,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .respond_permission(request_id, allow)
            .await
            .map_err(rpc_error)
    }

    /// Answer a question: question text to the chosen label(s) (several joined by ", ").
    pub async fn answer_question(
        &self,
        session_id: String,
        request_id: u64,
        answers: HashMap<String, String>,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .answer_question(request_id, answers.into_iter().collect::<BTreeMap<_, _>>())
            .await
            .map_err(rpc_error)
    }

    /// Dismiss a question.
    pub async fn cancel_question(
        &self,
        session_id: String,
        request_id: u64,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .cancel_question(request_id)
            .await
            .map_err(rpc_error)
    }

    /// Change the session's model.
    pub async fn set_model(&self, session_id: String, model: String) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .set_model(&model)
            .await
            .map_err(rpc_error)
    }

    /// Change the session's reasoning.
    pub async fn set_reasoning(
        &self,
        session_id: String,
        selection: ReasoningSelectionDto,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .set_reasoning(&selection)
            .await
            .map_err(rpc_error)
    }

    /// Change the session's permission mode.
    pub async fn set_permission_mode(
        &self,
        session_id: String,
        mode: RemotePermissionModeFfi,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .set_permission_mode(permission_mode(mode))
            .await
            .map_err(rpc_error)
    }

    /// The models the desktop offers (asked once per connection).
    pub async fn list_models(&self) -> Result<RemoteModelsFfi, RemoteFfiError> {
        let models = self.client()?.models().await.map_err(rpc_error)?;
        Ok(RemoteModelsFfi {
            models: models.models,
            current: models.current,
            details: models.details.unwrap_or_default(),
        })
    }
}

async fn watch_hosts_task(
    uid: String,
    rtdb: Arc<dyn Rtdb>,
    keys: Arc<dyn ViewerKeys>,
    clock: Arc<dyn Clock>,
    out: OutSender,
) {
    let path = match hosts_path(&uid) {
        Ok(path) => path,
        Err(error) => {
            let _ = out.send(Out::HostsFailed(error.to_string()));
            return;
        }
    };
    let mut listener = rtdb.listen(&path);
    let mut mirror = RtdbMirror::new();
    let mut presence = HostPresence::new(clock);
    let mut entries: Vec<(String, HostRecord)> = Vec::new();
    let mut last: Option<Vec<RemoteHostFfi>> = None;
    // Hosts that die produce no events, so online state is re-checked on the heartbeat period.
    let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            item = listener.recv() => match item {
                Some(ListenItem::Event(event)) => {
                    mirror.apply(&event);
                    let before: HashSet<String> = entries.iter().map(|(id, _)| id.clone()).collect();
                    entries = parse_hosts_listing(&mirror.get("/"));
                    let now: HashSet<&String> = entries.iter().map(|(id, _)| id).collect();
                    for gone in before.iter().filter(|id| !now.contains(id)) {
                        presence.forget(gone);
                    }
                    for (host_id, record) in &entries {
                        presence.observe(host_id, record.clone());
                    }
                }
                Some(ListenItem::Error(error)) => {
                    let _ = out.send(Out::HostsFailed(format!("{error:?}")));
                    return;
                }
                None => return,
            },
            _ = tick.tick() => {}
        }
        let paired = paired_hosts(keys.as_ref(), &entries).await;
        let hosts = host_views(&entries, &presence, &paired);
        if last.as_ref() != Some(&hosts) {
            last = Some(hosts.clone());
            let _ = out.send(Out::Hosts(hosts));
        }
    }
}

#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;
