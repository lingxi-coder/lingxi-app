import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { SessionRuntime } from '../src/main/bridge.js';

const session = { access_token: 'test-private-token', expires_at: 123, fedramp: false };
const command = { type: 'run_slash_command', raw: '/fusion compare approaches' };
function fixture(resolveOpenAiOAuth = async () => session as typeof session | undefined) {
  const calls: string[] = [];
  const failures: Array<{ raw: string; error: string }> = [];
  const recordFailure = (value: any) => {
    if (value.type !== 'fusion_admin' || value.command.action !== 'record_failure') return false;
    failures.push(JSON.parse(value.command.payload_json));
    calls.push('record');
    return true;
  };
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    resolveOpenAiOAuth,
    resolveProviderCredential: async id => { calls.push(`key:${id}`); return undefined; },
    onFirstPromptSent: () => { calls.push('commit'); },
  });
  const state = runtime as any;
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (value: any) => {
      if (recordFailure(value)) return;
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
  state.credentialRoutingSettings = {};
  state.resolveFusionStrategy = async () => ({
    reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aabbcc',
    config: { enabled: true, panelModels: [{ profile: 'openai-chatgpt', model: 'gpt-5.6-sol' }] },
  });
  runtime.restart = async () => { throw new Error('Fusion must preserve the running engine and its background work'); };
  runtime.serveCodexToken = async value => { assert.equal(value, session); calls.push('oauth'); };
  return { runtime, state, calls, failures, recordFailure };
}

test('Fusion serves shared Codex authentication without restarting or changing the conversation model', async () => {
  const { runtime, state, calls } = fixture();
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['oauth', 'old:run_slash_command', 'commit']);
  assert.equal(state.selectedModelReference, 'deepseek/deepseek-flash');
  assert.equal(runtime.turnActive, false);
});

test('Fusion does not restart or dispatch after cancellation during OAuth retrieval', async () => {
  let resolve!: (value: typeof session) => void;
  const { runtime, calls } = fixture(() => new Promise(done => { resolve = done; }));
  const pending = runtime.dispatchCommand(command);
  await new Promise(done => setImmediate(done));
  runtime.cancelTurn(undefined);
  resolve(session);
  await assert.rejects(pending, /interrupted/);
  assert.deepEqual(calls, ['cancel']);
});

test('Fusion cannot serve credentials or dispatch after an external lifecycle change', async () => {
  let resolve!: (value: typeof session) => void;
  const { runtime, state, calls } = fixture(() => new Promise(done => { resolve = done; }));
  const pending = runtime.dispatchCommand(command);
  await new Promise(done => setImmediate(done));
  ++state.generation;
  resolve(session);
  await assert.rejects(pending, /interrupted/);
  assert.deepEqual(calls, []);
});

test('Fusion records missing OAuth without starting a run', async () => {
  const { runtime, calls, failures } = fixture(async () => undefined);
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['record', 'commit']);
  assert.equal(failures[0]?.raw, command.raw);
  assert.match(failures[0]!.error, /authentication is unavailable/);
});

test('Fusion setup and publication retry bypass OAuth activation and commit their persisted messages', async () => {
  const { runtime, calls } = fixture(async () => { throw new Error('must not load'); });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion setup' });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --retry-publication saved' });
  assert.deepEqual(calls, ['old:run_slash_command', 'commit', 'old:run_slash_command']);
});

test('Fusion shared OAuth leaves active background work on the existing engine', async () => {
  const { runtime, calls } = fixture();
  Object.defineProperty(runtime, 'hasActiveAgents', { get: () => true });
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['oauth', 'old:run_slash_command', 'commit']);
});

test('Fusion resolves the chosen scoped strategy before credentials and fences dispatch without rewriting user text', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('unselected OAuth provider must not be loaded'); });
  delete state.resolveFusionStrategy;
  const client = state.client;
  const sent: any[] = [];
  client.sendCommand = (value: any) => {
    sent.push(value);
    if (value.type === 'fusion_admin') {
      assert.deepEqual(JSON.parse(value.command.payload_json), { strategy: { scope: 'project', id: 'review' } });
      queueMicrotask(() => client.emit('event', {
        type: 'fusion_strategy_resolved', operation_id: value.command.operation_id,
        selection_json: JSON.stringify({ reference: { scope: 'project', id: 'review' }, name: 'Review', revision: 'abcd',
          config: { enabled: true, panelModels: [{ profile: 'deepseek', model: 'deepseek-flash' }] } }),
      }));
    }
  };
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --strategy project:review inspect' });
  assert.deepEqual(calls, ['key:deepseek', 'commit']);
  assert.equal(sent.at(-1).raw, '/fusion --strategy-revision abcd --strategy project:review inspect');
  assert.equal(client.listenerCount('event'), 0);
});

test('missing selected Fusion strategy loads no credentials and leaves no resolution listener', async () => {
  const { runtime, state, calls, failures, recordFailure } = fixture();
  delete state.resolveFusionStrategy;
  const client = state.client;
  client.sendCommand = (value: any) => { if (!recordFailure(value)) queueMicrotask(() => client.emit('event', {
    type: 'fusion_strategy_resolved', operation_id: value.command.operation_id, error: 'Fusion strategy no longer exists',
  })); };
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['record', 'commit']);
  assert.deepEqual(failures, [{ raw: command.raw, error: 'Fusion strategy no longer exists' }]);
  assert.equal(client.listenerCount('event'), 0);
  assert.equal(runtime.turnActive, false);
});

test('cancellation during server strategy resolution cannot hydrate or dispatch the result', async () => {
  const { runtime, state, calls } = fixture();
  delete state.resolveFusionStrategy;
  const client = state.client;
  let operationId = 0;
  client.sendCommand = (value: any) => { operationId = value.command.operation_id; };
  const pending = runtime.dispatchCommand(command);
  runtime.cancelTurn(undefined);
  client.emit('event', { type: 'fusion_strategy_resolved', operation_id: operationId,
    selection_json: JSON.stringify({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'abcd', config: {} }) });
  await assert.rejects(pending, /interrupted/);
  assert.deepEqual(calls, ['cancel']);
  assert.equal(client.listenerCount('event'), 0);
});


test('Fusion fast preset never activates OAuth for a configured tail model that will not run', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('unused tail OAuth must not load'); });
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    panelModels: [{ profile: 'deepseek', model: 'a' }, { profile: 'kimi', model: 'b' }, { profile: 'openai-chatgpt', model: 'c' }], analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --fast inspect' });
  assert.deepEqual(calls, ['key:deepseek', 'key:kimi', 'old:run_slash_command', 'commit']);
});

test('Fusion explicit models hydrate their own profiles plus the analyst, excluding the stored panel roster', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('replaced roster OAuth must not load'); });
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    panelModels: [{ profile: 'openai-chatgpt', model: 'stored' }], analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models deepseek:a,kimi:b inspect' });
  assert.deepEqual(calls, ['key:deepseek', 'key:kimi', 'old:run_slash_command', 'commit']);
});

test('Fusion explicit OAuth model activates authentication even when it is outside the stored roster', async () => {
  const { runtime, state, calls } = fixture();
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    panelModels: [{ profile: 'kimi', model: 'stored' }], analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models openai-chatgpt:chosen,deepseek:a inspect' });
  assert.deepEqual(calls, ['oauth', 'key:deepseek', 'old:run_slash_command', 'commit']);
});

test('Fusion never loads an ordinary fallback OAuth provider for an exact configured pair', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('unused fallback OAuth must not load'); });
  state.credentialRoutingSettings = { routing: { fallback: { a: ['openai-chatgpt/unused'] } } };
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    panelModels: [{ profile: ' deepseek ', model: ' a ' }, { profile: 'kimi', model: 'b' }], analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['key:deepseek', 'key:kimi', 'old:run_slash_command', 'commit']);
});

test('bare Fusion model ids hydrate the captured conversation profile and ignore unrelated aliases', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('unrelated alias OAuth must not load'); });
  state.credentialRoutingSettings = { routing: { aliases: { a: 'openai-chatgpt/unused', b: 'openai/other' } } };
  let catalogRequests = 0;
  runtime.scheduledModelCatalog = async () => { catalogRequests++; return { type: 'model_list', models: [], current: 'kimi/session-model' }; };
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    panelModels: [{ profile: 'openai-chatgpt', model: 'stored' }], analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models a,b inspect' });
  assert.deepEqual(calls, ['key:kimi', 'key:deepseek', 'old:run_slash_command', 'commit']);
  assert.equal(catalogRequests, 2, 'current profile is confirmed before and after credential preparation');
});

test('a changed conversation profile cannot dispatch a bare Fusion model roster with stale credentials', async () => {
  const { runtime, state, calls, failures } = fixture();
  let catalogRequests = 0;
  runtime.scheduledModelCatalog = async () => ({ type: 'model_list', models: [], current: ++catalogRequests === 1 ? 'kimi/current' : 'openai/changed' });
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models a,b inspect' });
  assert.deepEqual(calls, ['key:kimi', 'key:deepseek', 'record', 'commit']);
  assert.match(failures[0]!.error, /conversation model changed/);
  assert.equal(state.pendingPromptHydrations.size, 0);
});

test('bare Fusion model profile remains valid while shared OAuth is served', async () => {
  const { runtime, state, calls } = fixture();
  runtime.scheduledModelCatalog = async () => ({ type: 'model_list', models: [], current: 'deepseek/deepseek-flash' });
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {
    analystModel: { profile: 'deepseek', model: 'judge' },
  } });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models openai-chatgpt:chosen,a inspect' });
  assert.deepEqual(calls, ['oauth', 'key:deepseek', 'old:run_slash_command', 'commit']);
});

test('Fusion malformed flags and credential failures use record-only dispatch with original text', async () => {
  for (const raw of ['/fusion --max-panel 9 inspect', '/fusion --strategy user:review  inspect\nthese']) {
    const { runtime, state, calls, failures } = fixture();
    state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa',
      config: { panelModels: [{ profile: 'deepseek', model: 'a' }] } });
    state.opts.resolveProviderCredential = async () => { throw new Error('Selected credential is unavailable'); };
    await runtime.dispatchCommand({ type: 'run_slash_command', raw });
    assert.deepEqual(calls, ['record', 'commit']);
    assert.equal(failures.length, 1);
    assert.equal(failures[0]!.raw, raw);
    assert.match(failures[0]!.error, /2\.\.=8|Selected credential is unavailable/);
  }
});

test('uncertain slash dispatch failures never trigger a record-only retry', async () => {
  const { runtime, state, failures } = fixture();
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {} });
  state.client.sendCommand = () => { throw new Error('Connection failed during dispatch'); };
  await assert.rejects(runtime.dispatchCommand(command), /Connection failed during dispatch/);
  assert.deepEqual(failures, []);
});

test('bare Fusion models retain a custom parent profile named claude', async () => {
  const { runtime, state, calls } = fixture();
  state.credentialRoutingSettings = { providers: { claude: { models: ['current', 'a', 'b'] } } };
  runtime.scheduledModelCatalog = async () => ({ type: 'model_list', models: [], current: 'claude/current' });
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {} });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --models a,b inspect' });
  assert.deepEqual(calls, ['key:claude', 'old:run_slash_command', 'commit']);
});

test('Fusion holds credential preparation ownership until slash dispatch is accepted', async () => {
  const { runtime, state } = fixture();
  state.resolveFusionStrategy = async () => ({ reference: { scope: 'user', id: 'review' }, name: 'Review', revision: 'aa', config: {} });
  const send = state.client.sendCommand;
  let dispatched = false;
  state.client.sendCommand = (value: any) => {
    if (value.type === 'run_slash_command') {
      assert.equal(state.pendingPromptHydrations.size, 1);
      dispatched = true;
    }
    send(value);
  };
  await runtime.dispatchCommand(command);
  assert.equal(dispatched, true);
  assert.equal(state.pendingPromptHydrations.size, 0);
});
