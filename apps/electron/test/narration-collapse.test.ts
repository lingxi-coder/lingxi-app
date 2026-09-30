import test from 'node:test';
import assert from 'node:assert/strict';

import {
  ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS,
  ASSISTANT_NARRATION_COLLAPSE_MAX_LINES,
  NARRATION_COLLAPSE_MAX_CHARS,
  NARRATION_COLLAPSE_MAX_LINES,
  narrationDefaultOpen,
  narrationShouldCollapse,
  type NarrationRunItem,
} from '../src/renderer/model/runItem';

const narration = (text: string, extra: Partial<NarrationRunItem> = {}): NarrationRunItem => ({
  type: 'narration',
  id: 'i1',
  role: 'assistant',
  text,
  ...extra,
});

test('assistant messages only collapse after the large character boundary', () => {
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(NARRATION_COLLAPSE_MAX_CHARS + 1))), false);
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS))), false);
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS + 1))), true);
});

test('assistant messages only collapse after the large hard-line boundary', () => {
  const boundary = Array.from({ length: ASSISTANT_NARRATION_COLLAPSE_MAX_LINES }, (_, i) => `line ${i + 1}`).join('\r\n');
  const overBoundary = `${boundary}\r\nlast line`;
  assert.equal(narrationShouldCollapse(narration(boundary)), false);
  assert.equal(narrationShouldCollapse(narration(overBoundary)), true);
});

test('user messages retain the compact collapse budget', () => {
  const boundary = Array.from({ length: NARRATION_COLLAPSE_MAX_LINES }, (_, i) => `line ${i + 1}`).join('\r\n');
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(NARRATION_COLLAPSE_MAX_CHARS), { role: 'user' })), false);
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(NARRATION_COLLAPSE_MAX_CHARS + 1), { role: 'user' })), true);
  assert.equal(narrationShouldCollapse(narration(boundary, { role: 'user' })), false);
  assert.equal(narrationShouldCollapse(narration(`${boundary}\r\nline 9`, { role: 'user' })), true);
});

test('blank, short, and roleless notices do not gain message disclosures', () => {
  assert.equal(narrationShouldCollapse(narration('   ')), false);
  assert.equal(narrationShouldCollapse(narration('A concise answer.')), false);
  assert.equal(narrationShouldCollapse(narration('x'.repeat(900), { role: undefined })), false);
});

test('long live replies stay open while long user and historical replies start closed', () => {
  const long = 'x'.repeat(ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS + 1);
  assert.equal(narrationDefaultOpen(narration(long, { streamed: true })), true);
  assert.equal(narrationDefaultOpen(narration(long)), false);
  assert.equal(narrationDefaultOpen(narration('x'.repeat(NARRATION_COLLAPSE_MAX_CHARS + 1), { role: 'user' })), false);
  assert.equal(narrationDefaultOpen(narration('short', { role: 'user' })), true);
});
