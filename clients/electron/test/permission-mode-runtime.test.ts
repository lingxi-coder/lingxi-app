import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge.js';
import { SettingsStore } from '../src/main/settings';
import type { ClientCommand, PermissionModeId } from '@lingxi/bridge-client';
function runtime(store: SettingsStore) {
  const commands: ClientCommand[] = [];
  const client = Object.assign(new EventEmitter(), { sendCommand: (command: ClientCommand) => { commands.push(command); } });
  const instance = new SessionRuntime({
    projectPath: '/workspace', sessionId: '11111111-2222-4333-8444-555555555555',
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    getSavedPermissionMode: () => store.getLastPermissionMode(),
    onPermissionModeSelected: mode => store.setLastPermissionMode(mode),
    confirmBypassPermissions: async () => store.getBypassPermissionsAccepted(),
  });
  Object.assign(instance, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true });
  (instance as any).wireClient(client, 0);
  return { instance, client, commands };
}
function acknowledge(client: EventEmitter, mode: PermissionModeId) {
  client.emit('event', { type: 'permission_mode_changed', mode });
}
test('only an acknowledged selection persists, and a new runtime restores it after restart', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-permission-runtime-'));
  try {
    const store = new SettingsStore(dir);
    const first = runtime(store);
    const pending = first.instance.dispatchCommand({ type: 'set_permission_mode', mode: 'acceptEdits' });
    assert.equal(store.getLastPermissionMode(), undefined);
    acknowledge(first.client, 'default');
    assert.equal(store.getLastPermissionMode(), undefined);
    acknowledge(first.client, 'acceptEdits');
    await pending;
    const second = runtime(new SettingsStore(dir));
    const restoring = (second.instance as any).restorePermissionMode();
    assert.deepEqual(second.commands, [{ type: 'set_permission_mode', mode: 'acceptEdits' }]);
    acknowledge(second.client, 'acceptEdits');
    await restoring;
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('rejected and interrupted choices retain the last saved mode', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-permission-runtime-'));
  try {
    const store = new SettingsStore(dir); store.setLastPermissionMode('plan');
    const { instance, client } = runtime(store);
    const rejected = instance.dispatchCommand({ type: 'set_permission_mode', mode: 'auto' });
    client.emit('event', { type: 'error', kind: { type: 'rejected' }, message: 'set_permission_mode failed: managed settings' });
    await assert.rejects(rejected, /managed settings/);
    assert.equal(new SettingsStore(dir).getLastPermissionMode(), 'plan');
    const interrupted = instance.dispatchCommand({ type: 'set_permission_mode', mode: 'default' });
    (instance as any).setState({ status: 'disconnected', reason: 'closed' });
    await assert.rejects(interrupted, /interrupted/);
    assert.equal(new SettingsStore(dir).getLastPermissionMode(), 'plan');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('history hydration reapplies the saved mode after restoring an old plan state', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-permission-runtime-'));
  try {
    const store = new SettingsStore(dir); store.setLastPermissionMode('acceptEdits');
    const { instance, client, commands } = runtime(store);
    const resumed = instance.resumeOwnedSession();
    client.emit('event', { type: 'session_resumed', session_id: instance.sessionId, mode: 'plan', messages: [] });
    client.emit('event', { type: 'model_changed', model: 'model' });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(commands.slice(0, 2).map(command => command.type), ['resume_session', 'set_permission_mode']);
    acknowledge(client, 'acceptEdits');
    await resumed;
    assert.equal(store.getLastPermissionMode(), 'acceptEdits');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('policy rejection during restore leaves the runtime usable and reports the failure', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-permission-runtime-'));
  try {
    const store = new SettingsStore(dir); store.setLastPermissionMode('auto');
    const { instance, client } = runtime(store);
    const events: unknown[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    const restored = (instance as any).restorePermissionMode();
    client.emit('event', { type: 'error', kind: { type: 'rejected' }, message: 'set_permission_mode failed: auto unavailable' });
    await restored;
    assert.ok(events.some(event => JSON.stringify(event).includes('Could not restore permission mode')));
    assert.equal(store.getLastPermissionMode(), 'auto');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('fresh connection restores saved permission before publishing connected', async () => {
  const { BridgeClient } = await import('@lingxi/bridge-client');
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-permission-runtime-'));
  const connect = BridgeClient.prototype.connect;
  const send = BridgeClient.prototype.sendCommand;
  try {
    const store = new SettingsStore(dir); store.setLastPermissionMode('plan');
    const { instance } = runtime(store);
    let modeRequested = false;
    let confirm: (() => void) | undefined;
    BridgeClient.prototype.connect = async function () {
      return { server_name: 'test', protocol_version: '1', capabilities: { client_protocol_version: '1' } } as any;
    };
    BridgeClient.prototype.sendCommand = function (command) {
      if (command.type === 'set_permission_mode') {
        modeRequested = true;
        assert.equal(command.mode, 'plan');
        assert.equal(instance.connectionState.status, 'connecting');
        confirm = () => this.emit('event', { type: 'permission_mode_changed', mode: 'plan' });
      }
    };
    (instance as any).refreshProviderCredentials = async () => [];
    const connecting = (instance as any).connectBridgeClient('/unused', 0);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(modeRequested, true);
    assert.equal(instance.connectionState.status, 'connecting');
    confirm!();
    await connecting;
    assert.equal(instance.connectionState.status, 'connected');
  } finally {
    BridgeClient.prototype.connect = connect;
    BridgeClient.prototype.sendCommand = send;
    rmSync(dir, { recursive: true, force: true });
  }
});
