import assert from 'node:assert/strict';
import { test } from 'node:test';
import { transcriptRowGap, transcriptWindowRange } from '../src/renderer/components/useTranscriptWindow';

test('window ranges cover either end and arbitrary positions in a large transcript', () => {
  const offsets = Array.from({ length: 10_001 }, (_, index) => index * 60);
  for (const top of [0, 300_000, 599_400]) {
    const range = transcriptWindowRange(offsets, top, 600);
    assert.ok(range.start <= Math.floor(top / 60));
    assert.ok(range.end >= Math.min(10_000, Math.ceil((top + 600) / 60)));
    assert.ok(range.end - range.start <= 35, 'mounted range depends on viewport, not history length');
  }
  assert.deepEqual(transcriptWindowRange([0], 0, 600), { start: 0, end: 0 });
});

test('window boundaries preserve user-turn and compact activity spacing', () => {
  const assistant = { id: 'a', type: 'narration', role: 'assistant' };
  const user = { id: 'u', type: 'narration', role: 'user' };
  const tools = { id: 't', type: 'tool-group' };
  const agents = { id: 'g', type: 'agents' };
  assert.equal(transcriptRowGap(undefined, assistant), 0);
  assert.equal(transcriptRowGap(assistant, assistant), 1);
  assert.equal(transcriptRowGap(user, tools), 1.5);
  assert.equal(transcriptRowGap(tools, user), 1);
  assert.equal(transcriptRowGap(tools, agents), 0.125);
  assert.equal(transcriptRowGap(assistant, tools), 0.75);
});
