import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { ClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';

test('remote and renderer snapshots remain stable across streamed appends, status replacement and replay resets', async t => {
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  t.after(() => runtime.dispose());
  const emit = (event: ClientEvent) => (runtime as any).broadcastClientEvent(event);
  const empty = runtime.remoteView();
  emit({ type: 'session_started', session_id: sessionId, mode: 'code' });
  emit({ type: 'text_delta', text: 'first' });
  emit({ type: 'ui_status', plugin: 'one', text: 'original' });
  const view = runtime.remoteView();
  const renderer = runtime.replaySnapshot();
  const capturedEvents = structuredClone(view.events);
  const capturedRenderer = structuredClone(renderer);
  for (let index = 0; index < 2_000; index++) emit({ type: 'text_delta', text: String(index) });
  emit({ type: 'compaction_status', phase: 'summarizing' });
  emit({ type: 'ui_status', plugin: 'two', text: 'second' });
  emit({ type: 'ui_status', plugin: 'one', text: 'replacement' });
  assert.deepEqual(empty.events, []);
  assert.deepEqual(view.events, capturedEvents, 'an existing remote view must not grow or lose coalesced events');
  assert.deepEqual(renderer, capturedRenderer, 'renderer replay must remain an immutable captured prefix');
  const current = runtime.remoteView();
  assert.equal(current.events.filter(({ event }) => event.type === 'text_delta').length, 2_001);
  assert.deepEqual(current.events.filter(({ event }) => event.type === 'ui_status').map(({ event }) => event), [
    { type: 'ui_status', plugin: 'two', text: 'second' },
    { type: 'ui_status', plugin: 'one', text: 'replacement' },
  ]);
  assert.deepEqual(current.events.map(event => event.sequence), current.events.map(event => event.sequence).sort((a, b) => a - b));
  const capturedCurrent = structuredClone(current.events);
  emit({ type: 'session_resumed', session_id: sessionId, mode: 'code', messages: [] });
  assert.deepEqual(current.events, capturedCurrent, 'replacing the replay base must not invalidate an in-flight remote snapshot');
  assert.deepEqual(runtime.remoteView().events, []);
  assert.equal(runtime.replaySnapshot()[0].event.type, 'session_resumed');
});
