import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { AllowedClientCommand } from '../src/shared/clientCommands.js';
import { UiSurfaceLifecycleQueue } from '../src/renderer/bridge/uiSurfaceLifecycle.js';

function deferred(): { promise: Promise<void>; resolve(): void } {
  let resolvePromise!: () => void;
  const promise = new Promise<void>((resolve) => { resolvePromise = resolve; });
  return { promise, resolve: resolvePromise };
}

test('surface lifecycle serializes old-session detach before new attach and reuses the client id', async () => {
  const calls: Array<{ sessionId: string; command: AllowedClientCommand }> = [];
  const firstAttach = deferred();
  const firstStarted = deferred();
  const host = {
    command: async (sessionId: string, command: AllowedClientCommand) => {
      calls.push({ sessionId, command });
      if (calls.length === 1) {
        firstStarted.resolve();
        await firstAttach.promise;
      }
    },
  };
  const lifecycle = new UiSurfaceLifecycleQueue();
  const clientId = 'renderer-window-1';

  const attachOld = lifecycle.attach(host, 'session-old', clientId);
  const detachOld = lifecycle.detach(host, 'session-old', clientId);
  const attachNew = lifecycle.attach(host, 'session-new', clientId);
  const detachOnUnmount = lifecycle.detach(host, 'session-new', clientId);

  await firstStarted.promise;
  assert.deepEqual(calls, [{
    sessionId: 'session-old',
    command: { type: 'ui_attach', surface: 'desktop', client_id: clientId },
  }]);
  firstAttach.resolve();
  await Promise.all([attachOld, detachOld, attachNew, detachOnUnmount]);

  assert.deepEqual(calls, [
    { sessionId: 'session-old', command: { type: 'ui_attach', surface: 'desktop', client_id: clientId } },
    { sessionId: 'session-old', command: { type: 'ui_detach', client_id: clientId } },
    { sessionId: 'session-new', command: { type: 'ui_attach', surface: 'desktop', client_id: clientId } },
    { sessionId: 'session-new', command: { type: 'ui_detach', client_id: clientId } },
  ]);
});

test('queued surface commands skip a disposed owner at dispatch time and permit an explicitly reopened owner', async () => {
  const removed = new Set<string>();
  const lifecycle = new UiSurfaceLifecycleQueue(sessionId => !removed.has(sessionId));
  const firstAttach = deferred(), firstStarted = deferred();
  const calls: Array<{ sessionId: string; type: string }> = [];
  const host = { command: async (sessionId: string, command: AllowedClientCommand) => {
    calls.push({ sessionId, type: command.type });
    if (calls.length === 1) { firstStarted.resolve(); await firstAttach.promise; }
  } };
  const first = lifecycle.attach(host, 'A', 'window');
  const detach = lifecycle.detach(host, 'A', 'window');
  const obsoleteAttach = lifecycle.attach(host, 'A', 'window');
  const next = lifecycle.attach(host, 'B', 'window');
  await firstStarted.promise;
  removed.add('A'); firstAttach.resolve();
  await Promise.all([first, detach, obsoleteAttach, next]);
  assert.deepEqual(calls, [{ sessionId: 'A', type: 'ui_attach' }, { sessionId: 'B', type: 'ui_attach' }]);
  removed.delete('A');
  await lifecycle.attach(host, 'A', 'window');
  assert.deepEqual(calls.at(-1), { sessionId: 'A', type: 'ui_attach' });
});
