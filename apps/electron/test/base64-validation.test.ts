import { test } from 'node:test';
import assert from 'node:assert/strict';

import { isBase64 } from '../src/shared/base64';
import { validateImageRefs } from '../src/main/validation';
import { validateNativeAudioOperationResult } from '../src/shared/nativeAudio';

/**
 * The base64 validator used to be
 * `/^(?:[A-Za-z0-9+\/]{4})*(?:[A-Za-z0-9+\/]{2}==|[A-Za-z0-9+\/]{3}=)?$/`.
 *
 * That pattern repeats a GROUP, and V8 pushes a backtrack frame per
 * repetition, so it throws `RangeError: Maximum call stack size exceeded`
 * somewhere between 4 MB and 8 MB of input — measured, not assumed. Below that
 * it is correct, which is why every test written against it passed: the
 * fixtures were `'AAEC'` and a payload rejected for length before the pattern
 * ever ran.
 *
 * `validateImageRefs` and the native-audio operation result validator both
 * scan such payloads without calling a regex path that recurses per group.
 *
 * `isBase64` scans a character class instead of repeating a group, which V8
 * compiles to a loop with no per-iteration frame.
 */

/** ~6 MB of base64: ~4.5 MB of Opus, or a 4.5 MB PNG. Both are ordinary sizes. */
const SIX_MB = 'A'.repeat(6_000_000);

test('a bounded native recording far larger than the old stack limit validates without stack recursion', () => {
  assert.doesNotThrow(() => validateNativeAudioOperationResult({
    type: 'recording', audio_base64: SIX_MB, mime_type: 'audio/webm',
  }), 'a few minutes of speech must pass the production native-result validator without stack recursion');
});

test('an image far larger than the old stack limit validates instead of throwing', () => {
  // Adjacent defect, same shared constant: this path never reached the audio
  // work, but it was broken by the pattern the audio gate borrowed.
  assert.throws(
    () => validateImageRefs([{ media_type: 'image/png', base64: SIX_MB }]),
    /invalid image format/,
    'a 4.5 MB pasted image must be rejected for its CONTENT (these bytes are not a PNG), '
    + 'not by a RangeError out of the validator',
  );
});

test('the new predicate accepts exactly what the old pattern accepted', () => {
  // Exhaustive over every string up to length 5 drawn from an alphabet that
  // includes a body character, both padding positions, a separator that is
  // legal in base64, and one that is not. 9330 strings — every padding
  // arrangement, every length residue, every illegal placement. This is the
  // check that makes replacing a validator safe: not "the cases I thought of",
  // but "the accepted set is unchanged wherever the old one can be evaluated".
  const GROUPED = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
  const previouslyAccepted = (value: string) => value.length % 4 === 0 && GROUPED.test(value);

  const alphabet = ['A', '9', '+', '/', '=', '!'];
  let checked = 0;
  let accepted = 0;
  const expand = (prefix: string, depth: number) => {
    assert.equal(
      isBase64(prefix),
      previouslyAccepted(prefix),
      `disagreement on ${JSON.stringify(prefix)}`,
    );
    checked += 1;
    if (previouslyAccepted(prefix)) accepted += 1;
    if (depth === 0) return;
    for (const character of alphabet) expand(prefix + character, depth - 1);
  };
  expand('', 5);

  assert.equal(checked, 9331, 'the sweep must actually cover the whole space it claims');
  assert.ok(accepted > 100, `the corpus must contain plenty of ACCEPTED strings, not just rejections (got ${accepted})`);
});

test('the equivalence sweep can actually fail', () => {
  // If `isBase64` were `() => true` the sweep above would have to notice.
  const GROUPED = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
  assert.equal(GROUPED.test('AA!A'), false, 'the old pattern must reject a non-alphabet character');
  assert.equal(isBase64('AA!A'), false);
  assert.equal(isBase64('A=AA'), false, 'padding in the middle is not base64');
  assert.equal(isBase64('AAA'), false, 'a length that is not a multiple of 4 is not base64');
  assert.equal(isBase64('AA=='), true);
  assert.equal(isBase64('AAA='), true);
  assert.equal(isBase64(''), true, 'the empty string is the played-in-place payload');
});
