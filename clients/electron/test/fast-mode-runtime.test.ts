import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge';
import { SettingsStore } from '../src/main/settings';
import type { ClientCommand } from '@lingxi/bridge-client';

function runtime(store: SettingsStore) {
  const commands: ClientCommand[] = [];
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (command: ClientCommand) => commands.push(command),
  });
  const instance = new SessionRuntime({
    projectPath: '/workspace',
    sessionId: '11111111-2222-4333-8444-555555555555',
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    getSavedFastMode: () => store.getLastFastMode(),
    onFastModeSelected: (enabled) => store.setLastFastMode(enabled),
  });
  Object.assign(instance, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true });
  (instance as any).wireClient(client, 0);
  return { instance, client, commands };
}

test('only an acknowledged Fast mode selection persists and a new runtime restores it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-fast-mode-'));
  try {
    const store = new SettingsStore(dir);
    const first = runtime(store);
    const selected = first.instance.dispatchCommand({ type: 'set_fast_mode', enabled: true });
    assert.equal(store.getLastFastMode(), undefined);
    first.client.emit('event', { type: 'fast_mode_changed', enabled: false });
    assert.equal(store.getLastFastMode(), undefined);
    first.client.emit('event', { type: 'fast_mode_changed', enabled: true });
    await selected;
    assert.equal(new SettingsStore(dir).getLastFastMode(), true);

    const second = runtime(new SettingsStore(dir));
    const restoring = (second.instance as any).restoreFastMode();
    assert.deepEqual(second.commands, [{ type: 'set_fast_mode', enabled: true }]);
    second.client.emit('event', { type: 'fast_mode_changed', enabled: true });
    await restoring;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('rejected and interrupted Fast mode changes retain the previous preference', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-fast-mode-'));
  try {
    const store = new SettingsStore(dir);
    store.setLastFastMode(false);
    const { instance, client } = runtime(store);

    const rejected = instance.dispatchCommand({ type: 'set_fast_mode', enabled: true });
    client.emit('event', {
      type: 'error',
      kind: { type: 'rejected' },
      message: 'set_fast_mode failed: unavailable for this model',
    });
    await assert.rejects(rejected, /unavailable for this model/);
    assert.equal(new SettingsStore(dir).getLastFastMode(), false);

    const interrupted = instance.dispatchCommand({ type: 'set_fast_mode', enabled: true });
    (instance as any).setState({ status: 'disconnected', reason: 'closed' });
    await assert.rejects(interrupted, /interrupted/);
    assert.equal(new SettingsStore(dir).getLastFastMode(), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
