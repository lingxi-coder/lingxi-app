import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { SessionRuntime } from '../src/main/bridge.js';

function fixture() {
  const commands: any[] = [], calls: string[] = [];
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    resolveProviderAccounts: async () => undefined, onFirstPromptSent: () => calls.push('commit') });
  const state = runtime as any;
  const client = Object.assign(new EventEmitter(), { sendCommand: (command: any) => { commands.push(command); }, cancel: () => {} });
  state.activeWorkspace = '/workspace'; state.activeWorkspaceTrusted = true; state.client = client;
  state.selectedModelReference = 'deepseek/deepseek-flash';
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aabbcc', config: { enabled: true } });
  runtime.refreshProviderAccounts = async () => { calls.push('accounts'); };
  runtime.restart = async () => { throw new Error('must preserve engine'); };
  return { runtime, state, commands, calls };
}
const command = { type: 'run_slash_command', raw: '/fusion compare approaches', images: [] };
test('Fusion injects the canonical account pool before dispatch without changing the parent model', async () => {
  const f = fixture(); await f.runtime.dispatchCommand(command);
  assert.deepEqual(f.calls, ['accounts', 'commit']);
  assert.equal(f.commands.length, 1); assert.equal(f.commands[0].type, 'run_slash_command');
  assert.match(f.commands[0].raw, /compare approaches/); assert.equal(f.state.selectedModelReference, 'deepseek/deepseek-flash');
});
test('Fusion cancellation fences delayed canonical account injection before dispatch', async () => {
  const f = fixture(); let release!: () => void;
  const waiting = new Promise<void>(resolve => { release = resolve; });
  f.runtime.refreshProviderAccounts = async () => waiting;
  const pending = f.runtime.dispatchCommand(command); await new Promise(resolve => setImmediate(resolve));
  f.state.pendingPromptHydrations.clear(); release();
  await assert.rejects(pending, /interrupted/); assert.deepEqual(f.commands, []); assert.deepEqual(f.calls, []);
});
test('Fusion account failure records the original command and never dispatches a model call', async () => {
  const f = fixture(); f.runtime.refreshProviderAccounts = async () => { throw new Error('account storage unavailable'); };
  await f.runtime.dispatchCommand(command);
  assert.equal(f.commands.length, 1); assert.equal(f.commands[0].type, 'fusion_admin');
  assert.equal(f.commands[0].command.action, 'record_failure');
  assert.equal(JSON.parse(f.commands[0].command.payload_json).raw, command.raw);
});
test('Fusion administration does not load model credentials', async () => {
  const f = fixture(); await f.runtime.dispatchCommand({ ...command, raw: '/fusion setup' });
  assert.deepEqual(f.calls, ['commit']); assert.equal(f.commands[0].type, 'run_slash_command');
});
