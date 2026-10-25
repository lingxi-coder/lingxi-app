/**
 * The 6-digit pairing code both sides compute independently and the user
 * compares: the desktop shows it in the approval dialog, the viewer shows it
 * next to "waiting for approval".
 */

import { b64uDecode, concatBytes, sha256, utf8Encode } from './encoding.js';
import { RemoteProtocolError } from './errors.js';
import type { AnswerRecord, OfferRecord } from './signaling.js';

const DOMAIN = utf8Encode('lingxi-rc-pair/1');

/**
 * `SHA-256("lingxi-rc-pair/1" ‖ host key ‖ device key ‖ offer nonce ‖ answer nonce)`,
 * first 4 bytes read as a big-endian u32, modulo 1 000 000, zero-padded to six
 * digits. Every input is raw bytes (32, 32, 16 and 16 long).
 */
export async function pairingCode(
  hostKeyRaw: Uint8Array,
  deviceKeyRaw: Uint8Array,
  offerNonceRaw: Uint8Array,
  answerNonceRaw: Uint8Array,
): Promise<string> {
  if (hostKeyRaw.length !== 32 || deviceKeyRaw.length !== 32 || offerNonceRaw.length !== 16 || answerNonceRaw.length !== 16) {
    throw new RemoteProtocolError('pairing code inputs have the wrong length');
  }
  const digest = await sha256(concatBytes(DOMAIN, hostKeyRaw, deviceKeyRaw, offerNonceRaw, answerNonceRaw));
  const value = new DataView(digest.buffer, digest.byteOffset, 4).getUint32(0, false);
  return String(value % 1_000_000).padStart(6, '0');
}

/** {@link pairingCode} from the two records that carry its inputs as base64url. */
export function pairingCodeFor(offer: Pick<OfferRecord, 'device' | 'nonce'>, answer: Pick<AnswerRecord, 'host' | 'nonce'>): Promise<string> {
  return pairingCode(
    b64uDecode(answer.host.key),
    b64uDecode(offer.device.key),
    b64uDecode(offer.nonce),
    b64uDecode(answer.nonce),
  );
}
