import { test } from 'node:test';
import assert from 'node:assert/strict';
import { saveProviderSettings } from '../src/renderer/bridge/providerSettingsSave';

test('provider settings save waits for matching persisted layer and ignores another session', async () => {
  let listener: (event: any) => void = () => undefined;
  let detached = false;
  const host = {
    onEvent: (callback: typeof listener) => { listener = callback; return () => { detached = true; }; },
    command: async () => undefined,
  };
  let completed = false;
  const pending = saveProviderSettings(host, 'session', 'user', { providers: { custom: { type: 'openai', models: [], apiKeyEnv: undefined } } }).then(() => { completed = true; });
  const event = { type: 'settings_snapshot', layers_json: JSON.stringify({ user: { providers: { custom: { models: [], type: 'openai' } } } }) };
  listener({ sessionId: 'other', event });
  listener({ sessionId: 'session', event: { ...event, layers_json: '{}' } });
  await Promise.resolve();
  assert.equal(completed, false);
  listener({ sessionId: 'session', event });
  await pending;
  assert.equal(detached, true);
});

test('provider settings save propagates engine and transport errors', async () => {
  let listener: (event: any) => void = () => undefined;
  const host = { onEvent: (callback: typeof listener) => { listener = callback; return () => undefined; }, command: async () => undefined };
  const pending = saveProviderSettings(host, 'session', 'project', { providers: {} });
  listener({ sessionId: 'session', event: { type: 'error', message: 'settings file is read-only' } });
  await assert.rejects(pending, /could not save provider settings/);
  await assert.rejects(saveProviderSettings({ ...host, command: async () => { throw new Error('disconnected'); } }, 'session', 'project', {}), /disconnected/);
});

test('provider settings save aborts on session navigation and disconnect and removes listeners', async () => {
  let stateListener: (event: any) => void = () => undefined;
  let removed = 0;
  const host = {
    onEvent: () => () => { removed += 1; },
    onConnectionStateChanged: (callback: typeof stateListener) => { stateListener = callback; return () => { removed += 1; }; },
    command: async () => undefined,
  };
  const controller = new AbortController();
  const navigation = saveProviderSettings(host, 'session', 'user', {}, controller.signal);
  controller.abort();
  await assert.rejects(navigation, /interrupted/);
  assert.equal(removed, 2);
  const disconnect = saveProviderSettings(host, 'session', 'user', {});
  stateListener({ sessionId: 'session', event: { status: 'disconnected' } });
  await assert.rejects(disconnect, /interrupted/);
  assert.equal(removed, 4);
});
