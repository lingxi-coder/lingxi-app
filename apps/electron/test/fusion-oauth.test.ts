import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';

const session = { access_token: 'test-private-token', expires_at: 123, fedramp: false };
const command = { type: 'run_slash_command', raw: '/fusion compare approaches' };
function fixture(resolveOpenAiOAuth = async () => session as typeof session | undefined) {
  const calls: string[] = [];
  const runtime = new SessionRuntime({
    registerIpc: false,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    resolveOpenAiOAuth,
    resolveProviderCredential: async id => { calls.push(`key:${id}`); return undefined; },
    onFirstPromptSent: () => { calls.push('commit'); },
  });
  const state = runtime as any;
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (value: any) => {
      if (value.type === 'task_list') {
        queueMicrotask(() => client.emit('event', { type: 'task_list_complete', request_id: value.request_id, active_count: 0 }));
      } else calls.push(`old:${value.type}`);
    },
    cancel: () => calls.push('cancel'),
  });
  state.activeWorkspace = '/workspace';
  state.activeWorkspaceTrusted = true;
  state.selectedModelReference = 'deepseek/deepseek-flash';
  state.client = client;
  state.credentialRoutingSettings = { fusion: { panelModels: [{ profile: 'openai-chatgpt', model: 'gpt-5.6-sol' }] } };
  runtime.restart = async beforeRestart => {
    ++state.fusionLifecycleEpoch;
    await beforeRestart?.();
    assert.equal(state.launchOAuthOverride, session);
    assert.equal(state.launchOAuthModel, 'deepseek/deepseek-flash');
    calls.push('restart');
    ++state.generation;
    state.pendingPromptHydrations.clear();
    state.client = { sendCommand: (value: any) => calls.push(`new:${value.type}`) };
    state.openAiOAuthActive = true;
  };
  runtime.restoreOwnedSessionIfNeeded = async () => { calls.push('restore'); };
  return { runtime, state, calls };
}

test('Fusion activates Codex OAuth and restores history without changing primary model or routing OAuth through API keys', async () => {
  const { runtime, state, calls } = fixture();
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['restart', 'restore', 'new:run_slash_command', 'commit']);
  assert.equal(state.launchOAuthOverride, undefined);
  assert.equal(state.launchOAuthModel, undefined);
  assert.equal(runtime.turnActive, false);
  await runtime.dispatchCommand(command);
  assert.equal(calls.filter(value => value === 'restart').length, 1);
});

test('Fusion does not restart or dispatch after cancellation during OAuth retrieval', async () => {
  let resolve!: (value: typeof session) => void;
  const { runtime, calls } = fixture(() => new Promise(done => { resolve = done; }));
  const pending = runtime.dispatchCommand(command);
  await Promise.resolve();
  runtime.cancelTurn(undefined);
  resolve(session);
  await assert.rejects(pending, /interrupted/);
  assert.deepEqual(calls, ['cancel']);
});

test('Fusion does not dispatch after an external lifecycle change during session restoration', async () => {
  const { runtime, state, calls } = fixture();
  runtime.restoreOwnedSessionIfNeeded = async () => { ++state.fusionLifecycleEpoch; };
  await assert.rejects(runtime.dispatchCommand(command), /interrupted/);
  assert.deepEqual(calls, ['restart']);
});

test('Fusion reports missing OAuth without sending or committing a command', async () => {
  const { runtime, calls } = fixture(async () => undefined);
  await assert.rejects(runtime.dispatchCommand(command), /authentication is unavailable/);
  assert.deepEqual(calls, []);
});

test('Fusion setup and publication retry bypass OAuth activation and commit their persisted messages', async () => {
  const { runtime, calls } = fixture(async () => { throw new Error('must not load'); });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion setup' });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --retry-publication saved' });
  assert.deepEqual(calls, ['old:run_slash_command', 'commit', 'old:run_slash_command']);
});

test('Fusion refuses OAuth restart while background agents are active', async () => {
  const { runtime, calls } = fixture();
  Object.defineProperty(runtime, 'hasActiveAgents', { get: () => true });
  await assert.rejects(runtime.dispatchCommand(command), /Wait for active work/);
  assert.deepEqual(calls, []);
});

test('Fusion checks registry tasks before restarting and leaves active Fusion or shell work intact', async () => {
  const { runtime, state, calls } = fixture();
  state.client.sendCommand = (value: any) => {
    assert.equal(value.type, 'task_list');
    queueMicrotask(() => state.client.emit('event', {
      type: 'task_list_complete', request_id: value.request_id, active_count: 1,
    }));
  };
  await assert.rejects(runtime.dispatchCommand(command), /Wait for background tasks/);
  assert.deepEqual(calls, []);
  assert.equal(state.client.listenerCount('event'), 0, 'query listener is released');
});

test('failed task checks do not authorize an OAuth restart', async () => {
  const { runtime, state, calls } = fixture();
  state.client.sendCommand = (value: any) => {
    queueMicrotask(() => state.client.emit('event', {
      type: 'task_list_complete', request_id: value.request_id, active_count: 0, error: 'registry unavailable',
    }));
  };
  await assert.rejects(runtime.dispatchCommand(command), /Could not check background work/);
  assert.deepEqual(calls, []);
});

test('taking OAuth ownership cannot stop another chat with active agents or registry tasks', async () => {
  for (const agents of [true, false]) {
    const manager = new SessionRuntimeManager({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
    let stopped = false;
    (manager as any).runtimes.set('other', {
      sessionId: 'other', hasOpenAiOAuth: true, isStarting: false, turnActive: false,
      hasActiveAgents: agents, pendingInteractions: 0, connectionState: { status: 'connected' },
      assertNoBackgroundTasks: async () => { throw new Error('Wait for background tasks'); },
      stop: async () => { stopped = true; },
    });
    await assert.rejects((manager as any).stopOtherCodexRuntimes('current'), /Wait/);
    assert.equal(stopped, false);
  }
});
