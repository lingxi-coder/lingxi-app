/**
 * Byte and text helpers of the remote-control core: base64url, UTF-8, digests,
 * ids, byte-string utilities, and the protocol constants that derive from them.
 */

import assert from 'node:assert/strict';
import { createHash, randomBytes as nodeRandomBytes } from 'node:crypto';
import { test } from 'node:test';

import { CLIENT_PROTOCOL_VERSION } from '../src/protocol.js';
import {
  b64uDecode,
  b64uEncode,
  bytesEqual,
  concatBytes,
  isB64u,
  randomBytes,
  randomId,
  sha256,
  utf8Decode,
  utf8Encode,
} from '../src/remote/encoding.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import * as version from '../src/remote/version.js';
import { hex } from './remote-helpers.js';

test('b64u matches Node base64url for every length and never pads', () => {
  for (let length = 0; length <= 70; length++) {
    const bytes = new Uint8Array(nodeRandomBytes(length));
    const text = b64uEncode(bytes);
    assert.equal(text, Buffer.from(bytes).toString('base64url'), `length ${length}`);
    assert.ok(!text.includes('='));
    assert.deepEqual(b64uDecode(text), bytes, `length ${length}`);
  }
});

test('b64u encodes large inputs (chunked string building) correctly', () => {
  const bytes = new Uint8Array(nodeRandomBytes(49152));
  const text = b64uEncode(bytes);
  assert.equal(text.length, 65536);
  assert.equal(text, Buffer.from(bytes).toString('base64url'));
  assert.deepEqual(b64uDecode(text), bytes);
});

test('b64u uses the URL-safe alphabet', () => {
  assert.equal(b64uEncode(new Uint8Array([0xfb, 0xff, 0xbf])), '-_-_');
  assert.deepEqual(b64uDecode('-_-_'), new Uint8Array([0xfb, 0xff, 0xbf]));
});

test('b64u decode rejects padding, the standard alphabet, whitespace and foreign characters', () => {
  for (const bad of ['QQ==', 'QQ=', 'a+b/', 'a b', 'a\nb', 'é', 'AAAA\u0000', ' AAAA', 'AA AA']) {
    assert.throws(() => b64uDecode(bad), RemoteProtocolError, JSON.stringify(bad));
  }
});

test('b64u decode rejects lengths no byte string encodes to', () => {
  for (const length of [1, 5, 9, 21, 65]) {
    assert.throws(() => b64uDecode('A'.repeat(length)), /impossible length/, `length ${length}`);
  }
  assert.deepEqual(b64uDecode(''), new Uint8Array(0));
});

test('b64u decode rejects non-zero trailing bits, so every byte string has one spelling', () => {
  assert.deepEqual(b64uDecode('QQ'), new Uint8Array([0x41]));
  assert.throws(() => b64uDecode('QR'), /trailing bits/);
  assert.deepEqual(b64uDecode('QUI'), new Uint8Array([0x41, 0x42]));
  assert.throws(() => b64uDecode('QUJ'), /trailing bits/);
});

test('isB64u checks both the alphabet and an exact byte length', () => {
  assert.equal(isB64u('QUJD'), true);
  assert.equal(isB64u('QUJD', 3), true);
  assert.equal(isB64u('QUJD', 4), false);
  assert.equal(isB64u(b64uEncode(new Uint8Array(32)), 32), true);
  assert.equal(isB64u(b64uEncode(new Uint8Array(32)).slice(0, 42), 32), false);
  assert.equal(isB64u('QQ==', 1), false);
  assert.equal(isB64u(42 as unknown), false);
  assert.equal(isB64u(undefined), false);
});

test('UTF-8: lossless encode, strict decode throws, lenient decode substitutes, BOM is kept', () => {
  const text = '灵犀 ✓ 😀 é';
  assert.equal(utf8Decode(utf8Encode(text), true), text);
  assert.throws(() => utf8Decode(new Uint8Array([0xff]), true), RemoteProtocolError);
  assert.throws(() => utf8Decode(new Uint8Array([0xc3]), true), RemoteProtocolError);
  assert.equal(utf8Decode(new Uint8Array([0x61, 0xff]), false), 'a�');
  const withBom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61]);
  assert.equal(utf8Decode(withBom, true), '\uFEFFa');
});

test('sha256 matches node:crypto', async () => {
  for (const input of [new Uint8Array(0), utf8Encode('abc'), new Uint8Array(nodeRandomBytes(100_000))]) {
    assert.equal(hex(await sha256(input)), createHash('sha256').update(input).digest('hex'));
  }
});

test('randomBytes fills any length and randomId is 22 characters of 16 fresh bytes', () => {
  assert.equal(randomBytes(0).length, 0);
  assert.equal(randomBytes(32).length, 32);
  assert.equal(randomBytes(70_000).length, 70_000);
  assert.ok(randomBytes(70_000).subarray(65_536).some((byte) => byte !== 0), 'bytes past the 64 KiB WebCrypto limit are filled too');
  const ids = new Set(Array.from({ length: 50 }, () => randomId()));
  assert.equal(ids.size, 50);
  for (const id of ids) {
    assert.match(id, /^[A-Za-z0-9_-]{22}$/);
    assert.equal(b64uDecode(id).length, 16);
  }
});

test('concatBytes and bytesEqual', () => {
  assert.deepEqual(concatBytes(), new Uint8Array(0));
  assert.deepEqual(concatBytes(new Uint8Array([1]), new Uint8Array(0), new Uint8Array([2, 3])), new Uint8Array([1, 2, 3]));
  assert.equal(bytesEqual(new Uint8Array([1, 2]), new Uint8Array([1, 2])), true);
  assert.equal(bytesEqual(new Uint8Array([1, 2]), new Uint8Array([1, 3])), false);
  assert.equal(bytesEqual(new Uint8Array([1, 2]), new Uint8Array([1])), false);
});

test('protocol constants carry the plan\'s values', () => {
  assert.equal(version.REMOTE_PROTOCOL, 'lingxi-rc/1');
  assert.equal(version.REMOTE_PROTOCOL_MAJOR, 1);
  assert.equal(version.DATA_CHANNEL_LABEL, 'lingxi-agent');
  assert.equal(version.REMOTE_FEATURE_BOTS, 'bots');
  assert.equal(version.FRAGMENT_MAX_BYTES, 16384);
  assert.equal(version.MESSAGE_MAX_BYTES, 16 * 1024 * 1024);
  assert.equal(version.TUNNEL_FRAME_PLAINTEXT_MAX, 49136);
  assert.equal(version.SDP_MAX_CHARS, 16384);
  assert.equal(version.DEFAULT_RTDB_URL, 'https://lingxi-5e4b2-default-rtdb.firebaseio.com');
  assert.equal(version.RTDB_EMULATOR_NAMESPACE, 'lingxi-5e4b2-default-rtdb');
  assert.deepEqual([...version.DEFAULT_STUN_URLS], ['stun:stun.cloudflare.com:3478', 'stun:stun.l.google.com:19302']);
  assert.equal(version.HEARTBEAT_INTERVAL_MS, 30_000);
  assert.equal(version.ONLINE_WINDOW_MS, 90_000);
  assert.equal(version.OFFER_ANSWER_TIMEOUT_MS, 30_000);
  assert.equal(version.OFFER_MAX_AGE_MS, 300_000);
  assert.equal(version.ICE_GATHER_TIMEOUT_MS, 3_000);
  assert.equal(version.DATA_CHANNEL_OPEN_TIMEOUT_MS, 15_000);
  assert.equal(version.DIRECT_FAILURE_MEMORY_MS, 600_000);
  assert.equal(version.DATA_CHANNEL_PING_INTERVAL_MS, 15_000);
  assert.equal(version.DATA_CHANNEL_DEAD_MS, 45_000);
  assert.equal(version.TUNNEL_PING_INTERVAL_MS, 25_000);
  assert.equal(version.TUNNEL_DEAD_MS, 75_000);
  assert.equal(version.TUNNEL_FLUSH_DELAY_MS, 25);
  assert.equal(version.TUNNEL_MAX_INFLIGHT_WRITES, 4);
  assert.equal(version.TUNNEL_DELETE_INTERVAL_MS, 500);
  assert.equal(version.TUNNEL_DELETE_BATCH, 32);
  assert.equal(version.TUNNEL_GAP_TIMEOUT_MS, 30_000);
  assert.equal(version.MAX_HOST_CONNECTIONS, 8);
});

test('the client protocol major is read from the SDK\'s own version, not copied', () => {
  assert.equal(version.CLIENT_PROTOCOL_MAJOR, Number.parseInt(CLIENT_PROTOCOL_VERSION, 10));
  assert.equal(version.CLIENT_PROTOCOL_MAJOR, 30);
});

test('frame sizes are consistent: a full frame stores as exactly 65536 characters', () => {
  assert.equal(version.TUNNEL_FRAME_PLAINTEXT_MAX + 16, 3 * 16384);
  assert.equal(b64uEncode(new Uint8Array(version.TUNNEL_FRAME_PLAINTEXT_MAX + 16)).length, 65536);
  // Three tunnel-sized fragments, each with its 4-byte length prefix, fit one frame; three full ones do not.
  assert.ok(3 * (4 + version.TUNNEL_FRAGMENT_MAX_BYTES) <= version.TUNNEL_FRAME_PLAINTEXT_MAX);
  assert.ok(3 * (4 + version.FRAGMENT_MAX_BYTES) > version.TUNNEL_FRAME_PLAINTEXT_MAX);
  assert.ok(3 * (4 + version.TUNNEL_FRAGMENT_MAX_BYTES + 1) > version.TUNNEL_FRAME_PLAINTEXT_MAX, 'the tunnel size is the largest that fits');
});
