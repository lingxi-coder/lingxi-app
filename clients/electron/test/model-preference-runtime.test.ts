import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge';
import { SettingsStore } from '../src/main/settings';
import type { ClientCommand } from '@lingxi/bridge-client';
function runtime(store: SettingsStore) {
  const commands: ClientCommand[] = [];
  const observed: string[] = [];
  const client = Object.assign(new EventEmitter(), { sendCommand: (command: ClientCommand) => commands.push(command) });
  const instance = new SessionRuntime({
    projectPath: '/workspace', sessionId: '11111111-2222-4333-8444-555555555555',
    launchConfig: () => ({ workspace: '/workspace', trusted: true, model: store.getPublic().model }),
    onModelSelected: model => store.setLastModel(model),
    getSavedModel: () => store.getPublic().model,
    onModelChanged: model => observed.push(model),
  });
  Object.assign(instance, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true });
  (instance as any).wireClient(client, 0);
  return { instance, client, commands, observed };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
test('only confirmed user model selections replace the restart default; history still updates observers', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client, commands, observed } = runtime(store);
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    assert.equal(store.getPublic().model, 'old-model');
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    assert.deepEqual(commands, [{ type: 'set_model', model: 'new-model' }]);
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'unrelated-model' });
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await selection;
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    assert.equal(new SettingsStore(dir).getPublic().model, 'new-model');
    assert.deepEqual(observed, ['historical-model', 'unrelated-model', 'new-model', 'historical-model']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('failed and interrupted model switches retain the saved choice', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client } = runtime(store);
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'invalid-model' });
    await tick();
    client.emit('event', { type: 'error', kind: { type: 'rejected' }, message: 'unknown model' });
    await assert.rejects(selection, /Model switch failed/);
    const interrupted = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    (instance as any).setState({ status: 'disconnected', reason: 'closed' });
    await assert.rejects(interrupted, /interrupted/);
    assert.equal(new SettingsStore(dir).getPublic().model, 'old-model');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('write failure is reported and rolls back the restart default', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const before = readFileSync(store.settingsPath, 'utf8');
    mkdirSync(`${store.settingsPath}.tmp`);
    const { instance, client, observed } = runtime(store);
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await assert.rejects(selection, /Could not save model/);
    assert.equal(store.getPublic().model, 'old-model');
    assert.equal(readFileSync(store.settingsPath, 'utf8'), before);
    assert.deepEqual(observed, ['new-model']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('resume reapplies the last selection over an old snapshot even without a subsequent message', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    new SettingsStore(dir).setLastModel('latest-model');
    const store = new SettingsStore(dir);
    const { instance, client, commands } = runtime(store);
    const resumed = instance.resumeOwnedSession();
    client.emit('event', { type: 'session_resumed', session_id: instance.sessionId, mode: 'default', messages: [] });
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    await tick();
    assert.deepEqual(commands.slice(0, 2).map(command => command.type), ['resume_session', 'set_model']);
    assert.deepEqual(commands[1], { type: 'set_model', model: 'latest-model' });
    client.emit('event', { type: 'model_changed', model: 'latest-model' });
    await resumed;
    assert.equal(new SettingsStore(dir).getPublic().model, 'latest-model');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('a now-unavailable saved model reports restore failure without breaking resume or overwriting it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('unavailable-model');
    const { instance, client } = runtime(store);
    const events: unknown[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    const resumed = instance.resumeOwnedSession();
    client.emit('event', { type: 'session_resumed', session_id: instance.sessionId, mode: 'default', messages: [] });
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    await tick();
    client.emit('event', { type: 'error', kind: { type: 'rejected' }, message: 'unknown model' });
    await resumed;
    assert.equal(store.getPublic().model, 'unavailable-model');
    assert.ok(events.some(event => JSON.stringify(event).includes('Could not restore model')));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('explicit /model persists only on confirmation, including reselecting the current model', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client, commands } = runtime(store);
    const events: any[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    client.emit('event', { type: 'model_changed', model: 'current-model' });
    const selecting = instance.dispatchCommand({ type: 'run_slash_command', raw: '/model current-model ' });
    await tick();
    assert.deepEqual(commands, [{ type: 'run_slash_command', raw: '/model current-model ' }]);
    assert.equal(store.getPublic().model, 'old-model');
    assert.ok(!events.some(event => event.type === 'slash_command_result'));
    await assert.rejects(instance.dispatchCommand({ type: 'run_slash_command', raw: '/model' }), /already in progress/);
    client.emit('event', { type: 'slash_command_result', display: 'Switched', is_error: false });
    client.emit('event', { type: 'model_list', current: 'provider/current-model', models: ['provider/current-model'] });
    await selecting;
    assert.equal(new SettingsStore(dir).getPublic().model, 'provider/current-model');
    assert.ok(events.some(event => event.type === 'slash_command_result' && !event.is_error));
    await instance.dispatchCommand({ type: 'run_slash_command', raw: '/model' });
    assert.deepEqual(commands.at(-1), { type: 'run_slash_command', raw: '/model' });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('rejected explicit /model never saves or reports success', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client } = runtime(store);
    const events: any[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    const selecting = instance.dispatchCommand({ type: 'run_slash_command', raw: '/model forbidden-model' });
    await tick();
    client.emit('event', { type: 'slash_command_result', display: 'Could not switch model: blocked by hook', is_error: false });
    client.emit('event', { type: 'model_list', current: 'old-model', models: ['old-model'] });
    await assert.rejects(selecting, /not confirmed/);
    assert.equal(new SettingsStore(dir).getPublic().model, 'old-model');

  } finally { rmSync(dir, { recursive: true, force: true }); }
});
