/**
 * Signaling records and rules of `lingxi-rc/1`: what the viewer and the host
 * write into the Realtime Database, where, how those records are signed, and
 * how untrusted data read back from RTDB is validated.
 *
 * Signing inputs are plain text, one field per line, joined with `\n` (no
 * trailing newline). Free text is b64u(UTF-8), an absent field is `-`, and the
 * SDP enters as b64u(SHA-256(UTF-8 sdp)) so its `a=fingerprint` is signed too.
 */

import {
  b64uDecode,
  b64uEncode,
  isB64u,
  sha256,
  utf8Encode,
} from './encoding.js';
import { RemoteProtocolError } from './errors.js';
import { idFromKey, signBytes, verifyBytes, type WebCryptoKey } from './keys.js';
import { systemClock, type Clock } from './clock.js';
import { SERVER_TIMESTAMP, type ServerTimestamp } from './rtdb.js';
import { OFFER_MAX_AGE_MS, ONLINE_WINDOW_MS, SDP_MAX_CHARS } from './version.js';

// ─────────────────────────────────────────────────────────────────────────────
// Records
// ─────────────────────────────────────────────────────────────────────────────

/** What kind of app is on the viewer side. */
export type DevicePlatform = 'web' | 'android' | 'ios';

/** How a connection carries its bytes. */
export type TransportKind = 'webrtc' | 'tunnel';

/** Why a host refused a connection (`AnswerRecord.reason`). */
export type AnswerRejectReason =
  | 'revoked'
  | 'disabled'
  | 'capacity'
  | 'upgrade_required'
  | 'bad_signature'
  | 'expired'
  | 'busy';

/** The viewer's identity as carried in an offer. */
export interface OfferDevice {
  /** `idFromKey(key)`. */
  id: string;
  /** b64u of the 32-byte raw Ed25519 device key. */
  key: string;
  name: string;
  platform: DevicePlatform;
  app_version: string;
}

/** `to_host/{host_id}/{conn_id}/offer`, written by the viewer. */
export interface OfferRecord {
  v: 1;
  /** `'tunnel'`: the viewer already knows a direct connection will not work. */
  transport: TransportKind;
  /** The complete offer SDP after ICE gathering; required for `webrtc`, absent for `tunnel`. */
  sdp?: string;
  device: OfferDevice;
  /** b64u of the viewer's 32-byte X25519 public key. */
  eph: string;
  /** b64u of 16 random bytes. */
  nonce: string;
  /** Server time of the write; not part of the signature. */
  created_at: number;
  /** b64u Ed25519 signature by the device key over {@link offerSigningInput}. */
  sig: string;
}

/** An {@link OfferRecord} as written: `created_at` is still the server-timestamp placeholder. */
export type OfferWrite = Omit<OfferRecord, 'created_at'> & { created_at: ServerTimestamp };

/** `to_viewer/{host_id}/{conn_id}/answer`, written by the host. */
export interface AnswerRecord {
  v: 1;
  status: 'accepted' | 'rejected';
  reason?: AnswerRejectReason;
  /** The transport the host chose; required when accepted. */
  transport?: TransportKind;
  /** The complete answer SDP; required for an accepted `webrtc` answer. */
  sdp?: string;
  host: { id: string; key: string };
  /** b64u of the host's 32-byte X25519 public key; required for an accepted `tunnel` answer. */
  eph?: string;
  /** b64u of 16 random bytes. */
  nonce: string;
  /** Server time of the write; not part of the signature. */
  created_at: number;
  /** b64u Ed25519 signature by the host key over {@link answerSigningInput}. */
  sig: string;
}

/** An {@link AnswerRecord} as written. */
export type AnswerWrite = Omit<AnswerRecord, 'created_at'> & { created_at: ServerTimestamp };

/** `hosts/{host_id}`, written by the desktop. */
export interface HostRecord {
  v: 1;
  name: string;
  /** Operating-system family of the desktop (decided by the host, e.g. `process.platform`). */
  platform: string;
  app_version: string;
  /** Major of the `client-protocol` contract the host speaks. */
  client_protocol_version: number;
  /** b64u of the 32-byte raw Ed25519 host key. */
  host_key: string;
  online: boolean;
  /** Server time of the last heartbeat. */
  last_seen: number;
  /** Server time the host process started. */
  started_at: number;
}

/** A {@link HostRecord} as written: the timestamps may still be placeholders. */
export type HostRecordWrite = Omit<HostRecord, 'last_seen' | 'started_at'> & {
  last_seen: number | ServerTimestamp;
  started_at: number | ServerTimestamp;
};

// ─────────────────────────────────────────────────────────────────────────────
// RTDB paths
// ─────────────────────────────────────────────────────────────────────────────

const ROOT = 'remote-control';
const MAX_SEGMENT_BYTES = 768;
// RTDB forbids . # $ [ ] / and ASCII control characters in keys.
const BAD_SEGMENT = /[.#$[\]\/\u0000-\u001f\u007f]/;

/** `true` when `value` may be used as one RTDB path segment. */
export function isPathSegment(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && !BAD_SEGMENT.test(value) && utf8Encode(value).length <= MAX_SEGMENT_BYTES;
}

/** Return `value` if it is a valid path segment, otherwise throw. */
export function assertPathSegment(value: string, label = 'path segment'): string {
  if (!isPathSegment(value)) throw new RemoteProtocolError(`invalid ${label}`);
  return value;
}

function join(uid: string, ...rest: string[]): string {
  return `/${ROOT}/${assertPathSegment(uid, 'uid')}${rest.map((s) => `/${s}`).join('')}`;
}

const seg = (value: string, label: string): string => assertPathSegment(value, label);

/** `/remote-control/{uid}/hosts` — the viewer lists the user's computers here. */
export function hostsPath(uid: string): string {
  return join(uid, 'hosts');
}

/** `/remote-control/{uid}/hosts/{host_id}`. */
export function hostPath(uid: string, hostId: string): string {
  return join(uid, 'hosts', seg(hostId, 'host_id'));
}

/** `/remote-control/{uid}/to_host/{host_id}` — the host's inbox; the host keeps one SSE here. */
export function toHostPath(uid: string, hostId: string): string {
  return join(uid, 'to_host', seg(hostId, 'host_id'));
}

/** `…/to_host/{host_id}/{conn_id}` — everything one connection sends to the host. */
export function toHostConnPath(uid: string, hostId: string, connId: string): string {
  return join(uid, 'to_host', seg(hostId, 'host_id'), seg(connId, 'conn_id'));
}

/** `…/to_host/{host_id}/{conn_id}/offer`. */
export function offerPath(uid: string, hostId: string, connId: string): string {
  return `${toHostConnPath(uid, hostId, connId)}/offer`;
}

/** `…/to_host/{host_id}/{conn_id}/up` — tunnel frames, viewer to host. */
export function upPath(uid: string, hostId: string, connId: string): string {
  return `${toHostConnPath(uid, hostId, connId)}/up`;
}

/** `/remote-control/{uid}/to_viewer/{host_id}` — all answers and tunnel frames for one host's viewers. */
export function toViewerPath(uid: string, hostId: string): string {
  return join(uid, 'to_viewer', seg(hostId, 'host_id'));
}

/** `…/to_viewer/{host_id}/{conn_id}` — everything the host sends one connection; the viewer listens here. */
export function toViewerConnPath(uid: string, hostId: string, connId: string): string {
  return join(uid, 'to_viewer', seg(hostId, 'host_id'), seg(connId, 'conn_id'));
}

/** `…/to_viewer/{host_id}/{conn_id}/answer`. */
export function answerPath(uid: string, hostId: string, connId: string): string {
  return `${toViewerConnPath(uid, hostId, connId)}/answer`;
}

/** `…/to_viewer/{host_id}/{conn_id}/down` — tunnel frames, host to viewer. */
export function downPath(uid: string, hostId: string, connId: string): string {
  return `${toViewerConnPath(uid, hostId, connId)}/down`;
}

const FRAME_KEY = /^f(0|[1-9][0-9]{0,9})$/;
const ARRAY_KEY = /^(0|[1-9][0-9]{0,9})$/;
const MAX_FRAME_SEQ = 9_999_999_999;

/** Key of tunnel frame `seq` under `up`/`down`: `f0`, `f1`, … (the prefix keeps RTDB from returning arrays). */
export function frameKey(seq: number): string {
  if (!Number.isSafeInteger(seq) || seq < 0 || seq > MAX_FRAME_SEQ) {
    throw new RemoteProtocolError('tunnel frame sequence out of range');
  }
  return `f${seq}`;
}

/**
 * Sequence number of a frame key, or `null` for anything else. Besides the
 * canonical `f12` this accepts a bare `12`, which is how an array-coerced
 * RTDB value presents the same frame.
 */
export function parseFrameKey(key: string): number | null {
  const match = FRAME_KEY.exec(key) ?? ARRAY_KEY.exec(key);
  if (!match) return null;
  const seq = Number(match[1] ?? match[0]);
  return seq <= MAX_FRAME_SEQ ? seq : null;
}

/** `…/up/f{seq}`. */
export function upFramePath(uid: string, hostId: string, connId: string, seq: number): string {
  return `${upPath(uid, hostId, connId)}/${frameKey(seq)}`;
}

/** `…/down/f{seq}`. */
export function downFramePath(uid: string, hostId: string, connId: string, seq: number): string {
  return `${downPath(uid, hostId, connId)}/${frameKey(seq)}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// Signing
// ─────────────────────────────────────────────────────────────────────────────

/** First line of an offer's signing input. */
export const OFFER_SIGNING_DOMAIN = 'lingxi-rc-offer/1';
/** First line of an answer's signing input. */
export const ANSWER_SIGNING_DOMAIN = 'lingxi-rc-answer/1';

/** The path coordinates a record is bound to; all three enter the signature. */
export interface SigningContext {
  uid: string;
  hostId: string;
  connId: string;
}

/** The signed part of an offer. */
export type OfferDraft = Omit<OfferRecord, 'sig' | 'created_at'>;
/** The signed part of an answer. */
export type AnswerDraft = Omit<AnswerRecord, 'sig' | 'created_at'>;

function line(value: string, label: string): string {
  if (/[\r\n]/.test(value)) throw new RemoteProtocolError(`${label} contains a line break`);
  return value;
}

const text = (value: string): string => b64uEncode(utf8Encode(value));

async function sdpDigest(sdp: string | undefined): Promise<string> {
  return sdp === undefined ? '-' : b64uEncode(await sha256(utf8Encode(sdp)));
}

/** The exact text the device key signs for an offer. */
export async function offerSigningInput(ctx: SigningContext, offer: OfferDraft): Promise<string> {
  const { device } = offer;
  return [
    OFFER_SIGNING_DOMAIN,
    line(ctx.uid, 'uid'),
    line(ctx.hostId, 'host_id'),
    line(ctx.connId, 'conn_id'),
    line(device.id, 'device.id'),
    line(device.key, 'device.key'),
    text(device.name),
    line(device.platform, 'device.platform'),
    text(device.app_version),
    line(offer.transport, 'transport'),
    await sdpDigest(offer.sdp),
    line(offer.eph, 'eph'),
    line(offer.nonce, 'nonce'),
  ].join('\n');
}

/**
 * The exact text the host key signs for an answer. `offerNonce` is the nonce
 * of the offer being answered, which ties the answer to that one offer.
 */
export async function answerSigningInput(ctx: SigningContext, offerNonce: string, answer: AnswerDraft): Promise<string> {
  return [
    ANSWER_SIGNING_DOMAIN,
    line(ctx.uid, 'uid'),
    line(ctx.hostId, 'host_id'),
    line(ctx.connId, 'conn_id'),
    line(offerNonce, 'offer nonce'),
    line(answer.status, 'status'),
    line(answer.reason ?? '-', 'reason'),
    line(answer.transport ?? '-', 'transport'),
    await sdpDigest(answer.sdp),
    line(answer.eph ?? '-', 'eph'),
    line(answer.nonce, 'nonce'),
  ].join('\n');
}

/** Sign an offer with the device key; the result is ready to PUT with a server `created_at`. */
export async function signOffer(ctx: SigningContext, devicePrivateKey: WebCryptoKey, draft: OfferDraft): Promise<OfferWrite> {
  const signature = await signBytes(devicePrivateKey, utf8Encode(await offerSigningInput(ctx, draft)));
  return { ...draft, created_at: SERVER_TIMESTAMP, sig: b64uEncode(signature) };
}

/** Sign an answer with the host key; the result is ready to PUT with a server `created_at`. */
export async function signAnswer(
  ctx: SigningContext,
  hostPrivateKey: WebCryptoKey,
  offerNonce: string,
  draft: AnswerDraft,
): Promise<AnswerWrite> {
  const signature = await signBytes(hostPrivateKey, utf8Encode(await answerSigningInput(ctx, offerNonce, draft)));
  return { ...draft, created_at: SERVER_TIMESTAMP, sig: b64uEncode(signature) };
}

/** Why a record failed verification. */
export type VerifyFailure =
  /** Not a record of the expected shape or size; drop it, do not answer. */
  | 'bad_shape'
  /** An id does not match its key, or the host id is not the one asked for. */
  | 'id_mismatch'
  /** The host key differs from the pinned (or advertised) one. */
  | 'key_mismatch'
  | 'bad_signature'
  /** `created_at` is further from the local clock than the allowed age. */
  | 'expired';

/** Outcome of {@link verifyOffer} / {@link verifyAnswer}. */
export type VerifyResult<T> = { ok: true; value: T } | { ok: false; reason: VerifyFailure; detail?: string };

function fail(reason: VerifyFailure, detail?: string): { ok: false; reason: VerifyFailure; detail?: string } {
  return detail === undefined ? { ok: false, reason } : { ok: false, reason, detail };
}

/** Inputs of {@link verifyOffer}. */
export interface VerifyOfferOptions extends SigningContext {
  /** The value read from `…/offer`; anything at all. */
  record: unknown;
  /** Local time in ms; defaults to the system clock. */
  now?: number;
  /** Accepted distance between `created_at` and `now`; defaults to 5 minutes. */
  maxAgeMs?: number;
}

/**
 * Steps 1–4 of the host's offer check, in the plan's order: shape and size,
 * device id against device key, signature, then `created_at` against the local
 * clock. Step 5, "each `conn_id` is handled once", is the caller's.
 *
 * Suggested host reaction: `bad_shape` is dropped silently; `id_mismatch` and
 * `bad_signature` are answered `rejected/bad_signature`; `expired` is answered
 * `rejected/expired`.
 */
export async function verifyOffer(options: VerifyOfferOptions): Promise<VerifyResult<OfferRecord>> {
  const parsed = parseOfferRecord(options.record);
  if (!parsed.ok) return fail('bad_shape', parsed.error);
  const offer = parsed.value;
  const deviceKey = b64uDecode(offer.device.key);
  if ((await idFromKey(deviceKey)) !== offer.device.id) return fail('id_mismatch', 'device.id does not match device.key');
  const input = utf8Encode(await offerSigningInput(options, offer));
  if (!(await verifyBytes(deviceKey, b64uDecode(offer.sig), input))) return fail('bad_signature');
  const now = options.now ?? systemClock.now();
  if (Math.abs(now - offer.created_at) > (options.maxAgeMs ?? OFFER_MAX_AGE_MS)) return fail('expired');
  return { ok: true, value: offer };
}

/** Inputs of {@link verifyAnswer}. */
export interface VerifyAnswerOptions extends SigningContext {
  /** The value read from `…/answer`; anything at all. */
  record: unknown;
  /** Nonce (b64u) of the offer this answer must belong to. */
  offerNonce: string;
  /**
   * Host key (b64u) the answer must be signed with: the one pinned at pairing,
   * or on first pairing `hosts/{host_id}.host_key`.
   */
  expectedHostKey: string;
  /** Local time in ms; defaults to the system clock. Only used with `maxAgeMs`. */
  now?: number;
  /** When set, `created_at` must be within this distance of `now`. */
  maxAgeMs?: number;
}

/**
 * The viewer's answer check: shape, then the host id (it must be the one
 * dialled and derive from the key), then the key (pinned or advertised), then
 * the signature, then optionally the age.
 */
export async function verifyAnswer(options: VerifyAnswerOptions): Promise<VerifyResult<AnswerRecord>> {
  const parsed = parseAnswerRecord(options.record);
  if (!parsed.ok) return fail('bad_shape', parsed.error);
  const answer = parsed.value;
  if (answer.host.id !== options.hostId) return fail('id_mismatch', 'answer is from a different host');
  const hostKey = b64uDecode(answer.host.key);
  if ((await idFromKey(hostKey)) !== answer.host.id) return fail('id_mismatch', 'host.id does not match host.key');
  if (answer.host.key !== options.expectedHostKey) return fail('key_mismatch');
  const input = utf8Encode(await answerSigningInput(options, options.offerNonce, answer));
  if (!(await verifyBytes(hostKey, b64uDecode(answer.sig), input))) return fail('bad_signature');
  if (options.maxAgeMs !== undefined) {
    const now = options.now ?? systemClock.now();
    if (Math.abs(now - answer.created_at) > options.maxAgeMs) return fail('expired');
  }
  return { ok: true, value: answer };
}

// ─────────────────────────────────────────────────────────────────────────────
// Parsing untrusted records
// ─────────────────────────────────────────────────────────────────────────────

/** Result of parsing a record read from RTDB: the typed value, or why it was refused. */
export type ParseResult<T> = { ok: true; value: T } | { ok: false; error: string };

const NAME_MAX_CHARS = 128;
const VERSION_MAX_CHARS = 64;
const PLATFORM_MAX_CHARS = 32;
// C0 and C1 controls, DEL and the Unicode line/paragraph separators.
const DISPLAY_UNSAFE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;
const ID = /^[A-Za-z0-9_-]{22}$/;

const DEVICE_PLATFORMS: readonly string[] = ['web', 'android', 'ios'];
const TRANSPORTS: readonly string[] = ['webrtc', 'tunnel'];
const REJECT_REASONS: readonly string[] = [
  'revoked',
  'disabled',
  'capacity',
  'upgrade_required',
  'bad_signature',
  'expired',
  'busy',
];

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function displayText(value: unknown, max: number): value is string {
  return typeof value === 'string' && value.length >= 1 && value.length <= max && !DISPLAY_UNSAFE.test(value);
}

function timestamp(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

class Invalid extends Error {}

function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Invalid(message);
}

function guarded<T>(build: () => T): ParseResult<T> {
  try {
    return { ok: true, value: build() };
  } catch (error) {
    if (error instanceof Invalid) return { ok: false, error: error.message };
    throw error;
  }
}

function optionalSdp(value: unknown): string | undefined {
  if (value === undefined) return undefined;
  check(typeof value === 'string' && value.length >= 1 && value.length <= SDP_MAX_CHARS, 'sdp: not a string of 1..16384 characters');
  return value;
}

/** Validate a value read from `…/offer`. Unknown extra fields are ignored. */
export function parseOfferRecord(raw: unknown): ParseResult<OfferRecord> {
  return guarded(() => {
    check(isObject(raw), 'offer: not an object');
    check(raw['v'] === 1, 'offer: unsupported v');
    const transport = raw['transport'];
    check(typeof transport === 'string' && TRANSPORTS.includes(transport), 'offer: bad transport');
    const sdp = optionalSdp(raw['sdp']);
    if (transport === 'webrtc') check(sdp !== undefined, 'offer: webrtc needs an sdp');
    else check(sdp === undefined, 'offer: tunnel must not carry an sdp');
    const device = raw['device'];
    check(isObject(device), 'offer: device is not an object');
    check(typeof device['id'] === 'string' && ID.test(device['id']), 'offer: bad device.id');
    check(isB64u(device['key'], 32), 'offer: bad device.key');
    check(displayText(device['name'], NAME_MAX_CHARS), 'offer: bad device.name');
    check(typeof device['platform'] === 'string' && DEVICE_PLATFORMS.includes(device['platform']), 'offer: bad device.platform');
    check(displayText(device['app_version'], VERSION_MAX_CHARS), 'offer: bad device.app_version');
    check(isB64u(raw['eph'], 32), 'offer: bad eph');
    check(isB64u(raw['nonce'], 16), 'offer: bad nonce');
    check(timestamp(raw['created_at']), 'offer: bad created_at');
    check(isB64u(raw['sig'], 64), 'offer: bad sig');
    const record: OfferRecord = {
      v: 1,
      transport: transport as TransportKind,
      device: {
        id: device['id'],
        key: device['key'],
        name: device['name'],
        platform: device['platform'] as DevicePlatform,
        app_version: device['app_version'],
      },
      eph: raw['eph'],
      nonce: raw['nonce'],
      created_at: raw['created_at'],
      sig: raw['sig'],
    };
    if (sdp !== undefined) record.sdp = sdp;
    return record;
  });
}

/** Validate a value read from `…/answer`. Unknown extra fields are ignored. */
export function parseAnswerRecord(raw: unknown): ParseResult<AnswerRecord> {
  return guarded(() => {
    check(isObject(raw), 'answer: not an object');
    check(raw['v'] === 1, 'answer: unsupported v');
    const status = raw['status'];
    check(status === 'accepted' || status === 'rejected', 'answer: bad status');
    const reason = raw['reason'];
    check(
      reason === undefined || (status === 'rejected' && typeof reason === 'string' && REJECT_REASONS.includes(reason)),
      'answer: bad reason',
    );
    const transport = raw['transport'];
    check(transport === undefined || (typeof transport === 'string' && TRANSPORTS.includes(transport)), 'answer: bad transport');
    const sdp = optionalSdp(raw['sdp']);
    const eph = raw['eph'];
    check(eph === undefined || isB64u(eph, 32), 'answer: bad eph');
    if (status === 'accepted') {
      check(transport !== undefined, 'answer: accepted without transport');
      if (transport === 'webrtc') check(sdp !== undefined, 'answer: webrtc needs an sdp');
      else {
        check(sdp === undefined, 'answer: tunnel must not carry an sdp');
        check(eph !== undefined, 'answer: tunnel needs an eph');
      }
    }
    const host = raw['host'];
    check(isObject(host), 'answer: host is not an object');
    check(typeof host['id'] === 'string' && ID.test(host['id']), 'answer: bad host.id');
    check(isB64u(host['key'], 32), 'answer: bad host.key');
    check(isB64u(raw['nonce'], 16), 'answer: bad nonce');
    check(timestamp(raw['created_at']), 'answer: bad created_at');
    check(isB64u(raw['sig'], 64), 'answer: bad sig');
    const record: AnswerRecord = {
      v: 1,
      status,
      host: { id: host['id'], key: host['key'] },
      nonce: raw['nonce'],
      created_at: raw['created_at'],
      sig: raw['sig'],
    };
    if (reason !== undefined) record.reason = reason as AnswerRejectReason;
    if (transport !== undefined) record.transport = transport as TransportKind;
    if (sdp !== undefined) record.sdp = sdp;
    if (eph !== undefined) record.eph = eph;
    return record;
  });
}

/** Validate a value read from `hosts/{host_id}`. Unknown extra fields are ignored. */
export function parseHostRecord(raw: unknown): ParseResult<HostRecord> {
  return guarded(() => {
    check(isObject(raw), 'host: not an object');
    check(raw['v'] === 1, 'host: unsupported v');
    check(displayText(raw['name'], NAME_MAX_CHARS), 'host: bad name');
    check(displayText(raw['platform'], PLATFORM_MAX_CHARS), 'host: bad platform');
    check(displayText(raw['app_version'], VERSION_MAX_CHARS), 'host: bad app_version');
    const protocol = raw['client_protocol_version'];
    check(typeof protocol === 'number' && Number.isSafeInteger(protocol) && protocol >= 0, 'host: bad client_protocol_version');
    check(isB64u(raw['host_key'], 32), 'host: bad host_key');
    check(typeof raw['online'] === 'boolean', 'host: bad online');
    check(timestamp(raw['last_seen']), 'host: bad last_seen');
    check(timestamp(raw['started_at']), 'host: bad started_at');
    return {
      v: 1,
      name: raw['name'],
      platform: raw['platform'],
      app_version: raw['app_version'],
      client_protocol_version: protocol,
      host_key: raw['host_key'],
      online: raw['online'],
      last_seen: raw['last_seen'],
      started_at: raw['started_at'],
    };
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Presence
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Whether a host looks online. `online` must be set, and either its last
 * heartbeat was *received* (`receivedAt`, local ms) under 90 s ago, or, when
 * no update has been received yet (first load), `last_seen` is within 90 s of
 * `now` in either direction, which tolerates clock skew.
 */
export function isHostOnline(record: Pick<HostRecord, 'online' | 'last_seen'>, now: number, receivedAt?: number): boolean {
  if (!record.online) return false;
  if (receivedAt !== undefined) return now - receivedAt < ONLINE_WINDOW_MS;
  return Math.abs(now - record.last_seen) < ONLINE_WINDOW_MS;
}

/**
 * Remembers when each host's heartbeat last *changed* on this machine, so
 * {@link isHostOnline} can use local receive time instead of the host's clock.
 * Feed it every host record the viewer sees; re-check {@link HostPresence.isOnline}
 * on a timer, since a dead host produces no events.
 */
export class HostPresence {
  private readonly hosts = new Map<string, { record: HostRecord; receivedAt?: number }>();

  constructor(private readonly clock: Clock = systemClock) {}

  /** Record an observed host record (the initial load and every later change). */
  observe(hostId: string, record: HostRecord): void {
    const known = this.hosts.get(hostId);
    if (known === undefined) this.hosts.set(hostId, { record });
    else if (known.record.last_seen !== record.last_seen) this.hosts.set(hostId, { record, receivedAt: this.clock.now() });
    else this.hosts.set(hostId, { record, receivedAt: known.receivedAt });
  }

  /** Forget a host that disappeared from the listing. */
  forget(hostId: string): void {
    this.hosts.delete(hostId);
  }

  /** `false` for hosts never observed. */
  isOnline(hostId: string): boolean {
    const known = this.hosts.get(hostId);
    return known !== undefined && isHostOnline(known.record, this.clock.now(), known.receivedAt);
  }
}
