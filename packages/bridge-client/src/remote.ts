/**
 * `@lingxi/bridge-client/remote` — the shared TypeScript core of LingXi Remote
 * Control (`lingxi-rc/1`): identity keys, signed signaling records, pairing
 * codes, message framing, the RTDB client and tunnel, and RPC. It runs
 * unchanged in Node, the Electron main process and renderer, and browsers
 * (WebCrypto, `fetch`, `TextEncoder`, `ReadableStream` and timers only).
 *
 * Layering, bottom to top: `encoding` → `keys` → `signaling` / `pairing` /
 * `tunnelCrypto` → `rtdb` (+ `rtdbMirror`, `memoryRtdb`) → `tunnel` →
 * `transport` → `rpc` → `messages`.
 */

// ── Protocol constants ───────────────────────────────────────────────────────

export {
  /** Protocol identifier, `'lingxi-rc/1'`. */
  REMOTE_PROTOCOL,
  /** Major sent as `initialize.protocol`; a different major is refused. */
  REMOTE_PROTOCOL_MAJOR,
  /** Major of the `client-protocol` contract (30), derived from `CLIENT_PROTOCOL_VERSION`. */
  CLIENT_PROTOCOL_MAJOR,
  /** Label of the DataChannel that carries RPC fragments, `'lingxi-agent'`. */
  DATA_CHANNEL_LABEL,
  /** `initialize.app.features` value of a viewer that understands Bots, `'bots'`. */
  REMOTE_FEATURE_BOTS,
  /** Largest fragment, kind byte included (16384). */
  FRAGMENT_MAX_BYTES,
  /** Fragment size that packs three to a tunnel frame (16374). */
  TUNNEL_FRAGMENT_MAX_BYTES,
  /** Largest reassembled message (16 MiB). */
  MESSAGE_MAX_BYTES,
  /** Largest tunnel frame plaintext (49136). */
  TUNNEL_FRAME_PLAINTEXT_MAX,
  /** Largest SDP in an offer or answer (16384 characters). */
  SDP_MAX_CHARS,
  /** Unsent tunnel bytes above which `whenWritable()` blocks (1 MiB). */
  TUNNEL_WRITE_HIGH_WATER_BYTES,
  /** Buffered DataChannel bytes above which a sender pauses (1 MiB). */
  DATA_CHANNEL_BUFFER_HIGH_BYTES,
  /** The project's default RTDB instance URL. */
  DEFAULT_RTDB_URL,
  /** `ns` value the RTDB emulator expects for that instance. */
  RTDB_EMULATOR_NAMESPACE,
  /** Default STUN servers, Cloudflare first. */
  DEFAULT_STUN_URLS,
  /** Host heartbeat interval (30 s). */
  HEARTBEAT_INTERVAL_MS,
  /** A host is online while its heartbeat is younger than this (90 s). */
  ONLINE_WINDOW_MS,
  /** How long a viewer waits for an answer (30 s). */
  OFFER_ANSWER_TIMEOUT_MS,
  /** Largest accepted distance between an offer's `created_at` and the host clock (5 min). */
  OFFER_MAX_AGE_MS,
  /** Longest wait for ICE candidates (3 s). */
  ICE_GATHER_TIMEOUT_MS,
  /** Time for the DataChannel to open before falling back to the tunnel (15 s). */
  DATA_CHANNEL_OPEN_TIMEOUT_MS,
  /** How long a viewer remembers that a host needed the tunnel (10 min). */
  DIRECT_FAILURE_MEMORY_MS,
  /** Most simultaneous viewer connections per host (8). */
  MAX_HOST_CONNECTIONS,
  /** DataChannel ping interval (15 s). */
  DATA_CHANNEL_PING_INTERVAL_MS,
  /** DataChannel silence after which the peer is gone (45 s). */
  DATA_CHANNEL_DEAD_MS,
  /** Tunnel ping interval (25 s). */
  TUNNEL_PING_INTERVAL_MS,
  /** Tunnel silence after which the peer is gone (75 s). */
  TUNNEL_DEAD_MS,
  /** Default RPC request timeout (30 s). */
  RPC_DEFAULT_TIMEOUT_MS,
  /** Delay before a partly filled tunnel frame is written (25 ms). */
  TUNNEL_FLUSH_DELAY_MS,
  /** Most tunnel frame writes in flight (4). */
  TUNNEL_MAX_INFLIGHT_WRITES,
  /** Longest wait before processed tunnel frames are deleted (500 ms). */
  TUNNEL_DELETE_INTERVAL_MS,
  /** Processed frames that trigger an immediate delete batch (32). */
  TUNNEL_DELETE_BATCH,
  /** A missing tunnel frame ends the link after this long (30 s). */
  TUNNEL_GAP_TIMEOUT_MS,
  /** An SSE stream silent this long is reconnected (90 s). */
  SSE_IDLE_TIMEOUT_MS,
  /** First SSE reconnect delay (1 s). */
  SSE_BACKOFF_MIN_MS,
  /** SSE reconnect delay ceiling (30 s). */
  SSE_BACKOFF_MAX_MS,
  /** Healthy time after which the SSE backoff restarts (60 s). */
  SSE_BACKOFF_RESET_MS,
} from './remote/version.js';

// ── Time and errors ──────────────────────────────────────────────────────────

export {
  /** Wall clock plus timers, injectable everywhere something waits. */
  type Clock,
  /** The real clock. */
  systemClock,
  /** A clock that moves only on `advance()`, for tests. */
  ManualClock,
} from './remote/clock.js';

export {
  /** The peer broke the wire rules (bad fragment, frame, encoding). */
  RemoteProtocolError,
} from './remote/errors.js';

// ── Encoding ─────────────────────────────────────────────────────────────────

export {
  /** A `Uint8Array` on a plain `ArrayBuffer`, what WebCrypto accepts. */
  type Bytes,
  /** Unpadded base64url encode (`b64u`). */
  b64uEncode,
  /** Strict unpadded base64url decode; throws `RemoteProtocolError`. */
  b64uDecode,
  /** Whether a value is valid base64url, optionally of an exact byte length. */
  isB64u,
  /** UTF-8 bytes of a string. */
  utf8Encode,
  /** Decode UTF-8, optionally strict. */
  utf8Decode,
  /** SHA-256 via WebCrypto. */
  sha256,
  /** Cryptographically random bytes. */
  randomBytes,
  /** A fresh 22-character `conn_id`-style id (16 random bytes, b64u). */
  randomId,
  /** Concatenate byte arrays. */
  concatBytes,
  /** Compare byte arrays by content. */
  bytesEqual,
} from './remote/encoding.js';

// ── Keys ─────────────────────────────────────────────────────────────────────

export {
  /** A WebCrypto key handle, typed so it works with DOM and Node typings. */
  type WebCryptoKey,
  /** Ed25519 signing key: raw public key, private `CryptoKey`, optional PKCS#8. */
  type IdentityKey,
  /** X25519 key for one connection attempt. */
  type EphemeralKey,
  /** `{ extractable }` option of the identity-key functions. */
  type KeyOptions,
  /** Generate an identity key; non-extractable keeps the private key out of JS. */
  generateIdentityKey,
  /** Import a stored PKCS#8 identity key. */
  importIdentityKey,
  /** Deterministic identity key from a 32-byte seed (fixtures). */
  identityKeyFromSeed,
  /** Ed25519 sign. */
  signBytes,
  /** Ed25519 verify; never throws, malformed input is `false`. */
  verifyBytes,
  /** Self-certifying id: first 22 characters of b64u(SHA-256(raw public key)). */
  idFromKey,
  /** Generate an X25519 ephemeral key. */
  generateEphemeralKey,
  /** Deterministic X25519 key from a 32-byte seed (fixtures). */
  ephemeralKeyFromSeed,
  /** X25519 shared secret; rejects the all-zero result. */
  x25519,
  /** HKDF-SHA256. */
  hkdfSha256,
  /** PKCS#8 header of an Ed25519 key. */
  ED25519_PKCS8_PREFIX,
  /** PKCS#8 header of an X25519 key. */
  X25519_PKCS8_PREFIX,
} from './remote/keys.js';

// ── Signaling ────────────────────────────────────────────────────────────────

export {
  /** `'web' | 'android' | 'ios'`. */
  type DevicePlatform,
  /** `'webrtc' | 'tunnel'`. */
  type TransportKind,
  /** Why a host refused a connection. */
  type AnswerRejectReason,
  /** The viewer identity inside an offer. */
  type OfferDevice,
  /** `to_host/{host}/{conn}/offer`. */
  type OfferRecord,
  /** An offer ready to PUT (`created_at` still the server placeholder). */
  type OfferWrite,
  /** `to_viewer/{host}/{conn}/answer`. */
  type AnswerRecord,
  /** An answer ready to PUT. */
  type AnswerWrite,
  /** `hosts/{host_id}`. */
  type HostRecord,
  /** A host record as written (timestamps may be placeholders). */
  type HostRecordWrite,
  /** The `{uid, hostId, connId}` a record is bound to. */
  type SigningContext,
  /** The signed part of an offer. */
  type OfferDraft,
  /** The signed part of an answer. */
  type AnswerDraft,
  /** Why verification failed. */
  type VerifyFailure,
  /** `{ ok: true, value }` or `{ ok: false, reason }`. */
  type VerifyResult,
  /** Inputs of `verifyOffer`. */
  type VerifyOfferOptions,
  /** Inputs of `verifyAnswer`. */
  type VerifyAnswerOptions,
  /** `{ ok: true, value }` or `{ ok: false, error }` from the parsers. */
  type ParseResult,
  /** Whether a string is a valid RTDB path segment. */
  isPathSegment,
  /** Validate a path segment or throw. */
  assertPathSegment,
  /** `/remote-control/{uid}/hosts`. */
  hostsPath,
  /** `…/hosts/{host_id}`. */
  hostPath,
  /** `…/to_host/{host_id}`, the host's inbox (one SSE). */
  toHostPath,
  /** `…/to_host/{host_id}/{conn_id}`. */
  toHostConnPath,
  /** `…/to_host/{host_id}/{conn_id}/offer`. */
  offerPath,
  /** `…/to_host/{host_id}/{conn_id}/up`, tunnel frames viewer to host. */
  upPath,
  /** `…/to_viewer/{host_id}`. */
  toViewerPath,
  /** `…/to_viewer/{host_id}/{conn_id}`, what a viewer listens on. */
  toViewerConnPath,
  /** `…/to_viewer/{host_id}/{conn_id}/answer`. */
  answerPath,
  /** `…/to_viewer/{host_id}/{conn_id}/down`, tunnel frames host to viewer. */
  downPath,
  /** `…/up/f{seq}`. */
  upFramePath,
  /** `…/down/f{seq}`. */
  downFramePath,
  /** `f{seq}`, the key of a tunnel frame. */
  frameKey,
  /** Sequence number of a frame key (`f12`, or array-form `12`), else `null`. */
  parseFrameKey,
  /** First line of an offer's signing input. */
  OFFER_SIGNING_DOMAIN,
  /** First line of an answer's signing input. */
  ANSWER_SIGNING_DOMAIN,
  /** The exact text the device key signs. */
  offerSigningInput,
  /** The exact text the host key signs. */
  answerSigningInput,
  /** Sign an offer with the device key. */
  signOffer,
  /** Sign an answer with the host key. */
  signAnswer,
  /** Host-side offer check: shape, id, signature, age. */
  verifyOffer,
  /** Viewer-side answer check: shape, host id, pinned key, signature. */
  verifyAnswer,
  /** Validate an untrusted offer. */
  parseOfferRecord,
  /** Validate an untrusted answer. */
  parseAnswerRecord,
  /** Validate an untrusted host record. */
  parseHostRecord,
  /** Whether a host looks online (pure rule). */
  isHostOnline,
  /** Tracks local heartbeat receive times for `isHostOnline`. */
  HostPresence,
} from './remote/signaling.js';

// ── Pairing ──────────────────────────────────────────────────────────────────

export {
  /** The 6-digit pairing code from raw keys and nonces. */
  pairingCode,
  /** The pairing code from an offer and its answer. */
  pairingCodeFor,
} from './remote/pairing.js';

// ── Framing ──────────────────────────────────────────────────────────────────

export {
  /** Fragment `kind` values: whole 0, more 1, last 2. */
  FRAGMENT_KIND,
  /** Split a message into fragments. */
  encodeMessage,
  /** Reassemble fragments into messages; throws `RemoteProtocolError`. */
  Reassembler,
} from './remote/framing.js';

// ── Tunnel cryptography ──────────────────────────────────────────────────────

export {
  /** `'up'` (viewer to host) or `'down'`. */
  type TunnelDir,
  /** The two AES-GCM keys of a connection. */
  type TunnelKeys,
  /** A frame failed authentication or was malformed. */
  TunnelCryptoError,
  /** HKDF the two traffic keys from the X25519 secret. */
  deriveTunnelKeys,
  /** The GCM nonce of a frame sequence number. */
  tunnelNonce,
  /** The AAD of a frame. */
  tunnelAad,
  /** Encrypt a frame to its stored `b64u(ciphertext ‖ tag)` value. */
  sealFrame,
  /** Decrypt a stored frame value. */
  openFrame,
  /** Lay fragments out as a frame plaintext. */
  packFragments,
  /** Inverse of `packFragments`. */
  unpackFragments,
} from './remote/tunnelCrypto.js';

// ── Realtime Database ────────────────────────────────────────────────────────

export {
  /** The `{".sv":"timestamp"}` write placeholder. */
  SERVER_TIMESTAMP,
  /** Type of `SERVER_TIMESTAMP`. */
  type ServerTimestamp,
  /** One stream event: `put` or `patch` at a relative path. */
  type RtdbEvent,
  /** Why a subscription ended for good. */
  type RtdbListenError,
  /** A live `listen`: `close()` and `reconnect()`. */
  type RtdbSubscription,
  /** The database interface everything else depends on. */
  type Rtdb,
  /** A failed request; `status` 0 means no response. */
  RtdbError,
  /** `[key, value]` pairs of an object or array value, skipping nulls. */
  childEntries,
  /** One dispatched SSE message. */
  type SseMessage,
  /** Incremental SSE parser. */
  SseParser,
  /** The `fetch` shape the client needs (fits `fetch` and Electron `net.fetch`). */
  type FetchFn,
  /** Options of `RtdbClient`. */
  type RtdbClientOptions,
  /** REST + SSE client. */
  RtdbClient,
} from './remote/rtdb.js';

export {
  /** One entry that differs before and after an event. */
  type RtdbChange,
  /** What an event changed, queryable by depth. */
  RtdbChanges,
  /** In-memory copy of a listened subtree. */
  RtdbMirror,
} from './remote/rtdbMirror.js';

export {
  /** Options of `MemoryRtdb`. */
  type MemoryRtdbOptions,
  /** In-memory `Rtdb` with real event semantics, for tests. */
  MemoryRtdb,
} from './remote/memoryRtdb.js';

// ── Transport, tunnel, RPC ───────────────────────────────────────────────────

export {
  /** An ordered fragment stream (DataChannel or tunnel). */
  type FragmentLink,
  /** Whole-message transport that `RpcPeer` runs over. */
  type RemoteTransport,
  /** Fragments to messages over a `FragmentLink`. */
  MessageTransport,
} from './remote/transport.js';

export {
  /** Options of `TunnelLink`. */
  type TunnelLinkOptions,
  /** The RTDB tunnel as a `FragmentLink`. */
  TunnelLink,
} from './remote/tunnel.js';

export {
  /** Which transport's ping timings `startKeepalive` uses. */
  type KeepaliveKind,
  /** The RPC error codes. */
  type RemoteErrorCode,
  /** Every RPC error code. */
  REMOTE_ERROR_CODES,
  /** An RPC failure with a code. */
  RemoteRpcError,
  /** Handler for inbound requests. */
  type RpcRequestHandler,
  /** Handler for inbound notifications. */
  type RpcNotificationHandler,
  /** Options of `RpcPeer`. */
  type RpcPeerOptions,
  /** One end of an RPC conversation; answers `ping` itself, and `startKeepalive` pings and times out. */
  RpcPeer,
} from './remote/rpc.js';

export {
  REMOTE_MEDIA_REFERENCE_MAX_CHARS,
  REMOTE_MEDIA_SDP_MAX_CHARS,
  parseRemoteMediaAnswer,
  parseRemoteMediaBridgeControlRequest,
  parseRemoteMediaBridgeOfferRequest,
  parseRemoteMediaControlRequest,
  parseRemoteMediaControlResult,
  parseRemoteMediaOffer,
  parseRemoteMediaPlaybackReceipt,
  remoteMediaSourceRef,
  isRemoteMediaSourceForRoute,
  remoteMediaBridgeControlSuccess,
  remoteMediaBridgeFailure,
  remoteMediaBridgeSuccess,
  type RemoteMediaAnswerResult,
  type RemoteMediaCapabilities,
  type RemoteMediaBridgeControlRequest,
  type RemoteMediaBridgeControlResponse,
  type RemoteMediaIntent,
  type RemoteMediaSourceKind,
  type RemoteMediaBridgeErrorCode,
  type RemoteMediaBridgeOfferRequest,
  type RemoteMediaBridgeOfferResponse,
  type RemoteMediaControlAction,
  type RemoteMediaControlRequestParams,
  type RemoteMediaControlResult,
  type RemoteMediaMode,
  type RemoteMediaOfferParams,
  type RemoteMediaPlaybackReceipt,
} from './remote/media.js';

// ── Message types ────────────────────────────────────────────────────────────

/**
 * Params/results of every RPC method, the `RemoteRecord` union, and the method
 * tables `ViewerRequests`, `ViewerNotifications`, `HostNotifications`; each is
 * documented where it is declared.
 */
export type * from './remote/messages.js';

// Independent binary file channel; never wrap this in MessageTransport.
export * from './remote/files.js';

export * from './remote/localApps.js';
