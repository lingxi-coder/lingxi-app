//! Canonical shared Remote Control implementation for the mobile UniFFI components.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{
    NativeMediaHost, PlatformRemoteBotsListener, PlatformRemoteKeyStore, PlatformRemoteListener,
    PlatformRemoteMediaConfigProvider, PlatformRemoteTokenProvider, REMOTE_DEVICE_PLATFORM,
};
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
use remote_control_client::files::{FileOffer, FileReceipt, FileTask};
use remote_control_client::keys::IdentityKey;
use remote_control_client::media::{
    RemoteIceServer, RemoteMediaConfig, RemoteMediaConfigProvider, RemoteMediaConnection,
    RemoteMediaIntent, RemoteMediaMode, RemoteMediaOffer,
};
use remote_control_client::messages::{
    PermissionModeId, RemoteBotStatus, RemoteBotSummary, RemoteImageRef, RemotePending,
    RemotePendingAction, RemotePromptOrigin, RemoteRecord, RemoteSeqRecord, RemoteSessionStatus,
    RemoteSessionSummary, RemoteTurnState,
};
use remote_control_client::native_media::NativeMediaOfferHandler;
use remote_control_client::peer::PeerConnector;
use remote_control_client::rpc::RpcError;
use remote_control_client::rtdb::{
    ListenItem, Rtdb, RtdbClient, RtdbClientOptions, RtdbEventKind, RtdbMirror, TokenError,
    TokenProvider,
};
use remote_control_client::session::{
    BotThreadsState, BotsState, OutgoingPrompt, OutgoingStatus, RemoteClient, RemoteClientListener,
    SessionController, SessionListener, SessionState, SessionStatus,
};
use remote_control_client::signaling::{
    hosts_path, parse_hosts_listing, HostPresence, HostRecord, TransportKind,
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

/// Media mode requested by the authenticated host. Negotiation SDP stays in Rust.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum RemoteMediaModeFfi {
    Audio,
    Video,
    AudioVideo,
}

/// Whether the session is passive observation or active conversation input.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum RemoteMediaIntentFfi {
    Observation,
    Conversation,
}

/// Verified connection and host-issued references used to choose ICE policy.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RemoteMediaConfigRequestFfi {
    pub account_uid: String,
    pub host_id: String,
    pub device_id: String,
    pub connection_id: String,
    pub session_id: String,
    pub media_session_ref: String,
    pub generation: u64,
    pub mode: RemoteMediaModeFfi,
    pub intent: RemoteMediaIntentFfi,
    pub audio_source_ref: Option<String>,
    pub video_source_ref: Option<String>,
}

/// Product-selected STUN/TURN server. Debug output redacts credentials.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteIceServerFfi {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

impl std::fmt::Debug for RemoteIceServerFfi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteIceServerFfi")
            .field("url_count", &self.urls.len())
            .field("has_username", &self.username.is_some())
            .field("has_credential", &self.credential.is_some())
            .finish()
    }
}

/// Per-session ICE and transport policy, frozen before native peer creation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteMediaConfigFfi {
    pub ice_servers: Vec<RemoteIceServerFfi>,
    pub relay_only: bool,
    pub gather_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub max_payload_bytes: u64,
}

/// Product defaults for cross-device media; callers may add TURN credentials
/// from their current Firebase/app settings before supplying the provider.
#[uniffi::export]
pub fn default_remote_media_config() -> RemoteMediaConfigFfi {
    RemoteMediaConfigFfi {
        ice_servers: remote_control_client::version::DEFAULT_STUN_URLS
            .iter()
            .map(|url| RemoteIceServerFfi {
                urls: vec![(*url).to_owned()],
                username: None,
                credential: None,
            })
            .collect(),
        relay_only: false,
        gather_timeout_ms: 3_000,
        connect_timeout_ms: 15_000,
        max_payload_bytes: remote_control_client::media::MEDIA_PAYLOAD_MAX_BYTES,
    }
}

struct MediaConfigProviderBridge {
    inner: Box<dyn PlatformRemoteMediaConfigProvider>,
}

#[async_trait]
impl RemoteMediaConfigProvider for MediaConfigProviderBridge {
    async fn resolve(
        &self,
        connection: &RemoteMediaConnection,
        offer: &RemoteMediaOffer,
    ) -> Result<RemoteMediaConfig, RpcError> {
        let request = RemoteMediaConfigRequestFfi {
            account_uid: connection.account_uid.clone(),
            host_id: connection.host_id.clone(),
            device_id: connection.device_id.clone(),
            connection_id: connection.connection_id.clone(),
            session_id: offer.session_id.clone(),
            media_session_ref: offer.media_session_ref.clone(),
            generation: offer.generation,
            mode: match offer.mode {
                RemoteMediaMode::Audio => RemoteMediaModeFfi::Audio,
                RemoteMediaMode::Video => RemoteMediaModeFfi::Video,
                RemoteMediaMode::AudioVideo => RemoteMediaModeFfi::AudioVideo,
            },
            intent: match offer.intent {
                RemoteMediaIntent::Observation => RemoteMediaIntentFfi::Observation,
                RemoteMediaIntent::Conversation => RemoteMediaIntentFfi::Conversation,
            },
            audio_source_ref: offer.audio_source_ref.clone(),
            video_source_ref: offer.video_source_ref.clone(),
        };
        let config = self.inner.resolve(request).await.map_err(|_| {
            RpcError::new(
                remote_control_client::rpc::RemoteErrorCode::Internal,
                "media transport configuration unavailable",
            )
        })?;
        let config = RemoteMediaConfig {
            ice_servers: config
                .ice_servers
                .into_iter()
                .map(|server| RemoteIceServer {
                    urls: server.urls,
                    username: server.username,
                    credential: server.credential,
                })
                .collect(),
            relay_only: config.relay_only,
            gather_timeout_ms: config.gather_timeout_ms,
            connect_timeout_ms: config.connect_timeout_ms,
            max_payload_bytes: config.max_payload_bytes,
        };
        config.validate()?;
        Ok(config)
    }
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

struct TokenBridge {
    inner: Box<dyn PlatformRemoteTokenProvider>,
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

/// Adapts the app's [`PlatformRemoteKeyStore`] to the client's [`ViewerKeys`]. What
/// was read or written is also kept in memory: the client asks for the identity
/// and the pins often, and a pin the store failed to persist still holds for this run.
struct KeyStoreBridge {
    inner: Box<dyn PlatformRemoteKeyStore>,
    identity: tokio::sync::Mutex<Option<Arc<IdentityKey>>>,
    pins: Mutex<HashMap<String, PinnedHost>>,
}

impl KeyStoreBridge {
    fn new(inner: Box<dyn PlatformRemoteKeyStore>) -> Self {
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

/// Where a Bot stands, derived from its chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteBotStatusFfi {
    /// No chat is running or waiting.
    Idle,
    /// A chat is running a turn.
    Running,
    /// A chat is waiting for an answer.
    RequiresAction,
}

/// One Bot of the computer whose Remote Control switch is on.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteBotSummaryFfi {
    /// Bot id, for `activate_bot_threads` and `create_bot_thread`.
    pub bot_id: String,
    /// Display name, cleaned for one line.
    pub name: String,
    /// What the Bot is for, cleaned and cut.
    pub description: String,
    /// Status.
    pub status: RemoteBotStatusFfi,
    /// Chats waiting for an answer right now.
    pub waiting_threads: u32,
    /// Epoch milliseconds of the Bot's latest chat activity.
    pub last_activity_at_ms: Option<u64>,
}

/// What the app knows of the computer's Bots.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteBotsStateFfi {
    /// The computer has not answered yet (or none is connected).
    Unknown,
    /// The computer is a version from before Bots, so there is none to show. It is
    /// asked again after every reconnect.
    Unsupported,
    /// The Bots whose Remote Control switch is on, as the computer sent them last.
    Bots {
        /// In the computer's order.
        bots: Vec<RemoteBotSummaryFfi>,
    },
}

/// What the app holds of one Bot's chats.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct RemoteBotThreadsStateFfi {
    /// The Bot's chats, newest first, as listed last; `None` until they have been.
    /// Follow one with `attach_session`.
    pub threads: Option<Vec<RemoteSessionSummaryFfi>>,
    /// Why the last listing failed; the next one that works clears it. The codes
    /// `unknown_session` and `not_remote_enabled` say that the Bot cannot be reached.
    pub error: Option<RemoteSessionErrorFfi>,
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

fn bot_summary_to_ffi(bot: &RemoteBotSummary) -> RemoteBotSummaryFfi {
    RemoteBotSummaryFfi {
        bot_id: bot.bot_id.clone(),
        name: bot.name.clone(),
        description: bot.description.clone(),
        status: match bot.status {
            RemoteBotStatus::Idle => RemoteBotStatusFfi::Idle,
            RemoteBotStatus::Running => RemoteBotStatusFfi::Running,
            RemoteBotStatus::RequiresAction => RemoteBotStatusFfi::RequiresAction,
        },
        waiting_threads: bot.waiting_threads,
        last_activity_at_ms: bot.last_activity_at,
    }
}

fn bots_state_to_ffi(state: &BotsState) -> RemoteBotsStateFfi {
    match state {
        BotsState::Unknown => RemoteBotsStateFfi::Unknown,
        BotsState::Unsupported => RemoteBotsStateFfi::Unsupported,
        BotsState::Bots(bots) => RemoteBotsStateFfi::Bots {
            bots: bots.iter().map(bot_summary_to_ffi).collect(),
        },
    }
}

fn bot_threads_state_to_ffi(state: &BotThreadsState) -> RemoteBotThreadsStateFfi {
    RemoteBotThreadsStateFfi {
        threads: state
            .threads
            .as_ref()
            .map(|threads| threads.iter().map(session_summary_to_ffi).collect()),
        error: state.error.as_ref().map(|error| RemoteSessionErrorFfi {
            code: error.code.clone(),
            message: error.message.clone(),
        }),
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
            ..
        } => RemoteRecordFfi::Prompt {
            turn_id: *turn_id,
            text: text.clone(),
            images: images.iter().map(image_ref_to_ffi).collect(),
            origin: match origin {
                RemotePromptOrigin::Desktop => RemotePromptOriginFfi::Desktop,
                RemotePromptOrigin::Device => RemotePromptOriginFfi::Device,
                RemotePromptOrigin::Scheduled | RemotePromptOrigin::Bot => RemotePromptOriginFfi::Scheduled,
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
    State(u64, RemoteConnectionStateFfi),
    Hosts(Vec<RemoteHostFfi>),
    HostsFailed(String),
    Sessions(u64, String, Vec<RemoteSessionSummaryFfi>),
    Bots(u64, String, RemoteBotsStateFfi),
    BotThreads(u64, String, String, RemoteBotThreadsStateFfi),
    Session(u64, Arc<AtomicBool>, Box<RemoteSessionUpdateFfi>),
}

type OutSender = mpsc::UnboundedSender<Out>;

/// Where the app's Bots listener is kept: empty until `set_bots_listener`, and read
/// by the delivery task for every Bots call.
type BotsListenerSlot = Arc<Mutex<Option<Arc<dyn PlatformRemoteBotsListener>>>>;

/// Forwards a connection's state while it is the current one.
struct StateRelay {
    out: OutSender,
    current: Arc<AtomicU64>,
    generation: u64,
}

impl ConnectionListener for StateRelay {
    fn on_state(&self, state: &ConnectionState) {
        if self.current.load(Ordering::SeqCst) == self.generation {
            let _ = self
                .out
                .send(Out::State(self.generation, connection_state_to_ffi(state)));
        }
    }
}

/// Forwards the session list, the Bots and the chats of the Bots while its
/// connection is the current one.
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
                self.generation,
                self.host_id.clone(),
                sessions.iter().map(session_summary_to_ffi).collect(),
            ));
        }
    }

    fn on_bots(&self, state: &BotsState) {
        if self.current.load(Ordering::SeqCst) == self.generation {
            let _ = self.out.send(Out::Bots(
                self.generation,
                self.host_id.clone(),
                bots_state_to_ffi(state),
            ));
        }
    }

    fn on_bot_threads(&self, bot_id: &str, state: &BotThreadsState) {
        if self.current.load(Ordering::SeqCst) == self.generation {
            let _ = self.out.send(Out::BotThreads(
                self.generation,
                self.host_id.clone(),
                bot_id.to_owned(),
                bot_threads_state_to_ffi(state),
            ));
        }
    }
}

/// Turns a followed session's states into deltas for the app.
struct SessionWatcher {
    out: OutSender,
    generation: u64,
    attached: Arc<AtomicBool>,
    marker: Mutex<SentMarker>,
}

impl SessionListener for SessionWatcher {
    fn on_session_state(&self, state: &SessionState) {
        if !self.attached.load(Ordering::SeqCst) {
            return;
        }
        // Computed and queued under one lock, so deltas leave in the order of the states.
        let mut marker = self.marker.lock().expect("marker");
        let update = session_update(&mut marker, state);
        let _ = self.out.send(Out::Session(
            self.generation,
            Arc::clone(&self.attached),
            Box::new(update),
        ));
    }
}

async fn deliver(
    mut queue: mpsc::UnboundedReceiver<Out>,
    listener: Arc<dyn PlatformRemoteListener>,
    current: Arc<AtomicU64>,
    bots: BotsListenerSlot,
) {
    while let Some(item) = queue.recv().await {
        match item {
            Out::State(generation, state) if generation == current.load(Ordering::SeqCst) => {
                listener.on_connection_state(state).await;
            }
            Out::Hosts(hosts) => listener.on_hosts(hosts).await,
            Out::HostsFailed(message) => listener.on_hosts_failed(message).await,
            Out::Sessions(generation, host_id, sessions)
                if generation == current.load(Ordering::SeqCst) =>
            {
                listener.on_sessions(host_id, sessions).await;
            }
            Out::Bots(generation, host_id, state)
                if generation == current.load(Ordering::SeqCst) =>
            {
                // Cloned out so that the slot is not held across the call.
                let heard = bots.lock().expect("bots listener").clone();
                if let Some(heard) = heard {
                    heard.on_bots(host_id, state).await;
                }
            }
            Out::BotThreads(generation, host_id, bot_id, state)
                if generation == current.load(Ordering::SeqCst) =>
            {
                let heard = bots.lock().expect("bots listener").clone();
                if let Some(heard) = heard {
                    heard.on_bot_threads(host_id, bot_id, state).await;
                }
            }
            Out::Session(generation, attached, update)
                if generation == current.load(Ordering::SeqCst)
                    && attached.load(Ordering::SeqCst) =>
            {
                listener.on_session_update(*update).await;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// Both mobile viewers share the data-only Harness WebRTC adapter.
fn webrtc_peer_connector() -> Option<Arc<dyn PeerConnector>> {
    Some(Arc::new(
        remote_control_client::native_peer::NativePeerConnector,
    ))
}

fn native_media_offer_handler(
    host: Arc<NativeMediaHost>,
    provider: Box<dyn PlatformRemoteMediaConfigProvider>,
) -> Option<Arc<dyn remote_control_client::media::RemoteMediaOfferHandler>> {
    let provider = Arc::new(MediaConfigProviderBridge { inner: provider });
    Some(NativeMediaOfferHandler::new(
        host.media_service(),
        host.device_ports(),
        provider,
    ))
}

/// What a client is made of; tests substitute the database and the clock.
struct Parts {
    uid: String,
    rtdb: Arc<dyn Rtdb>,
    keys: Arc<dyn ViewerKeys>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
    peers: Option<Arc<dyn PeerConnector>>,
    media_offer_handler: Option<Arc<dyn remote_control_client::media::RemoteMediaOfferHandler>>,
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
        for (watcher, _) in self.watchers.values() {
            watcher.attached.store(false, Ordering::SeqCst);
        }
        self.client.remove_listener(self.sessions_listener);
        self.client.dispose();
        self.connection.stop();
    }
}

/// A Local App shared by the connected computer.
#[derive(Clone, uniffi::Record)]
pub struct RemoteLocalAppFfi {
    pub app_id: String,
    pub name: String,
    pub summary: String,
    pub revision: u64,
    pub phase: String,
    pub background: bool,
    pub execution_host: String,
    pub remote_enabled: bool,
}

/// A short-lived browser bootstrap. Do not persist or log this value.
#[derive(Clone, uniffi::Record)]
pub struct RemoteLocalAppOpenFfi {
    pub app_id: String,
    pub revision: u64,
    pub url: String,
    pub expires_at: u64,
}

/// Metadata and receipt of an arbitrary original-file attachment.
#[derive(Clone, uniffi::Record)]
pub struct RemoteFileFfi {
    pub transfer_id: String,
    pub session_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub file_ref: String,
}
impl From<FileReceipt> for RemoteFileFfi {
    fn from(value: FileReceipt) -> Self {
        Self {
            transfer_id: value.offer.transfer_id,
            session_id: value.offer.session_id,
            name: value.offer.name,
            size: value.offer.size,
            sha256: value.offer.sha256,
            file_ref: value.r#ref,
        }
    }
}
impl RemoteFileFfi {
    fn offer(self) -> FileOffer {
        FileOffer {
            transfer_id: self.transfer_id,
            session_id: self.session_id,
            name: self.name,
            size: self.size,
            sha256: self.sha256,
        }
    }
}
fn file_error(message: String) -> RemoteFfiError {
    RemoteFfiError::InvalidArgument { message }
}

struct Core {
    runtime: tokio::runtime::Handle,
    parts: Parts,
    out: OutSender,
    tunnel_memory: Arc<TunnelMemory>,
    push_installation_id: Mutex<Option<String>>,
    /// Bumped whenever the current connection is replaced or ended; relays of an
    /// older connection go quiet.
    current: Arc<AtomicU64>,
    active: Mutex<Option<Active>>,
    hosts_task: Mutex<Option<JoinHandle<()>>>,
    bots_listener: BotsListenerSlot,
    delivery: JoinHandle<()>,
    repairing: tokio::sync::Mutex<()>,
    file_tasks: Mutex<HashMap<String, Arc<FileTask>>>,
}

impl Drop for Core {
    fn drop(&mut self) {
        // UniFFI frees objects synchronously on the foreign caller's thread.
        // Session/tunnel shutdown may spawn async cleanup even from deinit.
        let _runtime = self.runtime.enter();
        self.delivery.abort();
        for task in self.file_tasks.lock().expect("file tasks").values() {
            task.cancel.cancel();
        }
        if let Some(task) = self.hosts_task.lock().expect("hosts task").take() {
            task.abort();
        }
        if let Some(active) = self.active.lock().expect("active").take() {
            active.shutdown();
        }
    }
}

/// The Remote Control viewer state shared by the platform binding objects.
pub(super) struct RemoteControlCore {
    core: Core,
}

/// Build the client. `database_url` is the Realtime Database instance
/// (`https://lingxi-5e4b2-default-rtdb.firebaseio.com`), `uid` the Firebase uid
/// the token belongs to; `device_name` is shown next to the pairing code on the desktop.
pub(super) async fn build_remote_control_core(
    database_url: String,
    uid: String,
    device_name: String,
    app_version: String,
    tokens: Box<dyn PlatformRemoteTokenProvider>,
    keys: Box<dyn PlatformRemoteKeyStore>,
    listener: Box<dyn PlatformRemoteListener>,
    media_host: Arc<NativeMediaHost>,
    media_config_provider: Box<dyn PlatformRemoteMediaConfigProvider>,
) -> Result<RemoteControlCore, RemoteFfiError> {
    if uid.is_empty() || database_url.is_empty() {
        return Err(RemoteFfiError::InvalidArgument {
            message: "database_url and uid are required".into(),
        });
    }
    let rtdb = RtdbClient::new(RtdbClientOptions::new(
        database_url,
        Arc::new(TokenBridge { inner: tokens }),
    ));
    let media_offer_handler = native_media_offer_handler(media_host, media_config_provider);
    let parts = Parts {
        uid,
        rtdb: Arc::new(rtdb),
        keys: Arc::new(KeyStoreBridge::new(keys)),
        clock: system_clock(),
        random: Arc::new(random_unit),
        peers: webrtc_peer_connector(),
        media_offer_handler,
        device_name,
        app_version,
    };
    Ok(RemoteControlCore::from_parts(parts, Arc::from(listener)))
}

impl RemoteControlCore {
    fn file_connection(&self) -> Result<ViewerConnection, RemoteFfiError> {
        self.core
            .active
            .lock()
            .expect("active")
            .as_ref()
            .map(|a| a.connection.clone())
            .ok_or(RemoteFfiError::NotConnected)
    }
    fn begin_file_task(&self, id: &str) -> Result<Arc<FileTask>, RemoteFfiError> {
        let mut tasks = self.core.file_tasks.lock().expect("file tasks");
        if tasks
            .values()
            .filter(|t| t.active.load(Ordering::Acquire))
            .count()
            >= 4
            || tasks
                .get(id)
                .is_some_and(|t| t.active.load(Ordering::Acquire))
        {
            return Err(file_error("file transfer busy".into()));
        }
        if tasks.len() >= 32 {
            tasks.retain(|_, t| t.active.load(Ordering::Acquire));
        }
        let task = Arc::new(FileTask::default());
        task.active.store(true, Ordering::Release);
        tasks.insert(id.to_owned(), task.clone());
        Ok(task)
    }

    /// Must be called inside a Tokio runtime.
    fn from_parts(parts: Parts, listener: Arc<dyn PlatformRemoteListener>) -> Self {
        let (out, queue) = mpsc::unbounded_channel();
        let current = Arc::new(AtomicU64::new(0));
        let bots_listener = BotsListenerSlot::default();
        let tunnel_memory = Arc::new(TunnelMemory::new(Arc::clone(&parts.clock)));
        Self {
            core: Core {
                runtime: tokio::runtime::Handle::current(),
                parts,
                out,
                tunnel_memory,
                push_installation_id: Mutex::new(None),
                current: Arc::clone(&current),
                active: Mutex::new(None),
                hosts_task: Mutex::new(None),
                bots_listener: Arc::clone(&bots_listener),
                delivery: tokio::spawn(deliver(queue, listener, current, bots_listener)),
                repairing: tokio::sync::Mutex::new(()),
                file_tasks: Mutex::new(HashMap::new()),
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
        self.start_connection_if_current(host_id, None)
    }

    fn start_connection_if_current(
        &self,
        host_id: &str,
        expected: Option<u64>,
    ) -> Result<(), RemoteFfiError> {
        let core = &self.core;
        let parts = &core.parts;
        // Generation and active installation are one transaction: disconnect
        // cannot finish between them and be undone by a stale constructor.
        let mut active = core.active.lock().expect("active");
        if expected.is_some_and(|generation| generation != core.current.load(Ordering::SeqCst)) {
            return Ok(());
        }
        let generation = core.current.fetch_add(1, Ordering::SeqCst) + 1;
        let mut deps = ViewerConnectionDeps::new(
            parts.uid.clone(),
            host_id,
            Arc::clone(&parts.rtdb),
            Arc::clone(&parts.keys),
            REMOTE_DEVICE_PLATFORM,
        );
        deps.clock = Arc::clone(&parts.clock);
        deps.random = Arc::clone(&parts.random);
        deps.peers = parts.peers.clone();
        deps.media_offer_handler = parts.media_offer_handler.clone();
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
        let previous = active.replace(Active {
            host_id: host_id.to_owned(),
            connection,
            client,
            sessions_listener,
            watchers: HashMap::new(),
        });
        drop(active);
        if let Some(previous) = previous {
            previous.shutdown();
        }
        Ok(())
    }
}

impl RemoteControlCore {
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
        let (active, current, generation) = {
            let mut active = self.core.active.lock().expect("active");
            let current = active.as_ref().map(|a| a.host_id.clone());
            let generation = self.core.current.fetch_add(1, Ordering::SeqCst) + 1;
            (active.take(), current, generation)
        };
        if let Some(active) = active {
            active.shutdown();
        }
        if let Some(host_id) = current {
            let _ = self.core.out.send(Out::State(
                generation,
                RemoteConnectionStateFfi {
                    host_id,
                    phase: RemoteConnectionPhaseFfi::Idle,
                    transport: None,
                    pairing_code: None,
                    failure: None,
                    attempt: 0,
                    retry_at_ms: None,
                    host: None,
                    ever_ready: false,
                },
            ));
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

    /// List Local Apps shared by the connected computer.
    pub async fn list_apps(&self) -> Result<Vec<RemoteLocalAppFfi>, RemoteFfiError> {
        let (connection, generation) = {
            let active = self.core.active.lock().expect("active");
            let active = active.as_ref().ok_or(RemoteFfiError::NotConnected)?;
            (
                active.connection.clone(),
                self.core.current.load(Ordering::SeqCst),
            )
        };
        let apps = connection.list_apps().await.map_err(rpc_error)?;
        if generation != self.core.current.load(Ordering::SeqCst)
            || connection.state().phase != ConnectionPhase::Ready
        {
            return Err(RemoteFfiError::NotConnected);
        }
        Ok(apps
            .into_iter()
            .map(|app| RemoteLocalAppFfi {
                app_id: app.app_id,
                name: app.name,
                summary: app.summary,
                revision: app.revision,
                phase: app.phase,
                background: app.background,
                execution_host: app.execution_host,
                remote_enabled: app.remote_enabled,
            })
            .collect())
    }

    /// Obtain a short-lived browser bootstrap from the connected computer.
    pub async fn open_app(&self, app_id: String) -> Result<RemoteLocalAppOpenFfi, RemoteFfiError> {
        let (connection, generation) = {
            let active = self.core.active.lock().expect("active");
            let active = active.as_ref().ok_or(RemoteFfiError::NotConnected)?;
            (
                active.connection.clone(),
                self.core.current.load(Ordering::SeqCst),
            )
        };
        let app = connection.open_app(&app_id).await.map_err(rpc_error)?;
        if generation != self.core.current.load(Ordering::SeqCst)
            || connection.state().phase != ConnectionPhase::Ready
        {
            return Err(RemoteFfiError::NotConnected);
        }
        Ok(RemoteLocalAppOpenFfi {
            app_id: app.app_id,
            revision: app.revision,
            url: app.url,
            expires_at: app.expires_at,
        })
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
        if host_id.is_empty() {
            return Err(RemoteFfiError::InvalidArgument {
                message: "host_id is empty".into(),
            });
        }
        let Ok(_repair) = self.core.repairing.try_lock() else {
            return Ok(());
        };
        let generation = self.core.current.load(Ordering::SeqCst);
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
                self.start_connection_if_current(&host_id, Some(generation))
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
                generation: self.core.current.load(Ordering::SeqCst),
                attached: Arc::new(AtomicBool::new(true)),
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
        if let Some((watcher, id)) = active.watchers.remove(&session_id) {
            watcher.attached.store(false, Ordering::SeqCst);
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

    /// Hash an original file without loading it into memory.
    pub async fn prepare_file(
        &self,
        session_id: String,
        path: String,
        name: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        let task = self.begin_file_task(&path)?;
        let offer = task
            .run(
                &self.core.runtime,
                remote_control_client::files::prepare(
                    session_id,
                    path.into(),
                    name,
                    task.cancel.clone(),
                ),
            )
            .await
            .map_err(file_error)?;
        Ok(FileReceipt {
            offer,
            r#ref: String::new(),
        }
        .into())
    }
    /// Stream original bytes on the independent file DataChannel.
    pub async fn upload_file(
        &self,
        file: RemoteFileFfi,
        path: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        let connection = self.file_connection()?;
        let task = self.begin_file_task(&file.transfer_id)?;
        task.run(
            &self.core.runtime,
            remote_control_client::files::upload(
                connection,
                file.offer(),
                path.into(),
                task.cancel.clone(),
                task.progress.clone(),
            ),
        )
        .await
        .map(Into::into)
        .map_err(file_error)
    }
    /// Published files this account/device may access in the attached session.
    pub async fn list_files(
        &self,
        session_id: String,
    ) -> Result<Vec<RemoteFileFfi>, RemoteFfiError> {
        let value = self
            .file_connection()?
            .request(
                "file.list",
                Some(serde_json::json!({"session_id":session_id})),
                None,
            )
            .await
            .map_err(rpc_error)?;
        let files: Vec<FileReceipt> =
            serde_json::from_value(value.get("files").cloned().unwrap_or_default())
                .map_err(|e| file_error(e.to_string()))?;
        for file in &files {
            file.offer.validate().map_err(file_error)?;
            if file.r#ref != file.offer.transfer_id || file.offer.session_id != session_id {
                return Err(file_error("invalid published file reference".into()));
            }
        }
        Ok(files.into_iter().map(Into::into).collect())
    }
    /// Save a verified receipt to an app-selected destination; interrupted files resume.
    pub async fn download_file(
        &self,
        file: RemoteFileFfi,
        path: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        let connection = self.file_connection()?;
        let task = self.begin_file_task(&file.transfer_id)?;
        let file_ref = file.file_ref.clone();
        task.run(
            &self.core.runtime,
            remote_control_client::files::download(
                connection,
                FileReceipt {
                    offer: file.offer(),
                    r#ref: file_ref,
                },
                path.into(),
                task.cancel.clone(),
                task.progress.clone(),
            ),
        )
        .await
        .map(Into::into)
        .map_err(file_error)
    }
    /// Bytes durably acknowledged by the receiving device.
    pub fn file_progress(&self, transfer_id: String) -> u64 {
        self.core
            .file_tasks
            .lock()
            .expect("file tasks")
            .get(&transfer_id)
            .map_or(0, |t| t.progress.load(Ordering::Acquire))
    }
    /// Cancel an active operation. Network errors retain resumable checkpoints.
    pub fn cancel_file(&self, transfer_id: String) {
        if let Some(t) = self
            .core
            .file_tasks
            .lock()
            .expect("file tasks")
            .get(&transfer_id)
        {
            t.cancel.cancel();
        }
    }
    /// Attach only published receipts; desktop validates ownership and session.
    pub async fn send_prompt_with_files(
        &self,
        session_id: String,
        text: String,
        images: Vec<RemotePromptImageFfi>,
        file_refs: Vec<String>,
    ) -> Result<(), RemoteFfiError> {
        self.controller(&session_id)?
            .send_prompt_with_files(&text, prompt_images(images)?, file_refs)
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

    /// Where the Bots and their chats are reported; `None` stops the reports. Set it
    /// before reading `bots_state` or `bot_threads_state`: what changes afterwards is
    /// reported, and what changed before is in those. It stays across connections.
    pub fn set_bots_listener(&self, listener: Option<Box<dyn PlatformRemoteBotsListener>>) {
        *self.core.bots_listener.lock().expect("bots listener") = listener.map(Arc::from);
    }

    /// The current computer's Bots; `Unknown` until it has answered, and while no
    /// computer is connected.
    pub fn bots_state(&self) -> RemoteBotsStateFfi {
        self.client()
            .map(|client| bots_state_to_ffi(&client.bots()))
            .unwrap_or(RemoteBotsStateFfi::Unknown)
    }

    /// What is held of a Bot's chats: nothing yet unless `activate_bot_threads` has
    /// listed them on this connection, and nothing while no computer is connected.
    pub fn bot_threads_state(&self, bot_id: String) -> RemoteBotThreadsStateFfi {
        self.client()
            .map(|client| bot_threads_state_to_ffi(&client.bot_threads(&bot_id).state()))
            .unwrap_or_default()
    }

    /// A screen shows a Bot's chats: list them now, and keep them current (after each
    /// reconnect, and when the Bot's activity changes) until the screen calls
    /// `deactivate_bot_threads`. Each call is one screen; they count. The lists
    /// arrive through `on_bot_threads`. Screens are counted per connection: when the
    /// connection is replaced, activate again.
    pub async fn activate_bot_threads(&self, bot_id: String) -> Result<(), RemoteFfiError> {
        if bot_id.is_empty() {
            return Err(RemoteFfiError::InvalidArgument {
                message: "bot_id is empty".into(),
            });
        }
        self.client()?.bot_threads(&bot_id).activate();
        Ok(())
    }

    /// A screen stopped showing a Bot's chats.
    pub fn deactivate_bot_threads(&self, bot_id: String) {
        if let Ok(client) = self.client() {
            client.bot_threads(&bot_id).deactivate();
        }
    }

    /// Start a new chat with a Bot and return its session id, to follow with
    /// `attach_session`. A link that drops with the request on it is waited for, up to
    /// 30 s, and the request repeated once. Fails at once on a computer from before Bots.
    pub async fn create_bot_thread(&self, bot_id: String) -> Result<String, RemoteFfiError> {
        if bot_id.is_empty() {
            return Err(RemoteFfiError::InvalidArgument {
                message: "bot_id is empty".into(),
            });
        }
        self.client()?
            .create_bot_thread(&bot_id)
            .await
            .map_err(rpc_error)
    }

    /// The Bot a chat belongs to, if the computer has listed it under a Bot on this
    /// connection or this device started it.
    pub fn bot_of_thread(&self, session_id: String) -> Option<String> {
        self.client().ok()?.bot_of_thread(&session_id)
    }

    /// The name of the Bot that handed over the prompt of turn `turn_id` of a session
    /// followed on this connection, if the Bot gave one; `None` for every other prompt.
    /// The prompt itself reads as `Scheduled` in its record.
    pub fn prompt_origin_bot_name(&self, session_id: String, turn_id: u64) -> Option<String> {
        self.client().ok()?.prompt_bot_name(&session_id, turn_id)
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
    let mut initialized = false;
    let mut timed_out = false;
    let initial_timeout = tokio::time::sleep(std::time::Duration::from_secs(15));
    tokio::pin!(initial_timeout);
    // Hosts that die produce no events, so online state is re-checked on the heartbeat period.
    let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            item = listener.recv() => match item {
                Some(ListenItem::Event(event)) => {
                    // The first RTDB event is a complete root snapshot. The
                    // interval's immediate first tick is not evidence that
                    // the account has no computers.
                    if !initialized && (event.kind != RtdbEventKind::Put || event.path != "/") {
                        continue;
                    }
                    initialized = true;
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
                None => {
                    let _ = out.send(Out::HostsFailed("host list closed".into()));
                    return;
                },
            },
            _ = tick.tick() => { if !initialized { continue; } },
            _ = &mut initial_timeout, if !initialized && !timed_out => {
                // Show a retryable error without stopping the reconnecting
                // listener; a later initial snapshot clears the error.
                timed_out = true;
                let _ = out.send(Out::HostsFailed("host list timed out".into()));
                continue;
            }
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
