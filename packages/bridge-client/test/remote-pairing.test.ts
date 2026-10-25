/**
 * Pairing codes: the shared fixture vectors, an independent recomputation, and
 * the guarantees that make the code worth comparing by eye.
 */

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { b64uDecode } from '../src/remote/encoding.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import { pairingCode, pairingCodeFor } from '../src/remote/pairing.js';
import { fixture, unhex } from './remote-helpers.js';

interface Vector {
  name: string;
  host_key_hex: string;
  device_key_hex: string;
  offer_nonce_hex: string;
  answer_nonce_hex: string;
  sha256_hex: string;
  u32_be: number;
  code: string;
}

const { vectors } = fixture<{ vectors: Vector[] }>('pairing');

test('pairingCode reproduces every fixture vector', async () => {
  assert.ok(vectors.length >= 5);
  for (const v of vectors) {
    const code = await pairingCode(unhex(v.host_key_hex), unhex(v.device_key_hex), unhex(v.offer_nonce_hex), unhex(v.answer_nonce_hex));
    assert.equal(code, v.code, v.name);
  }
});

test('the fixtures include codes with leading zeros, which must stay six digits', async () => {
  const codes = vectors.map((v) => v.code);
  assert.ok(codes.some((c) => /^0[1-9]/.test(c)), 'one leading zero');
  assert.ok(codes.some((c) => /^000[1-9]/.test(c)), 'three leading zeros');
  assert.ok(codes.includes('000000'), 'all zeros');
  for (const c of codes) assert.match(c, /^[0-9]{6}$/);
});

test('the code is SHA-256 over the domain and raw bytes, first four bytes big-endian, mod 10^6 (recomputed independently)', async () => {
  for (const v of vectors) {
    const digest = createHash('sha256')
      .update('lingxi-rc-pair/1')
      .update(unhex(v.host_key_hex))
      .update(unhex(v.device_key_hex))
      .update(unhex(v.offer_nonce_hex))
      .update(unhex(v.answer_nonce_hex))
      .digest();
    assert.equal(digest.toString('hex'), v.sha256_hex, v.name);
    assert.equal(digest.readUInt32BE(0), v.u32_be);
    assert.equal(String(v.u32_be % 1_000_000).padStart(6, '0'), v.code);
  }
});

test('every input changes the code, and the argument order matters', async () => {
  const [v] = vectors;
  const host = unhex(v!.host_key_hex);
  const device = unhex(v!.device_key_hex);
  const offerNonce = unhex(v!.offer_nonce_hex);
  const answerNonce = unhex(v!.answer_nonce_hex);
  const base = await pairingCode(host, device, offerNonce, answerNonce);
  const flip = (bytes: Uint8Array): Uint8Array => {
    const copy = bytes.slice();
    copy[0] ^= 0x80;
    return copy;
  };
  const variants = await Promise.all([
    pairingCode(flip(host), device, offerNonce, answerNonce),
    pairingCode(host, flip(device), offerNonce, answerNonce),
    pairingCode(host, device, flip(offerNonce), answerNonce),
    pairingCode(host, device, offerNonce, flip(answerNonce)),
    pairingCode(device, host, offerNonce, answerNonce),
    pairingCode(host, device, answerNonce, offerNonce),
  ]);
  for (const [index, code] of variants.entries()) assert.notEqual(code, base, `variant ${index}`);
});

test('inputs of the wrong size are refused instead of silently hashed', async () => {
  const k = new Uint8Array(32);
  const n = new Uint8Array(16);
  await assert.rejects(pairingCode(new Uint8Array(31), k, n, n), RemoteProtocolError);
  await assert.rejects(pairingCode(k, new Uint8Array(33), n, n), RemoteProtocolError);
  await assert.rejects(pairingCode(k, k, new Uint8Array(15), n), RemoteProtocolError);
  await assert.rejects(pairingCode(k, k, n, new Uint8Array(17)), RemoteProtocolError);
  assert.match(await pairingCode(k, k, n, n), /^[0-9]{6}$/);
});

test('pairingCodeFor reads the same inputs from an offer and its answer', async () => {
  const signing = fixture('signing');
  const offer = signing.offers[0].record;
  const answer = signing.answers[0].record;
  const expected = await pairingCode(
    b64uDecode(answer.host.key),
    b64uDecode(offer.device.key),
    b64uDecode(offer.nonce),
    b64uDecode(answer.nonce),
  );
  assert.equal(await pairingCodeFor(offer, answer), expected);
  assert.match(expected, /^[0-9]{6}$/);

  // Both ends see the same records, so they show the same code.
  const other = signing.answers[1].record;
  assert.notEqual(await pairingCodeFor(offer, other), expected);
});
