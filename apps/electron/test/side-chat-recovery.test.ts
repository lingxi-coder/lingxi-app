import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import type { ClientCommand, ClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { SideChatRecovery } from '../src/main/sideChatRecovery.js';

function checkpoint(id = 'side-1'): ClientEvent {
  return {
    type: 'side_chat_checkpoint',
    side_id: id,
    checkpoint_json: `private-${id}`,
    events_json: ['user-row'],
  };
}

test('side recovery restores context and partial output without dispatching a prompt', async () => {
  const recovery = new SideChatRecovery();
  assert.equal(recovery.observe(checkpoint()), true);
  recovery.observe({
    type: 'side_chat_event',
    side_id: 'side-1',
    event_json: 'partial-output',
  });
  const sent: ClientCommand[] = [];
  let complete = false;
  const restored = recovery
    .restore((command) => {
      sent.push(command);
    })
    .then(() => {
      complete = true;
    });
  await Promise.resolve();
  assert.equal(complete, false, 'sending is not proof of restoration');
  assert.deepEqual(sent, [
    {
      type: 'side_chat_restore',
      side_id: 'side-1',
      checkpoint_json: 'private-side-1',
      events_json: ['user-row', 'partial-output'],
    },
  ]);
  recovery.observe({
    type: 'side_chat_snapshot',
    side_id: 'side-1',
    events_json: ['restored-user', 'cancelled'],
  });
  await restored;
  assert.equal(complete, true);
});

test('failures and transport interruptions retain side context for retry', async () => {
  const recovery = new SideChatRecovery();
  recovery.observe(checkpoint());
  await assert.rejects(
    recovery.restore(() => {
      throw new Error('connection lost');
    }),
    /connection lost/,
  );
  const interrupted = recovery.restore(() => {});
  recovery.interrupt();
  await assert.rejects(interrupted, /interrupted/);
  await recovery.restore(() => {
    recovery.observe({
      type: 'side_chat_snapshot',
      side_id: 'side-1',
      events_json: [],
    });
  });
  assert.equal(recovery.size, 1);
});

test('side identities are isolated and explicit end or disposal prevents resurrection', async () => {
  const recovery = new SideChatRecovery();
  recovery.observe(checkpoint('first'));
  recovery.observe(checkpoint('second'));
  recovery.observe({ type: 'side_chat_ended', side_id: 'first' });
  const sent: ClientCommand[] = [];
  await recovery.restore((command) => {
    sent.push(command);
    recovery.observe({
      type: 'side_chat_snapshot',
      side_id: 'second',
      events_json: [],
    });
  });
  assert.deepEqual(
    sent.map((command) =>
      command.type === 'side_chat_restore' ? command.checkpoint_json : '',
    ),
    ['private-second'],
  );
  recovery.clear();
  await recovery.restore(() => {
    assert.fail('disposed sides must not be restored');
  });
});

test('a newer checkpoint replaces stale replay and a backend rejection fails restoration', async () => {
  const recovery = new SideChatRecovery();
  recovery.observe(checkpoint());
  recovery.observe({
    type: 'side_chat_event',
    side_id: 'side-1',
    event_json: 'old-partial',
  });
  recovery.observe({
    ...checkpoint(),
    events_json: ['committed-replay'],
  } as ClientEvent);
  await assert.rejects(
    recovery.restore((command) => {
      if (command.type !== 'side_chat_restore')
        assert.fail('only restore is allowed');
      assert.deepEqual(command.events_json, ['committed-replay']);
      recovery.observe({
        type: 'side_chat_error',
        side_id: 'side-1',
        message: 'invalid checkpoint owner',
      });
    }),
    /invalid checkpoint owner/,
  );
  assert.equal(recovery.size, 1);
});

test('host-private checkpoint context never reaches renderer or remote event broadcasting', async () => {
  const runtime = new SessionRuntime({
    sessionId: '11111111-2222-4333-8444-555555555555',
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const internal = runtime as any;
  const client = Object.assign(new EventEmitter(), { sendCommand(): void {} });
  const events: unknown[] = [];
  internal.broadcastClientEvent = (event: unknown): void => {
    events.push(event);
  };
  internal.wireClient(client, internal.generation);
  client.emit('event', checkpoint());
  assert.equal(internal.sideChatRecovery.size, 1);
  assert.deepEqual(events, []);
  await runtime.dispose();
});
