import { test } from 'node:test';
import assert from 'node:assert/strict';

import { createRuntimeEventReplayBuffer, mergeRuntimeEventReplay } from '../src/preload/event-replay';

test('renderer event replay merges cached and live-racing envelopes exactly once', () => {
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const resumed = { sessionId, sequence: 1, event: { type: 'session_resumed' } };
  const text = { sessionId, sequence: 2, event: { type: 'text_delta', text: 'hello' } };
  const ended = { sessionId, sequence: 3, event: { type: 'turn_ended' } };

  const merged = mergeRuntimeEventReplay([resumed, text], [text, ended]);

  assert.deepEqual(merged.map((entry) => entry.sequence), [1, 2, 3]);
  assert.deepEqual(merged.map((entry) => entry.event.type), ['session_resumed', 'text_delta', 'turn_ended']);
});

test('new renderer subscription receives cached base and live-racing events once', () => {
  const sessionId = '22222222-3333-4444-8555-666666666666';
  const delivered: number[] = [];
  const receiver = createRuntimeEventReplayBuffer<{ type: string }>((envelope) => delivered.push(envelope.sequence));
  const resumed = { sessionId, sequence: 1, event: { type: 'session_resumed' } };
  const text = { sessionId, sequence: 2, event: { type: 'text_delta' } };
  const ended = { sessionId, sequence: 3, event: { type: 'turn_ended' } };

  receiver.push(text);
  receiver.resolve([resumed, text]);
  receiver.push(ended);

  assert.deepEqual(delivered, [1, 2, 3]);
});
