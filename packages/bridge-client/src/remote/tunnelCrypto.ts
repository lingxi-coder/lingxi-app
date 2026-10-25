/**
 * Cryptography of the RTDB tunnel: per-direction AES-256-GCM keys derived from
 * the X25519 shared secret, frame sealing, and the plaintext frame layout.
 *
 * A frame is stored in RTDB as `b64u(ciphertext ‖ tag)` under key `f{seq}`.
 * The nonce is four zero bytes followed by `seq` as a big-endian u64, so a
 * (key, nonce) pair never repeats as long as `seq` does not; the AAD binds the
 * connection, the direction and the sequence number, so a frame cannot be
 * replayed on another connection, in the other direction or at another position.
 */

import { asBufferSource, b64uDecode, b64uEncode, utf8Encode, type Bytes } from './encoding.js';
import { RemoteProtocolError } from './errors.js';
import { hkdfSha256, type WebCryptoKey } from './keys.js';
import { FRAGMENT_MAX_BYTES, TUNNEL_FRAME_PLAINTEXT_MAX } from './version.js';

const LABEL = 'lingxi-rc-tunnel/1';
const TAG_BYTES = 16;
const MAX_SEQ = 2 ** 53 - 1;
// 49152 sealed bytes encode to 65536 characters; the RTDB rules enforce the same bound.
const STORED_MAX_CHARS = 65536;

/** `up`: viewer to host. `down`: host to viewer. */
export type TunnelDir = 'up' | 'down';

/** The two traffic keys of one connection. */
export interface TunnelKeys {
  /** Seals viewer-to-host frames. */
  up: WebCryptoKey;
  /** Seals host-to-viewer frames. */
  down: WebCryptoKey;
}

/** A tunnel frame failed authentication or was not a well-formed sealed value. */
export class TunnelCryptoError extends RemoteProtocolError {
  constructor(message: string) {
    super(message);
    this.name = 'TunnelCryptoError';
  }
}

/**
 * `HKDF-SHA256(ikm = shared, salt = UTF-8(conn_id), info = "lingxi-rc-tunnel/1 up" | "… down")`,
 * 32 bytes each, as non-extractable AES-GCM keys.
 */
export async function deriveTunnelKeys(shared: Uint8Array, connId: string): Promise<TunnelKeys> {
  const salt = utf8Encode(connId);
  const derive = async (dir: TunnelDir): Promise<WebCryptoKey> => {
    const material = await hkdfSha256(shared, salt, utf8Encode(`${LABEL} ${dir}`), 32);
    return crypto.subtle.importKey('raw', asBufferSource(material), { name: 'AES-GCM' }, false, ['encrypt', 'decrypt']);
  };
  const [up, down] = await Promise.all([derive('up'), derive('down')]);
  return { up, down };
}

function checkSeq(seq: number): void {
  if (!Number.isSafeInteger(seq) || seq < 0 || seq > MAX_SEQ) throw new RemoteProtocolError('tunnel sequence out of range');
}

/** The 12-byte GCM nonce of frame `seq`: four zero bytes, then `seq` as a big-endian u64. */
export function tunnelNonce(seq: number): Bytes {
  checkSeq(seq);
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, BigInt(seq), false);
  return nonce;
}

/** The additional authenticated data of frame `seq`: `"lingxi-rc-tunnel/1\n{conn_id}\n{dir}\n{seq}"`. */
export function tunnelAad(connId: string, dir: TunnelDir, seq: number): Bytes {
  checkSeq(seq);
  return utf8Encode(`${LABEL}\n${connId}\n${dir}\n${seq}`);
}

/** Encrypt one frame's plaintext (at most {@link TUNNEL_FRAME_PLAINTEXT_MAX} bytes) into its stored value. */
export async function sealFrame(
  key: WebCryptoKey,
  connId: string,
  dir: TunnelDir,
  seq: number,
  plaintext: Uint8Array,
): Promise<string> {
  if (plaintext.length > TUNNEL_FRAME_PLAINTEXT_MAX) throw new RemoteProtocolError('tunnel frame plaintext is too large');
  const sealed = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: tunnelNonce(seq), additionalData: tunnelAad(connId, dir, seq), tagLength: TAG_BYTES * 8 },
    key,
    asBufferSource(plaintext),
  );
  return b64uEncode(new Uint8Array(sealed));
}

/**
 * Decrypt a stored frame value. Throws {@link TunnelCryptoError} when the value
 * is malformed, or when it does not authenticate under this key, connection,
 * direction and sequence number.
 */
export async function openFrame(
  key: WebCryptoKey,
  connId: string,
  dir: TunnelDir,
  seq: number,
  stored: string,
): Promise<Bytes> {
  if (stored.length > STORED_MAX_CHARS) throw new TunnelCryptoError('tunnel frame is too long');
  let sealed: Bytes;
  try {
    sealed = b64uDecode(stored);
  } catch {
    throw new TunnelCryptoError('tunnel frame is not base64url');
  }
  if (sealed.length < TAG_BYTES || sealed.length > TUNNEL_FRAME_PLAINTEXT_MAX + TAG_BYTES) {
    throw new TunnelCryptoError('tunnel frame has an impossible size');
  }
  try {
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: tunnelNonce(seq), additionalData: tunnelAad(connId, dir, seq), tagLength: TAG_BYTES * 8 },
      key,
      asBufferSource(sealed),
    );
    return new Uint8Array(plaintext);
  } catch {
    throw new TunnelCryptoError('tunnel frame failed authentication');
  }
}

/**
 * Lay fragments out as a frame plaintext: repeated `[u32 BE length][fragment]`.
 * The total may not exceed {@link TUNNEL_FRAME_PLAINTEXT_MAX}.
 */
export function packFragments(fragments: readonly Uint8Array[]): Bytes {
  let total = 0;
  for (const fragment of fragments) total += 4 + fragment.length;
  if (total > TUNNEL_FRAME_PLAINTEXT_MAX) throw new RemoteProtocolError('tunnel frame plaintext is too large');
  const out = new Uint8Array(total);
  const view = new DataView(out.buffer);
  let offset = 0;
  for (const fragment of fragments) {
    view.setUint32(offset, fragment.length, false);
    out.set(fragment, offset + 4);
    offset += 4 + fragment.length;
  }
  return out;
}

/**
 * Inverse of {@link packFragments}. Rejects an oversized plaintext, a truncated
 * length or body, and fragments that are empty or longer than
 * {@link FRAGMENT_MAX_BYTES}.
 */
export function unpackFragments(plaintext: Uint8Array): Bytes[] {
  if (plaintext.length > TUNNEL_FRAME_PLAINTEXT_MAX) throw new RemoteProtocolError('tunnel frame plaintext is too large');
  const view = new DataView(plaintext.buffer, plaintext.byteOffset, plaintext.byteLength);
  const fragments: Bytes[] = [];
  let offset = 0;
  while (offset < plaintext.length) {
    if (plaintext.length - offset < 4) throw new RemoteProtocolError('tunnel frame is truncated');
    const length = view.getUint32(offset, false);
    offset += 4;
    if (length === 0 || length > FRAGMENT_MAX_BYTES) throw new RemoteProtocolError('tunnel frame holds a fragment of impossible size');
    if (plaintext.length - offset < length) throw new RemoteProtocolError('tunnel frame is truncated');
    fragments.push(plaintext.slice(offset, offset + length) as Bytes);
    offset += length;
  }
  return fragments;
}
