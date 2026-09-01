import test from 'node:test';
import assert from 'node:assert/strict';

import {
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

test('narration collapse character boundary counts Unicode code points', () => {
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(NARRATION_COLLAPSE_MAX_CHARS))), false);
  assert.equal(narrationShouldCollapse(narration('🙂'.repeat(NARRATION_COLLAPSE_MAX_CHARS + 1))), true);
});

test('narration collapse line boundary normalizes CRLF and starts after eight hard lines', () => {
  const eight = Array.from({ length: NARRATION_COLLAPSE_MAX_LINES }, (_, i) => `line ${i + 1}`).join('\r\n');
  const nine = `${eight}\r\nline 9`;
  assert.equal(narrationShouldCollapse(narration(eight)), false);
  assert.equal(narrationShouldCollapse(narration(nine)), true);
});

test('blank, short, and roleless notices do not gain message disclosures', () => {
  assert.equal(narrationShouldCollapse(narration('   ')), false);
  assert.equal(narrationShouldCollapse(narration('A concise answer.')), false);
  assert.equal(narrationShouldCollapse(narration('x'.repeat(900), { role: undefined })), false);
});

test('long live replies stay open while long user and historical replies start closed', () => {
  const long = 'x'.repeat(NARRATION_COLLAPSE_MAX_CHARS + 1);
  assert.equal(narrationDefaultOpen(narration(long, { streamed: true })), true);
  assert.equal(narrationDefaultOpen(narration(long)), false);
  assert.equal(narrationDefaultOpen(narration(long, { role: 'user' })), false);
  assert.equal(narrationDefaultOpen(narration('short', { role: 'user' })), true);
});
