/**
 * Message fragmentation. An RPC message is one UTF-8 JSON text; it travels as
 * fragments `[kind][payload]` of at most 16 KiB each (one DataChannel message,
 * or one entry of a tunnel frame). kind `0` is a whole message, `1` means more
 * follows, `2` is the last piece.
 *
 * Fragments cut the UTF-8 bytes anywhere, even inside a multi-byte character;
 * decoding happens once, after reassembly.
 */

import { concatBytes, utf8Decode, utf8Encode, type Bytes } from './encoding.js';
import { RemoteProtocolError } from './errors.js';
import { FRAGMENT_MAX_BYTES, MESSAGE_MAX_BYTES } from './version.js';

/** The `kind` byte of a fragment. */
export const FRAGMENT_KIND = { whole: 0, more: 1, last: 2 } as const;

/**
 * Split `text` into fragments, each at most `maxFragmentBytes` (default
 * {@link FRAGMENT_MAX_BYTES}) including the kind byte. The empty message is one
 * fragment, `[0]`. Throws {@link RemoteProtocolError} above {@link MESSAGE_MAX_BYTES}.
 */
export function encodeMessage(text: string, maxFragmentBytes = FRAGMENT_MAX_BYTES): Bytes[] {
  if (!Number.isInteger(maxFragmentBytes) || maxFragmentBytes < 2 || maxFragmentBytes > FRAGMENT_MAX_BYTES) {
    throw new RangeError('maxFragmentBytes must be an integer from 2 to 16384');
  }
  const bytes = utf8Encode(text);
  if (bytes.length > MESSAGE_MAX_BYTES) throw new RemoteProtocolError('message exceeds the size limit');
  const payloadMax = maxFragmentBytes - 1;
  if (bytes.length <= payloadMax) return [fragment(FRAGMENT_KIND.whole, bytes)];
  const fragments: Bytes[] = [];
  for (let offset = 0; offset < bytes.length; offset += payloadMax) {
    const end = Math.min(offset + payloadMax, bytes.length);
    fragments.push(fragment(end === bytes.length ? FRAGMENT_KIND.last : FRAGMENT_KIND.more, bytes.subarray(offset, end)));
  }
  return fragments;
}

function fragment(kind: number, payload: Uint8Array): Bytes {
  const out = new Uint8Array(1 + payload.length);
  out[0] = kind;
  out.set(payload, 1);
  return out;
}

/**
 * Stateful inverse of {@link encodeMessage}: push fragments in arrival order,
 * receive each completed message. Any violation throws
 * {@link RemoteProtocolError} (and the connection should be closed with
 * `protocol_error`): an empty or over-long fragment, an unknown kind, a
 * continuation without a start, a whole message in the middle of a split one,
 * a message above {@link MESSAGE_MAX_BYTES}, or invalid UTF-8.
 */
export class Reassembler {
  private chunks: Bytes[] = [];
  private total = 0;

  /** Feed one fragment; returns the message it completes, else `null`. */
  push(fragmentBytes: Uint8Array): string | null {
    try {
      return this.accept(fragmentBytes);
    } catch (error) {
      this.reset();
      throw error;
    }
  }

  /** Bytes of a partly received message. */
  get pendingBytes(): number {
    return this.total;
  }

  /** Drop any partly received message. */
  reset(): void {
    this.chunks = [];
    this.total = 0;
  }

  private accept(input: Uint8Array): string | null {
    if (input.length === 0) throw new RemoteProtocolError('empty fragment');
    if (input.length > FRAGMENT_MAX_BYTES) throw new RemoteProtocolError('fragment exceeds the size limit');
    const kind = input[0];
    const payload = input.subarray(1);
    switch (kind) {
      case FRAGMENT_KIND.whole:
        if (this.chunks.length > 0) throw new RemoteProtocolError('whole message inside a split message');
        return utf8Decode(payload, true);
      case FRAGMENT_KIND.more:
      case FRAGMENT_KIND.last: {
        if (kind === FRAGMENT_KIND.last && this.chunks.length === 0) {
          throw new RemoteProtocolError('continuation without a start');
        }
        if (this.total + payload.length > MESSAGE_MAX_BYTES) throw new RemoteProtocolError('message exceeds the size limit');
        // Copy: the caller may reuse the fragment's buffer while the rest arrives.
        this.chunks.push(payload.slice());
        this.total += payload.length;
        if (kind === FRAGMENT_KIND.more) return null;
        const message = utf8Decode(concatBytes(...this.chunks), true);
        this.reset();
        return message;
      }
      default:
        throw new RemoteProtocolError('unknown fragment kind');
    }
  }
}
