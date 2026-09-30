import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import type { ClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import {
  createRuntimeEventReplayBuffer,
  type SequencedRuntimeEventEnvelope,
} from '../src/preload/event-replay';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';

test('compaction events preserve their originating session when two runtimes share a project and window', () => {
  const firstId = '11111111-2222-4333-8444-555555555555';
  const secondId = '22222222-3333-4444-8555-666666666666';
  const conversations = new Map([
    [firstId, emptyConversation()], [secondId, emptyConversation()],
  ]);
  const received: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  const buffer = createRuntimeEventReplayBuffer<ClientEvent>((envelope) => {
    received.push(envelope);
    conversations.set(envelope.sessionId, reduceEvent(conversations.get(envelope.sessionId)!, envelope.event));
  });
  const window = Object.assign(new EventEmitter(), {
    isDestroyed: () => false,
    send: (channel: string, payload: SequencedRuntimeEventEnvelope<ClientEvent>) => {
      if (channel === 'lingxi:event') buffer.push(payload);
    },
  });
  const clients = [firstId, secondId].map((sessionId) => {
    const client = new EventEmitter();
    const runtime = new SessionRuntime({
      sessionId, projectPath: '/shared/project', envelopeEvents: true,
      launchConfig: () => ({ sessionId, workspace: '/shared/project', trusted: true }),
    });
    runtime.registerWindow(window as any, 'app://desktop/index.html');
    (runtime as any).wireClient(client, (runtime as any).generation);
    client.emit('event', { type: 'session_started', session_id: sessionId });
    return client;
  });
  const firstBefore = conversations.get(firstId);
  clients[1]!.emit('event', { type: 'compaction_status', phase: 'summarizing' });
  const completed: ClientEvent = {
    type: 'compaction_completed', messages_before: 51, messages_after: 10,
    bytes_saved: 592896, summary: 'Second session summary',
  };
  clients[1]!.emit('event', completed);
  // Deliver buffered events through the real preload replay merge as on reload.
  buffer.resolve([]);
  assert.equal(conversations.get(firstId)!.items.length, firstBefore!.items.length);
  assert.equal(conversations.get(firstId)!.summaries.length, 0);
  assert.equal(conversations.get(secondId)!.summaries[0]?.content, 'Second session summary');
  assert.deepEqual(received.filter(({ event }) => event.type.startsWith('compaction_')).map(({ sessionId }) => sessionId), [secondId, secondId]);

  // A later independent compaction in the first session must not alter the second.
  const secondBefore = conversations.get(secondId);
  clients[0]!.emit('event', { type: 'compaction_status', phase: 'summarizing' });
  clients[0]!.emit('event', { ...completed, summary: 'First session summary' });
  assert.equal(conversations.get(secondId), secondBefore);
  assert.equal(conversations.get(firstId)!.summaries[0]?.content, 'First session summary');
  buffer.dispose();
});
