/**
 * Identity keys, X25519 and HKDF, checked against the RFC vectors and against
 * node:crypto (an independent implementation of the same primitives).
 */

import assert from 'node:assert/strict';
import { createCipheriv, createPrivateKey, createPublicKey, diffieHellman, hkdfSync, randomBytes as nodeRandomBytes, sign as nodeSign, verify as nodeVerify } from 'node:crypto';
import { test } from 'node:test';

import { b64uEncode } from '../src/remote/encoding.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import {
  ED25519_PKCS8_PREFIX,
  ephemeralKeyFromSeed,
  generateEphemeralKey,
  generateIdentityKey,
  hkdfSha256,
  idFromKey,
  identityKeyFromSeed,
  importIdentityKey,
  signBytes,
  verifyBytes,
  X25519_PKCS8_PREFIX,
  x25519,
} from '../src/remote/keys.js';
import { deriveTunnelKeys, sealFrame } from '../src/remote/tunnelCrypto.js';
import { hex, unhex } from './remote-helpers.js';

// ── Ed25519 ──────────────────────────────────────────────────────────────────

const RFC8032 = {
  seed: '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
  publicKey: 'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
  signatureOfEmpty:
    'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
};

test('Ed25519 reproduces RFC 8032 test vector 1 (seed, public key, empty-message signature)', async () => {
  const key = await identityKeyFromSeed(unhex(RFC8032.seed));
  assert.equal(hex(key.publicKey), RFC8032.publicKey);
  assert.equal(hex(key.pkcs8!), `${hex(ED25519_PKCS8_PREFIX)}${RFC8032.seed}`);
  const signature = await signBytes(key.privateKey, new Uint8Array(0));
  assert.equal(hex(signature), RFC8032.signatureOfEmpty);
  assert.equal(await verifyBytes(key.publicKey, signature, new Uint8Array(0)), true);
});

test('Ed25519 agrees with node:crypto in both directions on assorted messages', async () => {
  const seed = new Uint8Array(nodeRandomBytes(32));
  const key = await identityKeyFromSeed(seed);
  const nodeKey = createPrivateKey({ key: Buffer.concat([ED25519_PKCS8_PREFIX, seed]), format: 'der', type: 'pkcs8' });
  for (const length of [0, 1, 31, 32, 33, 1000]) {
    const message = new Uint8Array(nodeRandomBytes(length));
    const ours = await signBytes(key.privateKey, message);
    assert.deepEqual(Buffer.from(ours), nodeSign(null, message, nodeKey), `signature of ${length} bytes`);
    assert.equal(nodeVerify(null, message, createPublicKey(nodeKey), ours), true);
    assert.equal(await verifyBytes(key.publicKey, new Uint8Array(nodeSign(null, message, nodeKey)), message), true);
  }
});

test('verifyBytes is false, never a throw, for a bad signature, wrong message, wrong key and malformed input', async () => {
  const key = await identityKeyFromSeed(unhex(RFC8032.seed));
  const other = await generateIdentityKey({ extractable: false });
  const message = new Uint8Array([1, 2, 3]);
  const signature = await signBytes(key.privateKey, message);
  assert.equal(await verifyBytes(key.publicKey, signature, message), true);
  assert.equal(await verifyBytes(key.publicKey, signature, new Uint8Array([1, 2, 4])), false);
  assert.equal(await verifyBytes(other.publicKey, signature, message), false);
  const flipped = signature.slice();
  flipped[10] ^= 1;
  assert.equal(await verifyBytes(key.publicKey, flipped, message), false);
  assert.equal(await verifyBytes(key.publicKey, signature.subarray(0, 63), message), false);
  assert.equal(await verifyBytes(key.publicKey, new Uint8Array(0), message), false);
  assert.equal(await verifyBytes(key.publicKey.subarray(0, 31), signature, message), false);
  assert.equal(await verifyBytes(new Uint8Array(32).fill(0xff), signature, message), false, 'not a valid curve point');
  assert.equal(await verifyBytes(new Uint8Array(0), new Uint8Array(0), new Uint8Array(0)), false);
});

test('idFromKey is the first 22 characters of b64u(SHA-256(raw key))', async () => {
  const key = await identityKeyFromSeed(unhex(RFC8032.seed));
  const id = await idFromKey(key.publicKey);
  assert.equal(id.length, 22);
  assert.match(id, /^[A-Za-z0-9_-]{22}$/);
  const { createHash } = await import('node:crypto');
  assert.equal(id, createHash('sha256').update(key.publicKey).digest('base64url').slice(0, 22));
  assert.equal(id, 'If4x36FUomFia_hUBG_SJx');
  assert.notEqual(await idFromKey(new Uint8Array(32)), id);
});

test('generated identity keys: extractable ones expose PKCS#8, non-extractable ones never do', async () => {
  const exportable = await generateIdentityKey({ extractable: true });
  assert.equal(exportable.publicKey.length, 32);
  assert.equal(exportable.pkcs8!.length, 48);
  assert.equal(hex(exportable.pkcs8!.subarray(0, 16)), hex(ED25519_PKCS8_PREFIX));
  assert.equal(exportable.privateKey.extractable, true);

  const sealed = await generateIdentityKey({ extractable: false });
  assert.equal(sealed.publicKey.length, 32);
  assert.equal(sealed.pkcs8, undefined);
  assert.equal(sealed.privateKey.extractable, false);
  await assert.rejects(crypto.subtle.exportKey('pkcs8', sealed.privateKey));

  const signature = await signBytes(sealed.privateKey, new Uint8Array([9]));
  assert.equal(await verifyBytes(sealed.publicKey, signature, new Uint8Array([9])), true);
});

test('importIdentityKey round-trips the PKCS#8 and honours the extractable choice', async () => {
  const original = await generateIdentityKey({ extractable: true });
  const message = new Uint8Array([4, 5, 6]);

  const again = await importIdentityKey(original.pkcs8!, { extractable: true });
  assert.deepEqual(again.publicKey, original.publicKey);
  assert.deepEqual(again.pkcs8, original.pkcs8);
  assert.equal(await verifyBytes(original.publicKey, await signBytes(again.privateKey, message), message), true);

  const locked = await importIdentityKey(original.pkcs8!, { extractable: false });
  assert.deepEqual(locked.publicKey, original.publicKey);
  assert.equal(locked.pkcs8, undefined);
  assert.equal(locked.privateKey.extractable, false);
  await assert.rejects(crypto.subtle.exportKey('pkcs8', locked.privateKey));
  assert.equal(await verifyBytes(original.publicKey, await signBytes(locked.privateKey, message), message), true);

  await assert.rejects(importIdentityKey(new Uint8Array(10), { extractable: true }));
  await assert.rejects(identityKeyFromSeed(new Uint8Array(31)), RemoteProtocolError);
});

// ── X25519 ───────────────────────────────────────────────────────────────────

const RFC7748 = {
  alicePrivate: '77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a',
  alicePublic: '8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a',
  bobPrivate: '5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb',
  bobPublic: 'de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f',
  shared: '4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742',
};

test('X25519 reproduces RFC 7748 section 6.1 (public keys and the shared secret, both ways)', async () => {
  const alice = await ephemeralKeyFromSeed(unhex(RFC7748.alicePrivate));
  const bob = await ephemeralKeyFromSeed(unhex(RFC7748.bobPrivate));
  assert.equal(hex(alice.publicKey), RFC7748.alicePublic);
  assert.equal(hex(bob.publicKey), RFC7748.bobPublic);
  assert.equal(hex(await x25519(alice.privateKey, bob.publicKey)), RFC7748.shared);
  assert.equal(hex(await x25519(bob.privateKey, alice.publicKey)), RFC7748.shared);
  assert.equal(alice.privateKey.extractable, false);
});

test('X25519 agrees with node:crypto, and two generated ephemeral keys agree with each other', async () => {
  const a = await generateEphemeralKey();
  const b = await generateEphemeralKey();
  assert.equal(a.publicKey.length, 32);
  assert.equal(a.privateKey.extractable, false);
  const ab = await x25519(a.privateKey, b.publicKey);
  assert.deepEqual(ab, await x25519(b.privateKey, a.publicKey));
  assert.notDeepEqual(a.publicKey, b.publicKey);

  const seedA = new Uint8Array(nodeRandomBytes(32));
  const seedB = new Uint8Array(nodeRandomBytes(32));
  const ours = await x25519((await ephemeralKeyFromSeed(seedA)).privateKey, (await ephemeralKeyFromSeed(seedB)).publicKey);
  const nodeA = createPrivateKey({ key: Buffer.concat([X25519_PKCS8_PREFIX, seedA]), format: 'der', type: 'pkcs8' });
  const nodeBPublic = createPublicKey(createPrivateKey({ key: Buffer.concat([X25519_PKCS8_PREFIX, seedB]), format: 'der', type: 'pkcs8' }));
  assert.deepEqual(Buffer.from(ours), diffieHellman({ privateKey: nodeA, publicKey: nodeBPublic }));
});

test('x25519 rejects low-order peer keys (all-zero secret) and keys of the wrong size', async () => {
  const alice = await ephemeralKeyFromSeed(unhex(RFC7748.alicePrivate));
  await assert.rejects(x25519(alice.privateKey, new Uint8Array(32)), RemoteProtocolError);
  const one = new Uint8Array(32);
  one[0] = 1; // the identity element's neighbour: another small-order point
  await assert.rejects(x25519(alice.privateKey, one), RemoteProtocolError);
  await assert.rejects(x25519(alice.privateKey, new Uint8Array(31)), RemoteProtocolError);
  await assert.rejects(x25519(alice.privateKey, new Uint8Array(33)), RemoteProtocolError);
  await assert.rejects(ephemeralKeyFromSeed(new Uint8Array(5)), RemoteProtocolError);
});

// ── HKDF and AES-GCM ─────────────────────────────────────────────────────────

test('HKDF-SHA256 reproduces RFC 5869 test case 1 and agrees with node:crypto on random inputs', async () => {
  const okm = await hkdfSha256(
    unhex('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b'),
    unhex('000102030405060708090a0b0c'),
    unhex('f0f1f2f3f4f5f6f7f8f9'),
    42,
  );
  assert.equal(hex(okm), '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865');

  for (const [ikm, salt, info, length] of [[32, 22, 21, 32], [32, 0, 0, 32], [16, 5, 40, 64], [64, 100, 1, 1]] as const) {
    const i = new Uint8Array(nodeRandomBytes(ikm));
    const s = new Uint8Array(nodeRandomBytes(salt));
    const n = new Uint8Array(nodeRandomBytes(info));
    assert.deepEqual(Buffer.from(await hkdfSha256(i, s, n, length)), Buffer.from(hkdfSync('sha256', i, s, n, length)), `${ikm}/${salt}/${info}/${length}`);
  }
});

test('tunnel keys: HKDF labels, salt and AES-256-GCM sealing agree with node:crypto', async () => {
  const shared = unhex(RFC7748.shared);
  const connId = 'AAAAAAAAAAAAAAAAAAAAAA';
  const keys = await deriveTunnelKeys(shared, connId);
  assert.equal(keys.up.extractable, false);
  assert.deepEqual(keys.up.algorithm, { name: 'AES-GCM', length: 256 });
  assert.notEqual(keys.up, keys.down);

  for (const dir of ['up', 'down'] as const) {
    const raw = hkdfSync('sha256', shared, Buffer.from(connId), Buffer.from(`lingxi-rc-tunnel/1 ${dir}`), 32);
    const plaintext = new Uint8Array(nodeRandomBytes(100));
    const seq = 7;
    const nonce = Buffer.alloc(12);
    nonce.writeBigUInt64BE(BigInt(seq), 4);
    const cipher = createCipheriv('aes-256-gcm', Buffer.from(raw), nonce, { authTagLength: 16 });
    cipher.setAAD(Buffer.from(`lingxi-rc-tunnel/1\n${connId}\n${dir}\n${seq}`));
    const expected = Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]);
    const stored = await sealFrame(keys[dir], connId, dir, seq, plaintext);
    assert.equal(stored, b64uEncode(expected), `${dir} frame`);
  }
});
