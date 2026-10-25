/**
 * Signaling records: signing inputs and signatures (against the shared
 * fixtures), verification and tampering, strict parsing of untrusted data, RTDB
 * path helpers, and host presence.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ManualClock } from '../src/remote/clock.js';
import { b64uDecode, b64uEncode } from '../src/remote/encoding.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import { identityKeyFromSeed, idFromKey, type IdentityKey } from '../src/remote/keys.js';
import { SERVER_TIMESTAMP } from '../src/remote/rtdb.js';
import {
  answerPath,
  answerSigningInput,
  assertPathSegment,
  downFramePath,
  downPath,
  frameKey,
  HostPresence,
  hostPath,
  hostsPath,
  isHostOnline,
  isPathSegment,
  offerPath,
  offerSigningInput,
  parseAnswerRecord,
  parseFrameKey,
  parseHostRecord,
  parseOfferRecord,
  signAnswer,
  signOffer,
  toHostConnPath,
  toHostPath,
  toViewerConnPath,
  toViewerPath,
  upFramePath,
  upPath,
  verifyAnswer,
  verifyOffer,
  type AnswerRecord,
  type HostRecord,
  type OfferRecord,
} from '../src/remote/signaling.js';
import { fixture, unhex } from './remote-helpers.js';

const signing = fixture('signing');
const NOW: number = signing.now_for_verification;

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

async function keyOf(entry: { seed_hex: string }): Promise<IdentityKey> {
  return identityKeyFromSeed(unhex(entry.seed_hex));
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

test('ids derive from the keys exactly as in the fixtures', async () => {
  for (const entry of [signing.rfc8032_test1, signing.keys.host, signing.keys.device, signing.keys.device2]) {
    assert.equal(await idFromKey(unhex(entry.public_hex)), entry.id, entry.public_hex);
  }
});

test('every offer fixture reproduces its signing input, signature and verification', async () => {
  for (const c of signing.offers) {
    const ctx = { uid: c.uid, hostId: c.host_id, connId: c.conn_id };
    const record: OfferRecord = c.record;
    assert.equal(await offerSigningInput(ctx, record), c.signing_input, c.name);

    const signer = await keyOf(Object.values<{ id: string; seed_hex: string }>(signing.keys).find((k) => k.id === record.device.id)!);
    const { sig: _sig, created_at: _created, ...draft } = record;
    const written = await signOffer(ctx, signer.privateKey, draft);
    assert.equal(written.sig, record.sig, `${c.name}: Ed25519 is deterministic`);
    assert.equal(written.created_at, SERVER_TIMESTAMP);

    const verified = await verifyOffer({ ...ctx, record, now: NOW });
    assert.deepEqual(verified, { ok: true, value: record }, c.name);
  }
});

test('every answer fixture reproduces its signing input, signature and verification', async () => {
  const host = await keyOf(signing.keys.host);
  for (const c of signing.answers) {
    const ctx = { uid: c.uid, hostId: c.host_id, connId: c.conn_id };
    const record: AnswerRecord = c.record;
    assert.equal(await answerSigningInput(ctx, c.offer_nonce, record), c.signing_input, c.name);

    const { sig: _sig, created_at: _created, ...draft } = record;
    const written = await signAnswer(ctx, host.privateKey, c.offer_nonce, draft);
    assert.equal(written.sig, record.sig, c.name);

    const verified = await verifyAnswer({ ...ctx, record, offerNonce: c.offer_nonce, expectedHostKey: signing.keys.host.public_b64u });
    assert.deepEqual(verified, { ok: true, value: record }, c.name);
  }
});

test('signing inputs have no trailing newline, the plan\'s line counts, and "-" for absent fields', () => {
  for (const c of signing.offers) {
    assert.ok(!c.signing_input.endsWith('\n'));
    assert.equal(c.signing_input.split('\n').length, 13, c.name);
    assert.equal(c.signing_input.split('\n')[0], 'lingxi-rc-offer/1');
  }
  const tunnel = signing.offers[2];
  assert.equal(tunnel.signing_input.split('\n')[10], '-', 'no sdp hashes to "-"');
  for (const c of signing.answers) {
    assert.equal(c.signing_input.split('\n').length, 11, c.name);
    assert.equal(c.signing_input.split('\n')[0], 'lingxi-rc-answer/1');
  }
  const rejected = signing.answers[2].signing_input.split('\n');
  assert.deepEqual(rejected.slice(4), [signing.answers[2].offer_nonce, 'rejected', 'revoked', '-', '-', '-', signing.answers[2].record.nonce]);
});

test('free text enters the signing input as b64u(UTF-8) and the sdp as the b64u of its SHA-256', () => {
  const lines = signing.offers[1].signing_input.split('\n');
  assert.equal(b64uEncode(new TextEncoder().encode('小明的 iPhone 📱')), lines[6]);
  assert.equal(b64uEncode(new TextEncoder().encode('2.0.0-β')), lines[8]);
  assert.equal(lines[10], signing.offers[1].sdp_sha256_b64u);
});

test('a signing input refuses a line break inside a raw field', async () => {
  const c = signing.offers[0];
  await assert.rejects(offerSigningInput({ uid: 'a\nb', hostId: c.host_id, connId: c.conn_id }, c.record), RemoteProtocolError);
  await assert.rejects(offerSigningInput({ uid: c.uid, hostId: c.host_id, connId: 'x\r' }, c.record), RemoteProtocolError);
});

// ── Tampering ────────────────────────────────────────────────────────────────

const OFFER = signing.offers[0];
const OFFER_CTX = { uid: OFFER.uid, hostId: OFFER.host_id, connId: OFFER.conn_id };

async function verifyTampered(mutate: (record: OfferRecord) => void, ctx = OFFER_CTX) {
  const record = clone<OfferRecord>(OFFER.record);
  mutate(record);
  return verifyOffer({ ...ctx, record, now: NOW });
}

test('changing any signed offer field breaks verification, including one character of the sdp', async () => {
  const cases: Array<[string, (r: OfferRecord) => void]> = [
    ['device.name', (r) => { r.device.name = 'Test Phone '; }],
    ['device.platform', (r) => { r.device.platform = 'ios'; }],
    ['device.app_version', (r) => { r.device.app_version = '1.2.4'; }],
    ['eph', (r) => { r.eph = signing.ephemeral_public_b64u.viewer_2; }],
    ['nonce', (r) => { r.nonce = r.nonce.slice(0, 21) + (r.nonce.endsWith('w') ? 'Q' : 'w'); }],
    ['sdp (ice-ufrag)', (r) => { r.sdp = r.sdp!.replace('aBcD', 'aBcE'); }],
    ['sdp (fingerprint)', (r) => { r.sdp = r.sdp!.replace('A4:CE', 'A4:CF'); }],
    ['sdp (line ending)', (r) => { r.sdp = r.sdp!.replace(/\r\n$/, '\n'); }],
    ['transport', (r) => { r.transport = 'tunnel'; delete r.sdp; }],
    ['device key and id together', (r) => { r.device.key = signing.keys.device2.public_b64u; r.device.id = signing.keys.device2.id; }],
    ['signature bit', (r) => { const sig = b64uDecode(r.sig); sig[5] ^= 1; r.sig = b64uEncode(sig); }],
  ];
  for (const [field, mutate] of cases) {
    const result = await verifyTampered(mutate);
    assert.deepEqual(result.ok, false, field);
    assert.equal(!result.ok && result.reason, 'bad_signature', field);
  }
});

test('an offer is bound to its uid, host_id and conn_id', async () => {
  const ok = await verifyTampered(() => {});
  assert.equal(ok.ok, true);
  for (const ctx of [
    { ...OFFER_CTX, uid: 'someoneElse' },
    { ...OFFER_CTX, hostId: signing.keys.device.id },
    { ...OFFER_CTX, connId: signing.offers[1].conn_id },
  ]) {
    const result = await verifyTampered(() => {}, ctx);
    assert.equal(!result.ok && result.reason, 'bad_signature', JSON.stringify(ctx));
  }
});

test('a device id that does not belong to the device key is an id mismatch, before any signature work', async () => {
  const result = await verifyTampered((r) => { r.device.key = signing.keys.device2.public_b64u; });
  assert.deepEqual(result.ok, false);
  assert.equal(!result.ok && result.reason, 'id_mismatch');
});

test('created_at is not signed, only checked against the clock: five minutes either way', async () => {
  const within = await verifyTampered((r) => { r.created_at += 1234; });
  assert.equal(within.ok, true);
  const outcome = async (now: number, maxAgeMs?: number) => {
    const result = await verifyOffer({ ...OFFER_CTX, record: OFFER.record, now, ...(maxAgeMs === undefined ? {} : { maxAgeMs }) });
    return result.ok ? 'ok' : result.reason;
  };
  assert.equal(await outcome(NOW + 300_000), 'ok', 'exactly 5 minutes old');
  assert.equal(await outcome(NOW + 300_001), 'expired');
  assert.equal(await outcome(NOW - 300_000), 'ok');
  assert.equal(await outcome(NOW - 300_001), 'expired', 'an offer from the future is just as bad');
  assert.equal(await outcome(NOW + 10_001, 10_000), 'expired');
  assert.equal(await outcome(NOW + 10_000, 10_000), 'ok');
});

test('an answer is bound to the offer nonce, the expected host key, the host id and its own contents', async () => {
  const c = signing.answers[1]; // accepted, tunnel
  const base = { uid: c.uid, hostId: c.host_id, connId: c.conn_id, record: c.record, offerNonce: c.offer_nonce, expectedHostKey: signing.keys.host.public_b64u };
  assert.equal((await verifyAnswer(base)).ok, true);

  const reason = async (patch: Partial<typeof base>) => {
    const r = await verifyAnswer({ ...base, ...patch });
    return r.ok ? 'ok' : r.reason;
  };
  assert.equal(await reason({ offerNonce: signing.answers[0].offer_nonce }), 'bad_signature', 'answer replayed for another offer');
  assert.equal(await reason({ connId: signing.answers[0].conn_id }), 'bad_signature');
  assert.equal(await reason({ uid: 'other' }), 'bad_signature');
  assert.equal(await reason({ expectedHostKey: signing.keys.device.public_b64u }), 'key_mismatch', 'not the pinned key');
  assert.equal(await reason({ hostId: signing.keys.device.id }), 'id_mismatch', 'answer from a host that was not dialled');

  const mutated = async (mutate: (r: AnswerRecord) => void) => {
    const record = clone<AnswerRecord>(c.record);
    mutate(record);
    const r = await verifyAnswer({ ...base, record });
    return r.ok ? 'ok' : r.reason;
  };
  assert.equal(await mutated((r) => { r.eph = signing.ephemeral_public_b64u.viewer_1; }), 'bad_signature', 'host eph is signed');
  assert.equal(await mutated((r) => { r.nonce = signing.answers[0].record.nonce; }), 'bad_signature');
  assert.equal(await mutated((r) => { r.transport = 'webrtc'; delete r.eph; r.sdp = 'v=0'; }), 'bad_signature');
  assert.equal(await mutated((r) => { r.host.key = signing.keys.device.public_b64u; }), 'id_mismatch', 'host.id does not match host.key');
  assert.equal(await mutated((r) => { r.created_at += 1; }), 'ok', 'created_at is not signed');

  // A host with a self-consistent id and a valid signature, but not the pinned key: key_mismatch.
  const impostor = await keyOf(signing.keys.device);
  const forged = await signAnswer({ uid: c.uid, hostId: signing.keys.device.id, connId: c.conn_id }, impostor.privateKey, c.offer_nonce, {
    v: 1, status: 'rejected', reason: 'busy', host: { id: signing.keys.device.id, key: signing.keys.device.public_b64u }, nonce: c.record.nonce,
  });
  const forgedResult = await verifyAnswer({ ...base, hostId: signing.keys.device.id, record: { ...forged, created_at: NOW } });
  assert.equal(!forgedResult.ok && forgedResult.reason, 'key_mismatch');
});

test('an answer can be held to an age limit when the caller wants one', async () => {
  const c = signing.answers[0];
  const base = { uid: c.uid, hostId: c.host_id, connId: c.conn_id, record: c.record, offerNonce: c.offer_nonce, expectedHostKey: signing.keys.host.public_b64u };
  assert.equal((await verifyAnswer({ ...base, now: NOW + 10_000, maxAgeMs: 30_000 })).ok, true);
  const late = await verifyAnswer({ ...base, now: NOW + 31_000, maxAgeMs: 30_000 });
  assert.equal(!late.ok && late.reason, 'expired');
  assert.equal((await verifyAnswer({ ...base, now: NOW + 3_600_000 })).ok, true, 'no limit unless asked');
});

// ── Strict parsing ───────────────────────────────────────────────────────────

test('parseOfferRecord accepts the fixtures and ignores unknown fields without copying them', () => {
  for (const c of signing.offers) {
    const parsed = parseOfferRecord(c.record);
    assert.equal(parsed.ok, true, c.name);
    assert.deepEqual(parsed.ok && parsed.value, c.record);
  }
  const extra = { ...clone(OFFER.record), future_field: 'x', device: { ...OFFER.record.device, extra: 1 } };
  const parsed = parseOfferRecord(extra);
  assert.equal(parsed.ok, true);
  assert.deepEqual(parsed.ok && parsed.value, OFFER.record);
});

test('parseOfferRecord rejects every malformed shape with a reason and never throws', () => {
  const bad = (name: string, mutate: (r: any) => void) => { // eslint-disable-line @typescript-eslint/no-explicit-any
    const record = clone<any>(OFFER.record); // eslint-disable-line @typescript-eslint/no-explicit-any
    mutate(record);
    const parsed = parseOfferRecord(record);
    assert.equal(parsed.ok, false, name);
    assert.ok(!parsed.ok && parsed.error.length > 0, name);
  };
  for (const raw of [null, undefined, 42, 'offer', [], true]) assert.equal(parseOfferRecord(raw).ok, false);
  bad('v', (r) => { r.v = 2; });
  bad('missing v', (r) => { delete r.v; });
  bad('transport', (r) => { r.transport = 'carrier-pigeon'; });
  bad('webrtc without sdp', (r) => { delete r.sdp; });
  bad('empty sdp', (r) => { r.sdp = ''; });
  bad('sdp not a string', (r) => { r.sdp = 5; });
  bad('sdp too long', (r) => { r.sdp = 'v'.repeat(16385); });
  bad('tunnel with sdp', (r) => { r.transport = 'tunnel'; });
  bad('device missing', (r) => { delete r.device; });
  bad('device not an object', (r) => { r.device = 'phone'; });
  bad('device.id wrong length', (r) => { r.device.id = r.device.id.slice(1); });
  bad('device.id bad alphabet', (r) => { r.device.id = `${r.device.id.slice(1)}=`; });
  bad('device.key wrong length', (r) => { r.device.key = r.device.key.slice(1); });
  bad('device.key not base64url', (r) => { r.device.key = `${r.device.key.slice(1)}+`; });
  bad('device.name empty', (r) => { r.device.name = ''; });
  bad('device.name too long', (r) => { r.device.name = 'n'.repeat(129); });
  bad('device.name newline', (r) => { r.device.name = 'Phone\nApprove?'; });
  bad('device.name C1 control', (r) => { r.device.name = 'Phone\u0085'; });
  bad('device.name not a string', (r) => { r.device.name = ['x']; });
  bad('device.platform', (r) => { r.device.platform = 'windows'; });
  bad('device.app_version empty', (r) => { r.device.app_version = ''; });
  bad('device.app_version too long', (r) => { r.device.app_version = '1'.repeat(65); });
  bad('eph size', (r) => { r.eph = r.eph.slice(0, 42); });
  bad('nonce size', (r) => { r.nonce = `${r.nonce}A`; });
  bad('created_at string', (r) => { r.created_at = '1760000000000'; });
  bad('created_at negative', (r) => { r.created_at = -1; });
  bad('created_at null', (r) => { r.created_at = null; });
  bad('sig size', (r) => { r.sig = r.sig.slice(0, 85); });
  bad('sig missing', (r) => { delete r.sig; });
});

test('the sdp bound is 16384 characters: that many pass, one more does not', () => {
  const at = clone<OfferRecord>(OFFER.record);
  at.sdp = 's'.repeat(16384);
  assert.equal(parseOfferRecord(at).ok, true);
  at.sdp = 's'.repeat(16385);
  assert.equal(parseOfferRecord(at).ok, false);
});

test('parseAnswerRecord accepts the fixtures and enforces the status rules', () => {
  for (const c of signing.answers) assert.deepEqual(parseAnswerRecord(c.record), { ok: true, value: c.record }, c.name);

  const bad = (name: string, base: number, mutate: (r: any) => void) => { // eslint-disable-line @typescript-eslint/no-explicit-any
    const record = clone<any>(signing.answers[base].record); // eslint-disable-line @typescript-eslint/no-explicit-any
    mutate(record);
    assert.equal(parseAnswerRecord(record).ok, false, name);
  };
  for (const raw of [null, 1, 'x', []]) assert.equal(parseAnswerRecord(raw).ok, false);
  bad('v', 0, (r) => { r.v = 0; });
  bad('status', 0, (r) => { r.status = 'maybe'; });
  bad('accepted with a reason', 0, (r) => { r.reason = 'busy'; });
  bad('rejected with an unknown reason', 2, (r) => { r.reason = 'because'; });
  bad('accepted without transport', 0, (r) => { delete r.transport; });
  bad('accepted webrtc without sdp', 0, (r) => { delete r.sdp; });
  bad('accepted tunnel with sdp', 1, (r) => { r.sdp = 'v=0'; });
  bad('accepted tunnel without eph', 1, (r) => { delete r.eph; });
  bad('eph size', 1, (r) => { r.eph = 'AAAA'; });
  bad('sdp too long', 0, (r) => { r.sdp = 'v'.repeat(16385); });
  bad('host missing', 0, (r) => { delete r.host; });
  bad('host.id', 0, (r) => { r.host.id = 'short'; });
  bad('host.key', 0, (r) => { r.host.key = 'short'; });
  bad('nonce', 0, (r) => { r.nonce = 'short'; });
  bad('created_at', 0, (r) => { r.created_at = 'now'; });
  bad('sig', 0, (r) => { r.sig = 'short'; });
  const rejected = parseAnswerRecord({ ...clone(signing.answers[2].record), reason: undefined });
  assert.equal(rejected.ok, true, 'a rejection may omit the reason');
});

test('parseHostRecord validates the heartbeat record', () => {
  const host: HostRecord = {
    v: 1, name: 'Studio Mac', platform: 'darwin', app_version: '3.1.0', client_protocol_version: 29,
    host_key: signing.keys.host.public_b64u, online: true, last_seen: NOW, started_at: NOW - 1000,
  };
  assert.deepEqual(parseHostRecord(host), { ok: true, value: host });
  assert.deepEqual(parseHostRecord({ ...host, extra: 1 }), { ok: true, value: host });
  for (const [name, patch] of Object.entries({
    v: { v: 2 }, name: { name: '' }, nameNewline: { name: 'a\nb' }, platform: { platform: 'x'.repeat(33) }, version: { app_version: '' },
    protocol: { client_protocol_version: '29' }, protocolFraction: { client_protocol_version: 29.5 }, key: { host_key: 'abc' },
    online: { online: 1 }, lastSeen: { last_seen: null }, startedAt: { started_at: 'x' },
  })) {
    assert.equal(parseHostRecord({ ...host, ...patch }).ok, false, name);
  }
  for (const raw of [null, undefined, [], 'host', 7]) assert.equal(parseHostRecord(raw).ok, false);
  const missing: Record<string, unknown> = { ...host };
  delete missing['last_seen'];
  assert.equal(parseHostRecord(missing).ok, false);
});

// ── Paths ────────────────────────────────────────────────────────────────────

test('RTDB paths follow the plan\'s layout', () => {
  const uid = 'U1';
  const host = 'H1';
  const conn = 'C1';
  assert.equal(hostsPath(uid), '/remote-control/U1/hosts');
  assert.equal(hostPath(uid, host), '/remote-control/U1/hosts/H1');
  assert.equal(toHostPath(uid, host), '/remote-control/U1/to_host/H1');
  assert.equal(toHostConnPath(uid, host, conn), '/remote-control/U1/to_host/H1/C1');
  assert.equal(offerPath(uid, host, conn), '/remote-control/U1/to_host/H1/C1/offer');
  assert.equal(upPath(uid, host, conn), '/remote-control/U1/to_host/H1/C1/up');
  assert.equal(upFramePath(uid, host, conn, 12), '/remote-control/U1/to_host/H1/C1/up/f12');
  assert.equal(toViewerPath(uid, host), '/remote-control/U1/to_viewer/H1');
  assert.equal(toViewerConnPath(uid, host, conn), '/remote-control/U1/to_viewer/H1/C1');
  assert.equal(answerPath(uid, host, conn), '/remote-control/U1/to_viewer/H1/C1/answer');
  assert.equal(downPath(uid, host, conn), '/remote-control/U1/to_viewer/H1/C1/down');
  assert.equal(downFramePath(uid, host, conn, 0), '/remote-control/U1/to_viewer/H1/C1/down/f0');
});

test('every path segment is validated: non-empty, none of . # $ [ ] /, no control characters', () => {
  for (const good of ['abc', 'A-b_c', '22charactersLongIdHere', 'nssiE2PAvtL12AM03ivuG-', '小明', 'a b']) {
    assert.equal(isPathSegment(good), true, good);
  }
  for (const bad of ['', 'a.b', 'a#b', 'a$b', 'a[b', 'a]b', 'a/b', '/', 'a\u0000b', 'a\nb', 'a\u001fb', 'a\u007fb', 'x'.repeat(769)]) {
    assert.equal(isPathSegment(bad), false, JSON.stringify(bad));
  }
  assert.equal(isPathSegment(5 as unknown), false);
  assert.equal(isPathSegment(undefined), false);
  assert.equal(assertPathSegment('ok'), 'ok');
  assert.throws(() => assertPathSegment('a.b', 'conn_id'), /invalid conn_id/);

  assert.throws(() => hostPath('', 'h'), RemoteProtocolError);
  assert.throws(() => hostPath('u', 'h/..'), RemoteProtocolError);
  assert.throws(() => offerPath('u', 'h', 'c.d'), RemoteProtocolError);
  assert.throws(() => toViewerConnPath('u/x', 'h', 'c'), RemoteProtocolError);
  assert.throws(() => downPath('u', '$h', 'c'), RemoteProtocolError);
});

test('frame keys carry an f prefix; the reader also understands the array form', () => {
  assert.equal(frameKey(0), 'f0');
  assert.equal(frameKey(12), 'f12');
  assert.equal(frameKey(9_999_999_999), 'f9999999999');
  for (const bad of [-1, 1.5, 10_000_000_000, Number.NaN, 2 ** 53]) assert.throws(() => frameKey(bad), RemoteProtocolError, String(bad));

  assert.equal(parseFrameKey('f0'), 0);
  assert.equal(parseFrameKey('f12'), 12);
  assert.equal(parseFrameKey('f9999999999'), 9_999_999_999);
  assert.equal(parseFrameKey('12'), 12, 'array form');
  assert.equal(parseFrameKey('0'), 0);
  for (const bad of ['', 'f', 'f01', '01', 'f-1', 'f1.5', 'g1', 'F1', 'f10000000000', '10000000000', 'f 1', 'offer', 'answer']) {
    assert.equal(parseFrameKey(bad), null, JSON.stringify(bad));
  }
});

// ── Presence ─────────────────────────────────────────────────────────────────

const host = (patch: Partial<HostRecord> = {}): HostRecord => ({
  v: 1, name: 'Mac', platform: 'darwin', app_version: '1', client_protocol_version: 29,
  host_key: signing.keys.host.public_b64u, online: true, last_seen: 1_000_000, started_at: 900_000, ...patch,
});

test('isHostOnline: the flag is required; receive time wins when known, else last_seen against the local clock', () => {
  assert.equal(isHostOnline(host({ online: false }), 1_000_000, 1_000_000), false);
  assert.equal(isHostOnline(host({ online: false }), 1_000_000), false);

  // First load: no receive time, compare last_seen with now in either direction.
  assert.equal(isHostOnline(host(), 1_000_000 + 89_999), true);
  assert.equal(isHostOnline(host(), 1_000_000 + 90_000), false);
  assert.equal(isHostOnline(host(), 1_000_000 - 89_999), true, 'local clock behind the host\'s');
  assert.equal(isHostOnline(host(), 1_000_000 - 90_000), false);

  // After an update was received: last_seen is ignored, however skewed.
  assert.equal(isHostOnline(host({ last_seen: 1 }), 5_000_000 + 89_999, 5_000_000), true);
  assert.equal(isHostOnline(host({ last_seen: 1 }), 5_000_000 + 90_000, 5_000_000), false);
  assert.equal(isHostOnline(host({ last_seen: 9e12 }), 5_000_000 + 10, 5_000_000), true);
});

test('HostPresence stamps a heartbeat with the local clock only when last_seen changes', () => {
  const clock = new ManualClock(2_000_000);
  const presence = new HostPresence(clock);
  assert.equal(presence.isOnline('h'), false, 'never observed');

  // Loaded with a host clock 10 minutes off: online only by the skew rule, so offline.
  presence.observe('h', host({ last_seen: 1_400_000 }));
  assert.equal(presence.isOnline('h'), false);

  // The first heartbeat we actually receive switches us to local receive time.
  clock.advance(1_000);
  presence.observe('h', host({ last_seen: 1_430_000 }));
  assert.equal(presence.isOnline('h'), true);

  // Re-observing the same last_seen (an unrelated field changed) does not refresh it.
  clock.advance(60_000);
  presence.observe('h', host({ last_seen: 1_430_000, name: 'Renamed' }));
  clock.advance(30_001);
  assert.equal(presence.isOnline('h'), false, '91 s since the last heartbeat arrived');

  presence.observe('h', host({ last_seen: 1_460_000 }));
  assert.equal(presence.isOnline('h'), true);
  presence.observe('h', host({ last_seen: 1_460_000, online: false }));
  assert.equal(presence.isOnline('h'), false, 'an explicit offline flag always wins');

  presence.observe('h', host({ last_seen: 1_490_000, online: true }));
  assert.equal(presence.isOnline('h'), true);
  presence.forget('h');
  assert.equal(presence.isOnline('h'), false);
});

test('HostPresence on first load trusts a host whose last_seen is close to the local clock', () => {
  const clock = new ManualClock(5_000_000);
  const presence = new HostPresence(clock);
  presence.observe('fresh', host({ last_seen: 4_950_000 }));
  presence.observe('stale', host({ last_seen: 4_000_000 }));
  assert.equal(presence.isOnline('fresh'), true);
  assert.equal(presence.isOnline('stale'), false);
  clock.advance(41_000);
  assert.equal(presence.isOnline('fresh'), false, 'no heartbeat arrived and last_seen aged past 90 s');
});
