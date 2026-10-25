/**
 * Long-term Ed25519 identity keys, ephemeral X25519 keys and HKDF, all on
 * WebCrypto (`crypto.subtle`), which Node 22+, Electron 43 and current browsers
 * implement identically for these algorithms.
 *
 * Keys cross the API as raw 32-byte public keys plus a `CryptoKey` private
 * half; an identity key is optionally exportable as PKCS#8 for storage
 * (the web viewer keeps a non-extractable key instead).
 */

import { RemoteProtocolError } from './errors.js';
import {
  asBufferSource,
  b64uDecode,
  b64uEncode,
  concatBytes,
  sha256,
  type Bytes,
} from './encoding.js';

/**
 * A WebCrypto key handle. Derived from `crypto.subtle` rather than the global
 * `CryptoKey` so the same declaration works with DOM typings and Node's.
 */
export type WebCryptoKey = Awaited<ReturnType<typeof crypto.subtle.importKey>>;

/** PKCS#8 header of an Ed25519 private key; the 32-byte seed follows. */
export const ED25519_PKCS8_PREFIX = Uint8Array.from([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
]);

/** PKCS#8 header of an X25519 private key; the 32-byte seed follows. */
export const X25519_PKCS8_PREFIX = Uint8Array.from([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20,
]);

const KEY_BYTES = 32;
const SIGNATURE_BYTES = 64;

/** A long-term signing key: raw public half, private `CryptoKey`, optional PKCS#8. */
export interface IdentityKey {
  /** 32-byte raw Ed25519 public key. */
  publicKey: Bytes;
  privateKey: WebCryptoKey;
  /** PKCS#8 encoding of the private key; present only when the key is extractable. */
  pkcs8?: Bytes;
}

/** A single-connection key-agreement key. The private half is never extractable. */
export interface EphemeralKey {
  /** 32-byte raw X25519 public key. */
  publicKey: Bytes;
  privateKey: WebCryptoKey;
}

/** Whether the private half of a generated or imported key may be exported again. */
export interface KeyOptions {
  extractable: boolean;
}

const subtle = (): typeof crypto.subtle => crypto.subtle;

function requireLength(bytes: Uint8Array, length: number, what: string): void {
  if (bytes.length !== length) {
    throw new RemoteProtocolError(`${what} must be ${length} bytes, got ${bytes.length}`);
  }
}

/** Public key bytes of an extractable OKP private key, read from its JWK `x`. */
async function publicKeyOf(extractablePrivate: WebCryptoKey): Promise<Bytes> {
  const jwk = await subtle().exportKey('jwk', extractablePrivate);
  if (typeof jwk.x !== 'string') throw new RemoteProtocolError('key export has no public part');
  const publicKey = b64uDecode(jwk.x);
  requireLength(publicKey, KEY_BYTES, 'public key');
  return publicKey;
}

// ─────────────────────────────────────────────────────────────────────────────
// Ed25519 identity keys
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Generate a fresh identity key. With `extractable: false` the private key
 * never exists as JS bytes (the web viewer stores such a key in IndexedDB).
 */
export async function generateIdentityKey(options: KeyOptions): Promise<IdentityKey> {
  const pair = (await subtle().generateKey({ name: 'Ed25519' }, options.extractable, ['sign', 'verify'])) as unknown as {
    publicKey: WebCryptoKey;
    privateKey: WebCryptoKey;
  };
  const publicKey = new Uint8Array(await subtle().exportKey('raw', pair.publicKey));
  requireLength(publicKey, KEY_BYTES, 'public key');
  const key: IdentityKey = { publicKey, privateKey: pair.privateKey };
  if (options.extractable) key.pkcs8 = new Uint8Array(await subtle().exportKey('pkcs8', pair.privateKey));
  return key;
}

/**
 * Import a stored PKCS#8 identity key. The public half is read from a JWK
 * export of an extractable import; when `extractable: false` was requested the
 * key is imported a second time as non-extractable, so the returned handle
 * cannot be exported again.
 */
export async function importIdentityKey(pkcs8: Uint8Array, options: KeyOptions): Promise<IdentityKey> {
  const algorithm = { name: 'Ed25519' };
  const probe = await subtle().importKey('pkcs8', asBufferSource(pkcs8), algorithm, true, ['sign']);
  const publicKey = await publicKeyOf(probe);
  if (options.extractable) {
    return { publicKey, privateKey: probe, pkcs8: new Uint8Array(await subtle().exportKey('pkcs8', probe)) };
  }
  const privateKey = await subtle().importKey('pkcs8', asBufferSource(pkcs8), algorithm, false, ['sign']);
  return { publicKey, privateKey };
}

/** Deterministic, extractable identity key from a 32-byte seed (fixtures, tests). */
export async function identityKeyFromSeed(seed: Uint8Array): Promise<IdentityKey> {
  requireLength(seed, KEY_BYTES, 'seed');
  return importIdentityKey(concatBytes(ED25519_PKCS8_PREFIX, seed), { extractable: true });
}

/** Ed25519 signature (64 bytes) of `data`. */
export async function signBytes(privateKey: WebCryptoKey, data: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await subtle().sign({ name: 'Ed25519' }, privateKey, asBufferSource(data)));
}

/**
 * Check an Ed25519 signature. Never throws: a wrong-sized key or signature,
 * an invalid point or any WebCrypto failure all answer `false`.
 */
export async function verifyBytes(publicKeyRaw: Uint8Array, signature: Uint8Array, data: Uint8Array): Promise<boolean> {
  if (publicKeyRaw.length !== KEY_BYTES || signature.length !== SIGNATURE_BYTES) return false;
  try {
    const key = await subtle().importKey('raw', asBufferSource(publicKeyRaw), { name: 'Ed25519' }, false, ['verify']);
    return await subtle().verify({ name: 'Ed25519' }, key, asBufferSource(signature), asBufferSource(data));
  } catch {
    return false;
  }
}

/** Self-certifying id of a public key: the first 22 characters of b64u(SHA-256(raw key)). */
export async function idFromKey(publicKeyRaw: Uint8Array): Promise<string> {
  return b64uEncode(await sha256(publicKeyRaw)).slice(0, 22);
}

// ─────────────────────────────────────────────────────────────────────────────
// X25519 ephemeral keys
// ─────────────────────────────────────────────────────────────────────────────

/** Generate a fresh X25519 key for one connection attempt. */
export async function generateEphemeralKey(): Promise<EphemeralKey> {
  const pair = (await subtle().generateKey({ name: 'X25519' }, false, ['deriveBits'])) as unknown as {
    publicKey: WebCryptoKey;
    privateKey: WebCryptoKey;
  };
  const publicKey = new Uint8Array(await subtle().exportKey('raw', pair.publicKey));
  requireLength(publicKey, KEY_BYTES, 'public key');
  return { publicKey, privateKey: pair.privateKey };
}

/** Deterministic X25519 key from a 32-byte seed (fixtures, tests). */
export async function ephemeralKeyFromSeed(seed: Uint8Array): Promise<EphemeralKey> {
  requireLength(seed, KEY_BYTES, 'seed');
  const pkcs8 = asBufferSource(concatBytes(X25519_PKCS8_PREFIX, seed));
  const algorithm = { name: 'X25519' };
  const probe = await subtle().importKey('pkcs8', pkcs8, algorithm, true, ['deriveBits']);
  const publicKey = await publicKeyOf(probe);
  const privateKey = await subtle().importKey('pkcs8', pkcs8, algorithm, false, ['deriveBits']);
  return { publicKey, privateKey };
}

/**
 * X25519 shared secret (32 bytes). A peer key that yields the all-zero secret
 * (a low-order point) is rejected with {@link RemoteProtocolError}.
 */
export async function x25519(privateKey: WebCryptoKey, peerPublicRaw: Uint8Array): Promise<Bytes> {
  requireLength(peerPublicRaw, KEY_BYTES, 'peer public key');
  let shared: Bytes;
  try {
    const peer = await subtle().importKey('raw', asBufferSource(peerPublicRaw), { name: 'X25519' }, false, []);
    shared = new Uint8Array(await subtle().deriveBits({ name: 'X25519', public: peer }, privateKey, 256));
  } catch {
    throw new RemoteProtocolError('X25519 key agreement failed');
  }
  let any = 0;
  for (const byte of shared) any |= byte;
  if (any === 0) throw new RemoteProtocolError('X25519 shared secret is all zero');
  return shared;
}

// ─────────────────────────────────────────────────────────────────────────────
// HKDF
// ─────────────────────────────────────────────────────────────────────────────

/** HKDF-SHA256 (RFC 5869) producing `length` bytes. */
export async function hkdfSha256(
  ikm: Uint8Array,
  salt: Uint8Array,
  info: Uint8Array,
  length: number,
): Promise<Bytes> {
  const base = await subtle().importKey('raw', asBufferSource(ikm), 'HKDF', false, ['deriveBits']);
  const bits = await subtle().deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: asBufferSource(salt), info: asBufferSource(info) },
    base,
    length * 8,
  );
  return new Uint8Array(bits);
}
