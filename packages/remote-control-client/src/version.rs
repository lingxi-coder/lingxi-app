//! Constants of the `lingxi-rc/1` protocol: identifiers, size limits and every
//! timing the plan fixes. They mirror `packages/bridge-client/src/remote/version.ts`;
//! the vectors in `packages/bridge-client/test/fixtures/remote` are the shared contract.

use std::time::Duration;

/// Protocol identifier used in logs and documentation.
pub const REMOTE_PROTOCOL: &str = "lingxi-rc/1";
/// Major version sent in `initialize.protocol`; a different major is refused.
pub const REMOTE_PROTOCOL_MAJOR: u32 = 1;
/// Major of the `client-protocol` DTO contract, sent as `initialize.client_protocol_version`.
///
/// Derived from the pinned runtime's own `CLIENT_PROTOCOL_VERSION`, so the two cannot drift.
pub fn client_protocol_major() -> u32 {
    client::protocol::version::CLIENT_PROTOCOL_VERSION
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0)
}

/// Label of the reliable, ordered DataChannel that carries RPC fragments.
pub const DATA_CHANNEL_LABEL: &str = "lingxi-agent";

/// `initialize.app.features` value of a viewer that understands Bots: the `bots.*`
/// and `bot.*` methods and the `bot` prompt origin. The host adds none of them to
/// what any other viewer sees.
pub const REMOTE_FEATURE_BOTS: &str = "bots";

/// Largest fragment (kind byte included); one fragment is one DataChannel message.
pub const FRAGMENT_MAX_BYTES: usize = 16384;
/// Fragment size a tunnel sender uses so that three fragments fill one frame.
pub const TUNNEL_FRAGMENT_MAX_BYTES: usize = 16374;
/// Largest reassembled RPC message (UTF-8 bytes of the JSON text).
pub const MESSAGE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Largest tunnel-frame plaintext; plus the 16-byte GCM tag it base64url-encodes to 65536 characters.
pub const TUNNEL_FRAME_PLAINTEXT_MAX: usize = 49136;
/// Largest SDP (characters) accepted in an offer or answer record.
pub const SDP_MAX_CHARS: usize = 16384;
/// Unsent tunnel bytes above which senders wait.
pub const TUNNEL_WRITE_HIGH_WATER_BYTES: usize = 1024 * 1024;
/// Buffered DataChannel bytes above which a sender pauses until drained.
pub const DATA_CHANNEL_BUFFER_HIGH_BYTES: usize = 1024 * 1024;

/// The project's existing default Realtime Database instance (us-central1).
pub const DEFAULT_RTDB_URL: &str = "https://lingxi-5e4b2-default-rtdb.firebaseio.com";
/// `ns` query value the RTDB emulator expects for that instance.
pub const RTDB_EMULATOR_NAMESPACE: &str = "lingxi-5e4b2-default-rtdb";
/// STUN servers, Cloudflare first because the Google one is unreachable in China.
pub const DEFAULT_STUN_URLS: [&str; 2] = [
    "stun:stun.cloudflare.com:3478",
    "stun:stun.l.google.com:19302",
];

/// How often a host PATCHes `last_seen`.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// A host counts as online while its heartbeat is younger than this.
pub const ONLINE_WINDOW_MS: u64 = 90_000;
/// How long a viewer waits for an answer before deleting its offer.
pub const OFFER_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// A host rejects an offer whose `created_at` is further than this from its clock.
pub const OFFER_MAX_AGE_MS: u64 = 5 * 60_000;
/// Longest wait for ICE candidates before offering with what has arrived.
pub const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(3);
/// Time for the DataChannel to open before falling back to the tunnel.
pub const DATA_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// After a direct-connection failure a viewer goes straight to the tunnel for this long.
pub const DIRECT_FAILURE_MEMORY_MS: u64 = 10 * 60_000;
/// Most simultaneous viewer connections a host accepts.
pub const MAX_HOST_CONNECTIONS: usize = 8;

/// DataChannel: send a `ping` this often.
pub const DATA_CHANNEL_PING_INTERVAL: Duration = Duration::from_secs(15);
/// DataChannel: treat the peer as gone after this long without any message.
pub const DATA_CHANNEL_DEAD: Duration = Duration::from_secs(45);
/// Tunnel: send a `ping` this often.
pub const TUNNEL_PING_INTERVAL: Duration = Duration::from_secs(25);
/// Tunnel: treat the peer as gone after this long without any message.
pub const TUNNEL_DEAD: Duration = Duration::from_secs(75);
/// Default timeout of an RPC request.
pub const RPC_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A partly filled tunnel frame is written this long after its first fragment.
pub const TUNNEL_FLUSH_DELAY: Duration = Duration::from_millis(25);
/// Most tunnel frame writes in flight at once.
pub const TUNNEL_MAX_INFLIGHT_WRITES: usize = 4;
/// Processed frames are deleted in one PATCH at least this often.
pub const TUNNEL_DELETE_INTERVAL: Duration = Duration::from_millis(500);
/// Processed frames are deleted as soon as this many are waiting.
pub const TUNNEL_DELETE_BATCH: usize = 32;
/// A missing frame that stays missing this long ends the tunnel (`tunnel_gap`).
pub const TUNNEL_GAP_TIMEOUT: Duration = Duration::from_secs(30);

/// An SSE stream silent for this long is dropped and reconnected.
pub const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Reconnect backoff: first delay.
pub const SSE_BACKOFF_MIN: Duration = Duration::from_secs(1);
/// Reconnect backoff: ceiling.
pub const SSE_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A stream that stayed up this long restarts the backoff at its minimum.
pub const SSE_BACKOFF_RESET: Duration = Duration::from_secs(60);
