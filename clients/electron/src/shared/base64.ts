/**
 * Standard base64 recognition that does not blow the stack.
 *
 * The obvious pattern for this job —
 * `/^(?:[A-Za-z0-9+\/]{4})*(?:[A-Za-z0-9+\/]{2}==|[A-Za-z0-9+\/]{3}=)?$/` —
 * repeats a GROUP, and V8 pushes a backtrack frame per repetition. Measured on
 * this project's Node: correct up to about 4 MB of input, and
 * `RangeError: Maximum call stack size exceeded` by 8 MB. Every test written
 * against it passed anyway, because the fixtures were four characters long or
 * were rejected for length before the pattern ever ran.
 *
 * That mattered in two places, both reproduced against real code:
 *
 * - `validateClientCommand` on a ~6 MB `stop_recording` answer — a few minutes
 *   of speech, well inside the 24 MiB cap — threw instead of validating. The
 *   response never reached the engine, so the call parked for its whole
 *   30-second deadline and then failed with nothing to explain it.
 * - `validateImageRefs` on a ~6 MB pasted image, well inside the 20 MiB cap,
 *   threw the same way and failed the whole prompt.
 *
 * A single character class compiles to a loop with no per-iteration frame, so
 * the body is matched that way and the padding is counted directly. The
 * accepted set is unchanged — `base64-validation.test.ts` proves that
 * exhaustively over every string up to length 5 that mixes body characters,
 * padding and an illegal character.
 */

/** The base64 body alphabet. One character class, deliberately — see the header. */
const BASE64_BODY = /^[A-Za-z0-9+/]*$/;

const EQUALS = 0x3d;

/**
 * Whether `value` is standard, correctly padded base64.
 *
 * The empty string is accepted: it is the desktop's "already played in place"
 * synthesis payload (`renderer/audio/synthesis.ts`), and `base64::decode("")`
 * is `Ok(vec![])` on the engine side.
 *
 * Padding placement needs no separate rule. `=` is not in the body alphabet,
 * so any `=` before the final one or two is caught by the body scan, and with
 * the length a multiple of 4 the "two pads follow two body characters, one pad
 * follows three" requirement is implied.
 */
export function isBase64(value: string): boolean {
  if (value.length % 4 !== 0) return false;
  let end = value.length;
  if (end > 0 && value.charCodeAt(end - 1) === EQUALS) end -= 1;
  if (end > 0 && value.charCodeAt(end - 1) === EQUALS) end -= 1;
  return BASE64_BODY.test(end === value.length ? value : value.slice(0, end));
}
