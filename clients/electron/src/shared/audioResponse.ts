import { MAX_BRIDGE_FRAME_BYTES } from '@lingxi/bridge-client/protocol';

import { isBase64 } from './base64.js';

/**
 * Payload bounds for `ClientCommand::AudioResponse`, declared ONCE.
 *
 * Two processes need the same numbers for opposite reasons, which is exactly
 * why they cannot each keep their own copy:
 *
 * - `main/validation.ts` ENFORCES them. A response that fails its gate never
 *   reaches the engine at all.
 * - `renderer/audio/requests.ts` must STAY INSIDE them. The engine parks the
 *   originating call on a deadline (5s / 30s / 180s per op, and a
 *   text-derived one for `synthesize`; see `audio_bridge.rs`), so a response
 *   rejected at the gate is not a validation
 *   error the user sees — it is a silent stall of exactly that length,
 *   followed by a failure with no explanation. The renderer therefore checks
 *   these bounds itself and answers with a real, honest failure instead.
 *
 * Two copies of these numbers would drift the same way `AllowedClientCommand`
 * already did before it was consolidated into `clientCommands.ts`, and the
 * symptom of the drift would be that stall — the hardest possible thing to
 * attribute back to a mismatched constant. Hence `shared/`, which both the
 * main process and the renderer can reach.
 */

/**
 * Room reserved inside one WebSocket frame for everything an `audio_response`
 * carries BESIDES the payload: the `Frame::Request` envelope, the command tag,
 * the request id, the JSON punctuation, and the mime type (up to
 * {@link MAX_AUDIO_MIME_TYPE_LENGTH} code units, so up to ~1 KiB of UTF-8).
 * Comfortably over the few hundred bytes those actually need — the cost of the
 * slack is a fraction of a second of recording, and the cost of being short is
 * the whole session.
 */
const AUDIO_RESPONSE_FRAMING_ALLOWANCE = 64 * 1024;

/**
 * Bound on a base64 audio payload, DERIVED from the transport's own ceiling
 * rather than chosen alongside it.
 *
 * This bound used to be 24 MiB, picked as "generous, still bounds an IPC
 * frame". It was above the engine's 16 MiB WebSocket read limit
 * ({@link MAX_BRIDGE_FRAME_BYTES}), and the gap was reachable by ordinary use —
 * the USER, not this process, decides how long to hold the microphone, and
 * ~13 minutes of Opus at Chromium's default bitrate lands in it. A clip in that
 * range passed the renderer's own guard AND `main/validation.ts`'s gate, and
 * then killed the connection: an over-length frame is not a rejected command,
 * it is `Err(Capacity(MessageTooLong))` → `close_connection` → the turn aborted
 * and every broker drained. The user lost the session, not the recording.
 *
 * Deriving it is the part that matters. Two numbers "kept in sync" by a comment
 * drift; a number computed from the other cannot. `audio-engine-bounds.test.ts`
 * closes the remaining half by pinning {@link MAX_BRIDGE_FRAME_BYTES} against
 * the engine's own `MAX_INBOUND_FRAME_BYTES` and by measuring a maximal
 * `audio_response` frame against it.
 *
 * Raising it is not the fix for a clip that exceeds it: the renderer reports an
 * oversize clip as a failure, which the engine surfaces immediately. Genuinely
 * unbounded audio needs a chunked wire format, which is a protocol change
 * rather than a bigger number here — and this one has no room left to grow into
 * anyway.
 */
export const MAX_AUDIO_BASE64_LENGTH = MAX_BRIDGE_FRAME_BYTES - AUDIO_RESPONSE_FRAMING_ALLOWANCE;

/** Bound on `AudioResultDto::Failed`'s message. The renderer trims to fit. */
export const MAX_AUDIO_FAILURE_MESSAGE_LENGTH = 4096;

/** Bound on a recording's reported mime type (`audio/webm;codecs=opus` and friends). */
export const MAX_AUDIO_MIME_TYPE_LENGTH = 256;

/** Highest plausible PCM sample rate. `0` is legal — it is half of the "played in place" pair. */
export const MAX_AUDIO_SAMPLE_RATE_HZ = 768_000;


// ---------------------------------------------------------------------------
// What a sendable audio response may CONTAIN.
//
// The bounds above are numbers; these are the field rules, and they exist for
// the same reason. `main/validation.ts` refuses a response whose fields break
// them, and a refused response never reaches the engine - so the renderer has
// to know the same rules to avoid ever building one. Stating them twice is
// exactly how the NUL-in-a-message stall happened: the renderer's `describe()`
// handled emptiness and length, the gate ALSO refused control characters, and
// nothing connected the two. These predicates are that connection - the gate
// calls them to reject, the renderer calls them to avoid producing.
// ---------------------------------------------------------------------------

/**
 * Characters stripped from a generated failure message.
 *
 * The gate's hard requirement is narrower - `string()` refuses only `\0` - so
 * this is deliberately STRICTER, which is always safe: a message this accepts
 * is always a message the gate accepts, never the reverse. The extra reach
 * covers the rest of the C0 block and DEL, because these strings come from
 * `DOMException` text that ends up in diagnostics and logs, where a stray
 * escape sequence is its own small hazard. `\t`, `\n` and `\r` are kept:
 * they are legal, and a multi-line error message is worth more than a
 * flattened one.
 *
 * Two regexes for one character class on purpose - a single `/g` literal
 * carries `lastIndex` between `.test()` calls and would return alternating
 * answers for the same input.
 */
const FORBIDDEN_TEXT_MATCH = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/;
const FORBIDDEN_TEXT_REPLACE = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g;

/**
 * Whether a value may travel in a wire string field: a non-empty string, no
 * longer than `maxLength`, carrying no character this module forbids.
 */
export function isSendableAudioText(value: unknown, maxLength: number): value is string {
  return typeof value === 'string'
    && value.length > 0
    && value.length <= maxLength
    && !FORBIDDEN_TEXT_MATCH.test(value);
}

/**
 * Makes an arbitrary description sendable: strips forbidden characters, trims,
 * truncates to the message bound, and substitutes `fallback` rather than ever
 * returning the empty string, which the gate also refuses.
 */
export function sanitizeAudioMessage(raw: string, fallback: string): string {
  const scrubbed = raw.replace(FORBIDDEN_TEXT_REPLACE, ' ').trim();
  if (scrubbed.length === 0) return fallback;
  return scrubbed.length > MAX_AUDIO_FAILURE_MESSAGE_LENGTH
    ? `${scrubbed.slice(0, MAX_AUDIO_FAILURE_MESSAGE_LENGTH - 1)}\u2026`
    : scrubbed;
}

/**
 * Whether a value may travel in a base64 audio field. The empty string is
 * legal - it is half of the "played in place" synthesis pair.
 */
export function isSendableAudioBase64(value: unknown): value is string {
  return typeof value === 'string'
    && value.length <= MAX_AUDIO_BASE64_LENGTH
    && isBase64(value);
}

/** Whether a value may travel as a sample rate. `0` is legal - see the bound's doc. */
export function isSendableAudioSampleRate(value: unknown): value is number {
  return Number.isSafeInteger(value)
    && (value as number) >= 0
    && (value as number) <= MAX_AUDIO_SAMPLE_RATE_HZ;
}
