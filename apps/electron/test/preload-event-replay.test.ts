import { test } from 'node:test';
import assert from 'node:assert/strict';

import { createRuntimeEventReplayBuffer, mergeRuntimeEventReplay, requestRuntimeEventReplay } from '../src/preload/event-replay';

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

test('a background consumer error does not discard another session cold transcript', () => {
  const delivered: string[] = [];
  const errors: unknown[] = [];
  const receiver = createRuntimeEventReplayBuffer<string>(envelope => {
    if (envelope.sessionId === 'background') throw new Error('background reducer failed');
    delivered.push(envelope.event);
  }, error => errors.push(error));
  receiver.resolve([
    { sessionId: 'background', sequence: 1, event: 'invalid background event' },
    { sessionId: 'cold', sequence: 1, event: 'saved transcript' },
    { sessionId: 'cold', sequence: 2, event: 'saved model' },
  ]);
  assert.deepEqual(delivered, ['saved transcript', 'saved model']);
  assert.equal(errors.length, 1);
});

test('cold replay retries a transient IPC failure without losing or duplicating buffered live events', async () => {
  const delivered: string[] = [];
  const receiver = createRuntimeEventReplayBuffer<string>(envelope => delivered.push(envelope.event));
  const base = { sessionId: 'cold', sequence: 1, event: 'saved transcript' };
  const live = { sessionId: 'cold', sequence: 2, event: 'live continuation' };
  let attempts = 0;
  let done!: () => void;
  const completed = new Promise<void>(resolve => { done = resolve; });
  const stop = requestRuntimeEventReplay(async () => {
    if (++attempts === 1) { receiver.push(live); throw new Error('IPC not ready'); }
    return [base, live];
  }, {
    resolve: replay => { receiver.resolve(replay); done(); },
    reject: () => assert.fail('transient IPC failure must recover'),
  });
  try {
    await completed;
    assert.equal(attempts, 2);
    assert.deepEqual(delivered, ['saved transcript', 'live continuation']);
  } finally { stop(); receiver.dispose(); }
});

test('disposing a cold replay subscription cancels scheduled retries', async () => {
  let attempts = 0;
  let failed!: () => void;
  const requested = new Promise<void>(resolve => { failed = resolve; });
  const stop = requestRuntimeEventReplay(async () => {
    attempts++; failed(); throw new Error('IPC not ready');
  }, { resolve: () => assert.fail('disposed subscription delivered'), reject: () => assert.fail('disposed subscription rejected') });
  await requested;
  await Promise.resolve();
  stop();
  await new Promise(resolve => setTimeout(resolve, 300));
  assert.equal(attempts, 1);
});

test('a stalled cold replay request recovers before the renderer restore deadline', async () => {
  const delivered: string[] = [];
  const receiver = createRuntimeEventReplayBuffer<string>(envelope => delivered.push(envelope.event));
  let attempts = 0;
  let done!: () => void;
  const completed = new Promise<void>(resolve => { done = resolve; });
  const stop = requestRuntimeEventReplay(() => {
    if (++attempts === 1) return new Promise(() => {});
    return Promise.resolve([{ sessionId: 'cold', sequence: 1, event: 'saved transcript' }]);
  }, { resolve: replay => { receiver.resolve(replay); done(); }, reject: () => assert.fail('stalled IPC must retry') });
  try {
    await completed;
    assert.equal(attempts, 2);
    assert.deepEqual(delivered, ['saved transcript']);
  } finally { stop(); receiver.dispose(); }
});

test('permanent replay failure is bounded and releases buffered live events with diagnostics', async () => {
  let attempts = 0;
  const delivered: string[] = [], errors: unknown[] = [];
  const receiver = createRuntimeEventReplayBuffer<string>(event => delivered.push(event.event));
  receiver.push({ sessionId: 'cold', sequence: 2, event: 'live event' });
  let done!: () => void;
  const completed = new Promise<void>(resolve => { done = resolve; });
  const stop = requestRuntimeEventReplay(async () => { attempts++; throw new Error('unavailable'); }, {
    resolve: () => assert.fail('permanent error resolved'),
    reject: () => { receiver.reject(); done(); },
  }, error => errors.push(error));
  try {
    await completed;
    assert.equal(attempts, 3);
    assert.equal(errors.length, 1);
    assert.deepEqual(delivered, ['live event']);
  } finally { stop(); receiver.dispose(); }
});
