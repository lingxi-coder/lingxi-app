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
