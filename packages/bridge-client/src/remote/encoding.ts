/**
 * Byte and text helpers for the remote-control protocol. Pure WebCrypto /
 * TextEncoder code, so it runs unchanged in Node, Electron and browsers.
 */

import { RemoteProtocolError } from './errors.js';

/** A `Uint8Array` known to sit on a plain `ArrayBuffer`, as WebCrypto requires. */
export type Bytes = Uint8Array<ArrayBuffer>;

/** View `bytes` as WebCrypto input (no copy). Everything created here is on a plain buffer. */
export function asBufferSource(bytes: Uint8Array): Bytes {
  return bytes as Bytes;
}

// ─────────────────────────────────────────────────────────────────────────────
// base64url (RFC 4648 §5, no padding)
// ─────────────────────────────────────────────────────────────────────────────

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
const ALPHABET_CODES = Uint8Array.from(ALPHABET, (c) => c.charCodeAt(0));
const DECODE_TABLE = (() => {
  const table = new Int8Array(128).fill(-1);
  for (let i = 0; i < ALPHABET.length; i++) table[ALPHABET.charCodeAt(i)] = i;
  return table;
})();

const CHUNK = 8192;

/** Encode bytes as unpadded base64url (the protocol's `b64u`). */
export function b64uEncode(bytes: Uint8Array): string {
  const length = bytes.length;
  const outLength = Math.floor(length / 3) * 4 + (length % 3 === 0 ? 0 : (length % 3) + 1);
  const codes = new Uint8Array(outLength);
  let o = 0;
  let i = 0;
  for (; i + 2 < length; i += 3) {
    const v = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
    codes[o++] = ALPHABET_CODES[(v >>> 18) & 63]!;
    codes[o++] = ALPHABET_CODES[(v >>> 12) & 63]!;
    codes[o++] = ALPHABET_CODES[(v >>> 6) & 63]!;
    codes[o++] = ALPHABET_CODES[v & 63]!;
  }
  if (length - i === 1) {
    const v = bytes[i]! << 16;
    codes[o++] = ALPHABET_CODES[(v >>> 18) & 63]!;
    codes[o++] = ALPHABET_CODES[(v >>> 12) & 63]!;
  } else if (length - i === 2) {
    const v = (bytes[i]! << 16) | (bytes[i + 1]! << 8);
    codes[o++] = ALPHABET_CODES[(v >>> 18) & 63]!;
    codes[o++] = ALPHABET_CODES[(v >>> 12) & 63]!;
    codes[o++] = ALPHABET_CODES[(v >>> 6) & 63]!;
  }
  let out = '';
  for (let p = 0; p < codes.length; p += CHUNK) {
    out += String.fromCharCode(...codes.subarray(p, p + CHUNK));
  }
  return out;
}

/**
 * Decode unpadded base64url. Strict: rejects characters outside the alphabet
 * (including `=` padding, `+`, `/` and whitespace), lengths no byte string can
 * encode to, and non-zero trailing bits.
 */
export function b64uDecode(text: string): Bytes {
  const length = text.length;
  if (length % 4 === 1) throw new RemoteProtocolError('base64url: impossible length');
  const bytes = new Uint8Array(Math.floor((length * 3) / 4));
  let o = 0;
  let acc = 0;
  let bits = 0;
  for (let i = 0; i < length; i++) {
    const code = text.charCodeAt(i);
    const value = code < 128 ? DECODE_TABLE[code]! : -1;
    if (value < 0) throw new RemoteProtocolError('base64url: invalid character');
    acc = (acc << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes[o++] = (acc >>> bits) & 0xff;
      acc &= (1 << bits) - 1;
    }
  }
  if (acc !== 0) throw new RemoteProtocolError('base64url: non-zero trailing bits');
  return bytes;
}

/** `true` when `text` is valid unpadded base64url, optionally of exactly `byteLength` bytes. */
export function isB64u(text: unknown, byteLength?: number): text is string {
  if (typeof text !== 'string') return false;
  if (byteLength !== undefined && text.length !== b64uLength(byteLength)) return false;
  try {
    b64uDecode(text);
    return true;
  } catch {
    return false;
  }
}

/** Length of the unpadded base64url encoding of `byteLength` bytes. */
export function b64uLength(byteLength: number): number {
  return Math.floor(byteLength / 3) * 4 + (byteLength % 3 === 0 ? 0 : (byteLength % 3) + 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// UTF-8
// ─────────────────────────────────────────────────────────────────────────────

const encoder = new TextEncoder();
const strictDecoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const lenientDecoder = new TextDecoder('utf-8', { fatal: false, ignoreBOM: true });

/** UTF-8 bytes of `text`. */
export function utf8Encode(text: string): Bytes {
  return encoder.encode(text) as Bytes;
}

/**
 * Decode UTF-8. With `strict` an invalid sequence throws {@link RemoteProtocolError};
 * otherwise it becomes U+FFFD. A leading BOM is kept, so decoding is lossless.
 */
export function utf8Decode(bytes: Uint8Array, strict = false): string {
  try {
    return (strict ? strictDecoder : lenientDecoder).decode(bytes);
  } catch {
    throw new RemoteProtocolError('invalid UTF-8');
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing and randomness
// ─────────────────────────────────────────────────────────────────────────────

/** SHA-256 digest (32 bytes). */
export async function sha256(data: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', asBufferSource(data)));
}

/** `byteLength` cryptographically random bytes. */
export function randomBytes(byteLength: number): Bytes {
  const out = new Uint8Array(byteLength);
  // getRandomValues refuses requests above 65536 bytes.
  for (let p = 0; p < byteLength; p += 65536) {
    crypto.getRandomValues(out.subarray(p, Math.min(p + 65536, byteLength)));
  }
  return out;
}

/** A fresh `conn_id`-style identifier: 16 random bytes as 22 base64url characters. */
export function randomId(): string {
  return b64uEncode(randomBytes(16));
}

// ─────────────────────────────────────────────────────────────────────────────
// Byte strings
// ─────────────────────────────────────────────────────────────────────────────

/** Concatenate byte strings into a new array. */
export function concatBytes(...parts: readonly Uint8Array[]): Bytes {
  let total = 0;
  for (const part of parts) total += part.length;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** Content equality of two byte strings (constant time for equal lengths). */
export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}
