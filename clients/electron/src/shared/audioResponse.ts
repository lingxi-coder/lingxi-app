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
 *   originating call on a deadline (5s / 30s / 180s per op, see
 *   `audio_bridge.rs`), so a response rejected at the gate is not a validation
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
 * Bound on a base64 audio payload. Generous by design: a `stop_recording`
 * answer carries a whole clip, whose length the USER (not this process)
 * chooses by how long they hold the microphone. 24 MiB of base64 is ~18 MiB
 * of Opus — hours of speech — while still bounding an IPC frame.
 *
 * Raising it is not the fix for a clip that exceeds it: the renderer reports
 * an oversize clip as a failure, which the engine surfaces immediately.
 * Genuinely unbounded audio needs a chunked wire format, which is a protocol
 * change rather than a bigger number here.
 */
export const MAX_AUDIO_BASE64_LENGTH = 24 * 1024 * 1024;

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
