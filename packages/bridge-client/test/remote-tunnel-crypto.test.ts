/**
 * Tunnel cryptography against the shared fixtures: key agreement and
 * derivation, sealed frames in both directions at several sequence numbers,
 * and every way a frame must fail to open.
 */

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { b64uDecode, b64uEncode, concatBytes, utf8Encode } from '../src/remote/encoding.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import { ephemeralKeyFromSeed, hkdfSha256, x25519 } from '../src/remote/keys.js';
import {
  deriveTunnelKeys,
  openFrame,
  packFragments,
  sealFrame,
  tunnelAad,
  tunnelNonce,
  TunnelCryptoError,
  unpackFragments,
  type TunnelDir,
} from '../src/remote/tunnelCrypto.js';
import { FRAGMENT_MAX_BYTES, TUNNEL_FRAME_PLAINTEXT_MAX } from '../src/remote/version.js';
import { fixture, hex, unhex } from './remote-helpers.js';

interface Frame {
  name: string;
  dir: TunnelDir;
  seq: number;
  key: string;
  fragments_hex: string[];
  plaintext_hex: string;
  nonce_hex: string;
  aad_utf8: string;
  aad_hex: string;
  stored_b64u: string;
}
interface LargeFrame {
  name: string;
  dir: TunnelDir;
  seq: number;
  fragments: Array<{ kind: number; payload_length: number; fill_hex: string }>;
  plaintext_length: number;
  plaintext_sha256_hex: string;
  stored_length: number;
  stored_sha256_hex: string;
}
interface OpenFailure {
  name: string;
  dir: TunnelDir;
  seq: number;
  conn_id: string;
  stored_b64u: string;
}

const tunnel = fixture<{
  conn_id: string;
  viewer_eph: { seed_hex: string; public_hex: string };
  host_eph: { seed_hex: string; public_hex: string };
  shared_hex: string;
  hkdf: { salt_utf8: string; info_up_utf8: string; info_down_utf8: string; k_up_hex: string; k_down_hex: string };
  frames: Frame[];
  large_frames: LargeFrame[];
  open_failures: OpenFailure[];
  unpack_failures: Array<{ name: string; plaintext_hex?: string; fragments?: Array<{ length: number; fill_hex: string }> }>;
}>('tunnel');

const sha = (bytes: Uint8Array | string): string => createHash('sha256').update(bytes).digest('hex');

async function keys() {
  return deriveTunnelKeys(unhex(tunnel.shared_hex), tunnel.conn_id);
}

function withPrefix(fragment: Uint8Array): Uint8Array {
  const out = new Uint8Array(4 + fragment.length);
  new DataView(out.buffer).setUint32(0, fragment.length);
  out.set(fragment, 4);
  return out;
}

function largePlaintext(frame: LargeFrame): Uint8Array {
  const fragments = frame.fragments.map(({ kind, payload_length, fill_hex }) => {
    const fragment = new Uint8Array(1 + payload_length).fill(unhex(fill_hex)[0]!);
    fragment[0] = kind;
    return fragment;
  });
  return packFragments(fragments);
}

// ── Key agreement and derivation ─────────────────────────────────────────────

test('both ends agree on the fixture\'s shared secret, from seeds to HKDF output', async () => {
  const viewer = await ephemeralKeyFromSeed(unhex(tunnel.viewer_eph.seed_hex));
  const host = await ephemeralKeyFromSeed(unhex(tunnel.host_eph.seed_hex));
  assert.equal(hex(viewer.publicKey), tunnel.viewer_eph.public_hex);
  assert.equal(hex(host.publicKey), tunnel.host_eph.public_hex);
  assert.equal(hex(await x25519(viewer.privateKey, host.publicKey)), tunnel.shared_hex);
  assert.equal(hex(await x25519(host.privateKey, viewer.publicKey)), tunnel.shared_hex);

  const shared = unhex(tunnel.shared_hex);
  const salt = utf8Encode(tunnel.hkdf.salt_utf8);
  assert.equal(tunnel.hkdf.salt_utf8, tunnel.conn_id);
  assert.equal(hex(await hkdfSha256(shared, salt, utf8Encode(tunnel.hkdf.info_up_utf8), 32)), tunnel.hkdf.k_up_hex);
  assert.equal(hex(await hkdfSha256(shared, salt, utf8Encode(tunnel.hkdf.info_down_utf8), 32)), tunnel.hkdf.k_down_hex);
  assert.equal(tunnel.hkdf.info_up_utf8, 'lingxi-rc-tunnel/1 up');
  assert.equal(tunnel.hkdf.info_down_utf8, 'lingxi-rc-tunnel/1 down');
  assert.notEqual(tunnel.hkdf.k_up_hex, tunnel.hkdf.k_down_hex);
});

// ── Sealed frames ────────────────────────────────────────────────────────────

test('nonce and AAD match the fixtures for every frame', () => {
  for (const f of tunnel.frames) {
    assert.equal(hex(tunnelNonce(f.seq)), f.nonce_hex, f.name);
    assert.equal(new TextDecoder().decode(tunnelAad(tunnel.conn_id, f.dir, f.seq)), f.aad_utf8, f.name);
    assert.equal(hex(tunnelAad(tunnel.conn_id, f.dir, f.seq)), f.aad_hex, f.name);
    assert.equal(f.aad_utf8, `lingxi-rc-tunnel/1\n${tunnel.conn_id}\n${f.dir}\n${f.seq}`);
    assert.equal(f.key, `f${f.seq}`);
  }
  assert.equal(hex(tunnelNonce(0)), '000000000000000000000000');
  assert.equal(hex(tunnelNonce(1)), '000000000000000000000001');
  assert.equal(hex(tunnelNonce(4294967296)), '000000000000000100000000');
  assert.equal(hex(tunnelNonce(Number.MAX_SAFE_INTEGER)), '00000000' + '001fffffffffffff');
});

test('sealing reproduces every fixture frame, and opening returns the plaintext', async () => {
  const { up, down } = await keys();
  const byDir = { up, down };
  assert.ok(tunnel.frames.length >= 6);
  for (const f of tunnel.frames) {
    const plaintext = unhex(f.plaintext_hex);
    assert.equal(await sealFrame(byDir[f.dir], tunnel.conn_id, f.dir, f.seq, plaintext), f.stored_b64u, f.name);
    assert.deepEqual(await openFrame(byDir[f.dir], tunnel.conn_id, f.dir, f.seq, f.stored_b64u), plaintext, f.name);
    assert.deepEqual(unpackFragments(plaintext).map(hex), f.fragments_hex, f.name);
    assert.deepEqual(packFragments(f.fragments_hex.map(unhex)), plaintext, f.name);
  }
});

test('the same plaintext seals differently per direction and per sequence number', async () => {
  const { up, down } = await keys();
  const plaintext = unhex(tunnel.frames[0]!.plaintext_hex);
  const stored = new Set([
    await sealFrame(up, tunnel.conn_id, 'up', 0, plaintext),
    await sealFrame(down, tunnel.conn_id, 'down', 0, plaintext),
    await sealFrame(up, tunnel.conn_id, 'up', 1, plaintext),
    await sealFrame(up, 'AAAAAAAAAAAAAAAAAAAAAA', 'up', 0, plaintext),
  ]);
  assert.equal(stored.size, 4);
});

test('large frames match by length and hash, including the exactly-full 65536-character frame', async () => {
  const { up, down } = await keys();
  const byDir = { up, down };
  assert.equal(tunnel.large_frames.length, 2);
  for (const f of tunnel.large_frames) {
    const plaintext = largePlaintext(f);
    assert.equal(plaintext.length, f.plaintext_length, f.name);
    assert.equal(sha(plaintext), f.plaintext_sha256_hex, f.name);
    const stored = await sealFrame(byDir[f.dir], tunnel.conn_id, f.dir, f.seq, plaintext);
    assert.equal(stored.length, f.stored_length, f.name);
    assert.equal(sha(stored), f.stored_sha256_hex, f.name);
    assert.deepEqual(await openFrame(byDir[f.dir], tunnel.conn_id, f.dir, f.seq, stored), plaintext, f.name);
  }
  const full = tunnel.large_frames[1]!;
  assert.equal(full.plaintext_length, TUNNEL_FRAME_PLAINTEXT_MAX);
  assert.equal(full.stored_length, 65536);
});

test('a plaintext above 49136 bytes is refused', async () => {
  const { up } = await keys();
  await assert.rejects(sealFrame(up, tunnel.conn_id, 'up', 0, new Uint8Array(TUNNEL_FRAME_PLAINTEXT_MAX + 1)), RemoteProtocolError);
  await sealFrame(up, tunnel.conn_id, 'up', 0, new Uint8Array(TUNNEL_FRAME_PLAINTEXT_MAX));
  await sealFrame(up, tunnel.conn_id, 'up', 0, new Uint8Array(0));
});

// ── Tampering ────────────────────────────────────────────────────────────────

test('a frame fails to open under a wrong conn_id, direction, sequence number or key', async () => {
  const { up, down } = await keys();
  const f = tunnel.frames[0]!; // up, seq 0
  assert.deepEqual(await openFrame(up, tunnel.conn_id, 'up', 0, f.stored_b64u), unhex(f.plaintext_hex));

  const attempts: Array<[string, () => Promise<unknown>]> = [
    ['conn_id', () => openFrame(up, 'BBBBBBBBBBBBBBBBBBBBBB', 'up', 0, f.stored_b64u)],
    ['direction (AAD only)', () => openFrame(up, tunnel.conn_id, 'down', 0, f.stored_b64u)],
    ['sequence number', () => openFrame(up, tunnel.conn_id, 'up', 1, f.stored_b64u)],
    ['key', () => openFrame(down, tunnel.conn_id, 'up', 0, f.stored_b64u)],
    ['key and direction swapped', () => openFrame(down, tunnel.conn_id, 'down', 0, f.stored_b64u)],
  ];
  for (const [what, attempt] of attempts) {
    await assert.rejects(attempt(), TunnelCryptoError, what);
  }
});

test('every byte of the stored value is covered by the tag', async () => {
  const { up } = await keys();
  const f = tunnel.frames[2]!;
  const sealed = b64uDecode(f.stored_b64u);
  for (const index of [0, 1, Math.floor(sealed.length / 2), sealed.length - 17, sealed.length - 16, sealed.length - 1]) {
    const copy = sealed.slice();
    copy[index] ^= 0x01;
    await assert.rejects(openFrame(up, tunnel.conn_id, f.dir, f.seq, b64uEncode(copy)), TunnelCryptoError, `byte ${index}`);
  }
  await assert.rejects(openFrame(up, tunnel.conn_id, f.dir, f.seq, b64uEncode(sealed.subarray(0, sealed.length - 1))), TunnelCryptoError);
  await assert.rejects(openFrame(up, tunnel.conn_id, f.dir, f.seq, b64uEncode(new Uint8Array([...sealed, 0]))), TunnelCryptoError);
});

test('the fixture\'s open failures all fail with TunnelCryptoError', async () => {
  const { up } = await keys();
  assert.ok(tunnel.open_failures.length >= 6);
  for (const f of tunnel.open_failures) {
    await assert.rejects(openFrame(up, f.conn_id, f.dir, f.seq, f.stored_b64u), TunnelCryptoError, f.name);
  }
});

test('malformed stored values are TunnelCryptoErrors, not stray exceptions', async () => {
  const { up } = await keys();
  for (const stored of ['', 'AAAA', '!!!!', 'QQ==', 'a'.repeat(65537), b64uEncode(new Uint8Array(15)), b64uEncode(new Uint8Array(49153 + 16))]) {
    await assert.rejects(openFrame(up, tunnel.conn_id, 'up', 0, stored), TunnelCryptoError, stored.slice(0, 20));
  }
  await assert.rejects(openFrame(up, tunnel.conn_id, 'up', -1, 'AAAA'), RemoteProtocolError);
  assert.throws(() => tunnelNonce(-1), RemoteProtocolError);
  assert.throws(() => tunnelNonce(1.5), RemoteProtocolError);
  assert.throws(() => tunnelAad('c', 'up', 2 ** 53), RemoteProtocolError);
});

// ── Packing ──────────────────────────────────────────────────────────────────

test('packFragments / unpackFragments round-trip and keep the [u32 BE length][fragment] layout', () => {
  const fragments = [Uint8Array.from([0, 1, 2]), Uint8Array.from([1]), new Uint8Array(16384).fill(7)];
  const packed = packFragments(fragments);
  assert.equal(packed.length, 4 + 3 + 4 + 1 + 4 + 16384);
  assert.equal(hex(packed.subarray(0, 12)), '00000003' + '000102' + '00000001' + '01');
  assert.deepEqual(unpackFragments(packed), fragments);
  assert.deepEqual(unpackFragments(new Uint8Array(0)), []);
  assert.deepEqual(packFragments([]), new Uint8Array(0));
});

test('the 49136-byte plaintext bound is exact for packing and unpacking', () => {
  // Three length prefixes take 12 bytes, so the largest triple is 16384 + 16384 + 16356 bytes of fragment.
  const largest = [new Uint8Array(16384).fill(1), new Uint8Array(16384).fill(1), new Uint8Array(16356).fill(1)];
  const packed = packFragments(largest);
  assert.equal(packed.length, TUNNEL_FRAME_PLAINTEXT_MAX);
  assert.deepEqual(unpackFragments(packed), largest);

  const oneMore = [largest[0]!, largest[1]!, new Uint8Array(16357).fill(1)];
  assert.throws(() => packFragments(oneMore), RemoteProtocolError);
  assert.throws(() => packFragments([...largest, new Uint8Array(1)]), RemoteProtocolError);
  assert.throws(() => unpackFragments(new Uint8Array(TUNNEL_FRAME_PLAINTEXT_MAX + 1)), RemoteProtocolError);
});

test('unpackFragments rejects truncated, empty and over-long fragments (fixture cases and boundaries)', () => {
  for (const c of tunnel.unpack_failures) {
    const plaintext = c.plaintext_hex !== undefined
      ? unhex(c.plaintext_hex)
      : concatBytes(...c.fragments!.map(({ length, fill_hex }) => withPrefix(new Uint8Array(length).fill(unhex(fill_hex)[0]!))));
    assert.throws(() => unpackFragments(plaintext), RemoteProtocolError, c.name);
  }
  assert.throws(() => unpackFragments(unhex('00')), RemoteProtocolError);
  assert.deepEqual(unpackFragments(unhex('0000000100')), [Uint8Array.from([0])], 'a one-byte fragment is fine');
});

test('fragments up to exactly 16384 bytes unpack; 16385 do not', () => {
  const ok = packFragments([new Uint8Array(FRAGMENT_MAX_BYTES)]);
  assert.equal(unpackFragments(ok)[0]!.length, FRAGMENT_MAX_BYTES);
  const over = new Uint8Array(4 + FRAGMENT_MAX_BYTES + 1);
  new DataView(over.buffer).setUint32(0, FRAGMENT_MAX_BYTES + 1);
  assert.throws(() => unpackFragments(over), /impossible size/);
});
