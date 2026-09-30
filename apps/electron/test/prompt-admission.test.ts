import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';

function fixture(resolveProviderCredential?: () => Promise<string | undefined>) {
  const workspace = '/prompt-admission';
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace, trusted: true }), resolveProviderCredential });
  const prompts: Array<{ text: string; turnId?: number }> = [];
  const cancels: Array<number | undefined> = [];
  const client = new EventEmitter() as EventEmitter & { sendPrompt(text: string, options: { turnId?: number }): void; cancel(id?: number): void; close(): void };
  client.sendPrompt = (text, options) => prompts.push({ text, turnId: options.turnId });
  client.cancel = (id) => cancels.push(id);
  client.close = () => undefined;
  const internal = runtime as any;
  internal.activeWorkspace = workspace; internal.activeWorkspaceTrusted = true; internal.client = client;
  internal.selectedModelReference = 'anthropic/model'; internal.credentialRoutingSettings = {};
  internal.wireClient(client, internal.generation);
  internal.setState({ status: 'connected' });
  return { runtime, internal, client, prompts, cancels };
}

test('prompt admission preserves its renderer turn ID before turn_started arrives', async (t) => {
  const h = fixture(); t.after(() => h.runtime.dispose());
  h.runtime.sendPrompt('voice prompt', [], 12345);
  assert.deepEqual(h.prompts, [{ text: 'voice prompt', turnId: 12345 }]);
  assert.equal(h.internal.activeTurnId, 12345);
  h.runtime.cancelTurn(12345);
  assert.deepEqual(h.cancels, [12345]);
  assert.equal(h.internal.cancellingTurn, true);
  assert.throws(() => h.runtime.sendPrompt('invalid identity', [], -1), /turn id/i);
});

test('targeted cancel during credential hydration drops only the matching prompt', async (t) => {
  let complete!: (value: string | undefined) => void;
  const credentials = new Promise<string | undefined>((resolve) => { complete = resolve; });
  const h = fixture(() => credentials); t.after(() => h.runtime.dispose());
  const first = Promise.resolve(h.runtime.sendPrompt('cancelled voice prompt', [], 123));
  const rejected = assert.rejects(first, /interrupted/);
  const second = Promise.resolve(h.runtime.sendPrompt('other admitted prompt', [], 456));
  h.runtime.cancelTurn(123);
  complete(undefined);
  await rejected;
  await second;
  assert.deepEqual(h.prompts, [{ text: 'other admitted prompt', turnId: 456 }]);
  assert.deepEqual(h.cancels, [123]);
  assert.equal(h.internal.activeTurnId, 456);
  assert.equal(h.internal.cancellingTurn, false);
});

test('a stale targeted cancel does not clear another pending prompt hydration', async (t) => {
  let complete!: (value: string | undefined) => void;
  const credentials = new Promise<string | undefined>((resolve) => { complete = resolve; });
  const h = fixture(() => credentials); t.after(() => h.runtime.dispose());
  const prompt = Promise.resolve(h.runtime.sendPrompt('current voice prompt', [], 456));
  h.runtime.cancelTurn(123);
  assert.equal(h.runtime.turnActive, true);
  complete(undefined);
  await prompt;
  assert.deepEqual(h.prompts, [{ text: 'current voice prompt', turnId: 456 }]);
});

test('manager IPC forwards the captured session and admission turn identity', async (t) => {
  const { SessionRuntimeManager } = await import('../src/main/sessionRuntimeManager.js');
  const { ipcMain, CH_SEND_PROMPT, CH_CANCEL } = await import('../src/main/bridgeIpc.js');
  const handles = new Map<string, (...args: any[]) => any>();
  const previousHandle = ipcMain.handle;
  const previousRemove = ipcMain.removeHandler;
  ipcMain.handle = ((channel: string, handler: (...args: any[]) => any) => { handles.set(channel, handler); }) as any;
  ipcMain.removeHandler = ((channel: string) => { handles.delete(channel); }) as any;
  const manager = new SessionRuntimeManager({ launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(async () => { await manager.dispose(); ipcMain.handle = previousHandle; ipcMain.removeHandler = previousRemove; });
  const ref = { projectPath: '/prompt-admission', sessionId: '11111111-2222-4333-8444-555555555555' };
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  const calls: any[] = [];
  internal.activeWorkspace = ref.projectPath; internal.activeWorkspaceTrusted = true;
  internal.client = { sendPrompt: (text: string, options: unknown) => calls.push({ text, options }), cancel: (id: number) => calls.push({ cancel: id }), close: () => undefined, removeAllListeners: () => undefined };
  internal.setState({ status: 'connected' });
  const window = new EventEmitter() as any;
  window.mainFrame = { url: 'app://desktop/index.html' }; window.isDestroyed = () => false; window.send = () => undefined;
  manager.registerWindow(window, window.mainFrame.url);
  manager.registerIpc();
  const event = { sender: window, senderFrame: window.mainFrame };
  await handles.get(CH_SEND_PROMPT)!(event, ref.sessionId, 'owned voice prompt', [], 987);
  handles.get(CH_CANCEL)!(event, ref.sessionId, 987);
  assert.deepEqual(calls, [{ text: 'owned voice prompt', options: { images: [], turnId: 987 } }, { cancel: 987 }]);
  assert.equal(internal.cancellingTurn, true);
});
