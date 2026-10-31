/**
 * Constants of the `lingxi-rc/1` remote-control protocol: wire identifiers,
 * size limits and every timing the plan fixes. All durations are milliseconds.
 *
 * The Rust client (`remote-control-client`) mirrors these values; the vectors
 * in `test/fixtures/remote/*.json` are the shared contract.
 */

import { CLIENT_PROTOCOL_VERSION } from '../protocol.js';

// ─────────────────────────────────────────────────────────────────────────────
// Identity
// ─────────────────────────────────────────────────────────────────────────────

/** Protocol identifier carried in documentation, logs and signing domains. */
export const REMOTE_PROTOCOL = 'lingxi-rc/1';

/** Major version sent in `initialize.protocol`; a different major is refused. */
export const REMOTE_PROTOCOL_MAJOR = 1;

/**
 * Major of the `client-protocol` DTO contract (`ClientEvent` & co.), sent as
 * `initialize.client_protocol_version`. Derived from the SDK's own
 * {@link CLIENT_PROTOCOL_VERSION} so the two cannot drift.
 */
export const CLIENT_PROTOCOL_MAJOR: number = Number.parseInt(CLIENT_PROTOCOL_VERSION.split('.')[0] ?? '', 10);

/** Label of the reliable, ordered DataChannel that carries RPC fragments. */
export const DATA_CHANNEL_LABEL = 'lingxi-agent';

/**
 * `initialize.app.features` value of a viewer that understands Bots: the `bots.*`
 * and `bot.*` methods and the `bot` prompt origin. The host adds none of them to
 * what any other viewer sees.
 */
export const REMOTE_FEATURE_BOTS = 'bots';

// ─────────────────────────────────────────────────────────────────────────────
// Sizes
// ─────────────────────────────────────────────────────────────────────────────

/** Largest fragment (kind byte included); one fragment is one DataChannel message. */
export const FRAGMENT_MAX_BYTES = 16384;

/**
 * Fragment size a tunnel sender should use: three of these, each with its 4-byte
 * length prefix, fill a frame (3 × (4 + 16374) = 49134 ≤ {@link TUNNEL_FRAME_PLAINTEXT_MAX}),
 * whereas full 16 KiB fragments fit only two per frame.
 */
export const TUNNEL_FRAGMENT_MAX_BYTES = 16374;

/** Largest reassembled RPC message (UTF-8 bytes of the JSON text). */
export const MESSAGE_MAX_BYTES = 16 * 1024 * 1024;

/**
 * Largest tunnel-frame plaintext. Plus the 16-byte GCM tag this is exactly
 * 49152 bytes, which base64url-encodes to 65536 characters (the RTDB rule bound).
 */
export const TUNNEL_FRAME_PLAINTEXT_MAX = 49136;

/** Largest SDP (characters) accepted in an offer or answer record. */
export const SDP_MAX_CHARS = 16384;

/** Unsent tunnel bytes (queued + in flight) above which `whenWritable()` blocks. */
export const TUNNEL_WRITE_HIGH_WATER_BYTES = 1024 * 1024;

/** Buffered DataChannel bytes above which the sender pauses until drained. */
export const DATA_CHANNEL_BUFFER_HIGH_BYTES = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Endpoints
// ─────────────────────────────────────────────────────────────────────────────

/** The project's existing default Realtime Database instance (us-central1). */
export const DEFAULT_RTDB_URL = 'https://lingxi-5e4b2-default-rtdb.firebaseio.com';

/** `ns` query value the RTDB emulator expects for that instance. */
export const RTDB_EMULATOR_NAMESPACE = 'lingxi-5e4b2-default-rtdb';

/** STUN servers, Cloudflare first because the Google one is unreachable in China. */
export const DEFAULT_STUN_URLS: readonly string[] = Object.freeze([
  'stun:stun.cloudflare.com:3478',
  'stun:stun.l.google.com:19302',
]);

// ─────────────────────────────────────────────────────────────────────────────
// Presence and signaling
// ─────────────────────────────────────────────────────────────────────────────

/** How often a host PATCHes `last_seen`. */
export const HEARTBEAT_INTERVAL_MS = 30_000;

/** A host counts as online while its heartbeat is younger than this. */
export const ONLINE_WINDOW_MS = 90_000;

/** How long a viewer waits for an answer before deleting its offer. */
export const OFFER_ANSWER_TIMEOUT_MS = 30_000;

/** A host rejects an offer whose `created_at` is further than this from its clock. */
export const OFFER_MAX_AGE_MS = 5 * 60_000;

/** Longest wait for ICE candidates before offering with what has arrived. */
export const ICE_GATHER_TIMEOUT_MS = 3_000;

/** Time for the DataChannel to open before falling back to the tunnel. */
export const DATA_CHANNEL_OPEN_TIMEOUT_MS = 15_000;

/** After a direct-connection failure, a viewer goes straight to the tunnel for this long. */
export const DIRECT_FAILURE_MEMORY_MS = 10 * 60_000;

/** Most simultaneous viewer connections a host accepts. */
export const MAX_HOST_CONNECTIONS = 8;

// ─────────────────────────────────────────────────────────────────────────────
// Liveness
// ─────────────────────────────────────────────────────────────────────────────

/** DataChannel: send a `ping` this often… */
export const DATA_CHANNEL_PING_INTERVAL_MS = 15_000;
/** …and treat the peer as gone after this long without any message. */
export const DATA_CHANNEL_DEAD_MS = 45_000;

/** Tunnel: send a `ping` this often… */
export const TUNNEL_PING_INTERVAL_MS = 25_000;
/** …and treat the peer as gone after this long without any message. */
export const TUNNEL_DEAD_MS = 75_000;

/** Default timeout of an RPC request. */
export const RPC_DEFAULT_TIMEOUT_MS = 30_000;

// ─────────────────────────────────────────────────────────────────────────────
// Tunnel writer / reader
// ─────────────────────────────────────────────────────────────────────────────

/** A partly filled tunnel frame is written this long after its first fragment. */
export const TUNNEL_FLUSH_DELAY_MS = 25;

/** Most tunnel frame writes in flight at once. */
export const TUNNEL_MAX_INFLIGHT_WRITES = 4;

/** Processed frames are deleted in one PATCH at least this often… */
export const TUNNEL_DELETE_INTERVAL_MS = 500;
/** …or as soon as this many are waiting. */
export const TUNNEL_DELETE_BATCH = 32;

/** A missing frame that stays missing this long ends the tunnel (`tunnel_gap`). */
export const TUNNEL_GAP_TIMEOUT_MS = 30_000;

// ─────────────────────────────────────────────────────────────────────────────
// RTDB stream
// ─────────────────────────────────────────────────────────────────────────────

/** An SSE stream silent for this long is dropped and reconnected (the server pings every ~30 s). */
export const SSE_IDLE_TIMEOUT_MS = 90_000;

/** Reconnect backoff: first delay, ceiling, and the healthy time after which it restarts. */
export const SSE_BACKOFF_MIN_MS = 1_000;
export const SSE_BACKOFF_MAX_MS = 30_000;
export const SSE_BACKOFF_RESET_MS = 60_000;
