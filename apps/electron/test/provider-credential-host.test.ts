import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  HostController, CH_PROVIDER_CREDENTIAL_SET, CH_PROVIDER_CREDENTIAL_CLEAR, CH_PROVIDER_CREDENTIALS_GET,
} from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';

function fixture() {
  const handlers = new Map<string, (...args: any[]) => any>();
  const secrets = new Map<string, string>([['deepseek', 'initial']]);
  const cached = new Set(['deepseek']);
  const operations: string[] = [];
  let invalidations = 0;
  let syncError: Error | undefined;
  const runtime = {
    connectionState: { status: 'connected' },
    customProviderIds: [],
    configuredCredentialIds: [] as string[],
    get activeCredentialProviderIds() { return [...cached]; },
  };
  const broker = {
    health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
    listStatus: async () => [...secrets].map(([providerId]) => ({ providerId, configured: true })),
    preview: async (providerId: string) => ({ providerId, configured: secrets.has(providerId), maskedValue: '••••test' }),
    resolve: async (providerId: string) => secrets.get(providerId),
    set: async (providerId: string, value: string) => {
      operations.push('store'); secrets.set(providerId, value);
      return { providerId, configured: true, maskedValue: '••••test' };
    },
    delete: async (providerId: string) => { operations.push('delete'); secrets.delete(providerId); },
  };
  const bridge = {
    registerIpc() {}, registerWindow() {}, get: () => runtime,
    runtimeSummaries: [], hasActiveWork: () => true,
    invalidateLaunchConfigCache: () => { invalidations += 1; },
    refreshCachedProviderCredential: async (id: string, value: string) => {
      operations.push(`set:${value}`); if (syncError) throw syncError; cached.add(id);
    },
    clearCachedProviderCredential: async (id: string) => {
      operations.push('clear'); if (syncError) throw syncError; cached.delete(id);
    },
  };
  const settings = {
    getWorkspace: () => '/workspace',
    getPublic: () => ({ activeSession: { projectPath: '/workspace', sessionId: 'session' } }),
  };
  const host = new HostController(settings as any, bridge as any, new DiagnosticBuffer(), undefined, {
    handle: (channel: string, handler: (...args: any[]) => any) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  } as any, undefined, broker as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const event = { sender, senderFrame: frame };
  return {
    host, broker, secrets, operations,
    set: (value: string, id = 'deepseek') => handlers.get(CH_PROVIDER_CREDENTIAL_SET)!(event, id, value),
    clear: (id = 'deepseek') => handlers.get(CH_PROVIDER_CREDENTIAL_CLEAR)!(event, id),
    status: () => handlers.get(CH_PROVIDER_CREDENTIALS_GET)!(event),
    invalidations: () => invalidations,
    failSync: () => { syncError = new Error('engine ACK failed'); },
    configureCredentials: (ids: string[]) => { runtime.configuredCredentialIds = ids; },
  };
}

test('failed engine acknowledgement rejects a stored key mutation and still invalidates launch material', async () => {
  const f = fixture();
  f.failSync();
  await assert.rejects(f.set('replacement'), /engine ACK failed/);
  assert.equal(f.secrets.get('deepseek'), 'replacement');
  await assert.rejects(f.clear(), /engine ACK failed/);
  assert.equal(f.secrets.has('deepseek'), false);
  assert.equal(f.invalidations(), 2);
  assert.equal((await f.status()).find((row: any) => row.providerId === 'deepseek').runtimeOnly, true,
    'failed clearing reports the remaining engine-only key rather than claiming deletion reached the engine');
});

test('concurrent save and delete retain invocation order through broker persistence and engine acknowledgements', async () => {
  const f = fixture();
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  const set = f.broker.set;
  f.broker.set = async (id, value) => { await pending; return set(id, value); };
  const saving = f.set('replacement');
  const deleting = f.clear();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(f.operations, []);
  release();
  await Promise.all([saving, deleting]);
  assert.deepEqual(f.operations, ['store', 'set:replacement', 'delete', 'clear']);
  assert.equal(f.secrets.has('deepseek'), false);
  assert.equal(f.invalidations(), 2, 'both mutations invalidate startup material');
});

test('a status read started before deletion cannot restore stale configured metadata', async () => {
  const f = fixture();
  let finish!: (rows: Array<{ providerId: string; configured: boolean }>) => void;
  let calls = 0;
  const listStatus = f.broker.listStatus;
  f.broker.listStatus = async () => ++calls === 1
    ? new Promise((resolve) => { finish = resolve; })
    : listStatus();
  const reading = f.status();
  await f.clear();
  finish([{ providerId: 'deepseek', configured: true }]);
  const result = await reading;
  assert.equal(result.find((row: any) => row.providerId === 'deepseek').configured, false);
  assert.equal(calls, 2);
});

test('explicit environment overrides remain consistent after stored key save and deletion without exposing either key', async () => {
  const previous = process.env.DEEPSEEK_API_KEY;
  process.env.DEEPSEEK_API_KEY = 'explicit-environment-override';
  try {
    const f = fixture();
    const saved = await f.set('stored-replacement');
    assert.equal(saved.credential.environmentOverride, true);
    assert.equal(JSON.stringify(saved).includes('explicit-environment-override'), false);
    assert.equal(JSON.stringify(saved).includes('stored-replacement'), false);
    const cleared = await f.clear();
    assert.equal(cleared.environmentOverride, true);
    assert.equal(cleared.runtimeOnly, true);
    assert.deepEqual(f.operations, [
      'store', 'set:explicit-environment-override', 'delete', 'set:explicit-environment-override',
    ]);
  } finally {
    if (previous === undefined) delete process.env.DEEPSEEK_API_KEY;
    else process.env.DEEPSEEK_API_KEY = previous;
  }
});

test('credential writes accept declared key references and reject undeclared arbitrary IDs', async () => {
  const f = fixture();
  f.configureCredentials(['custom', 'custom-cn-key']);
  await f.set('replacement', 'custom-cn-key');
  assert.equal(f.secrets.get('custom-cn-key'), 'replacement');
  await f.clear('custom-cn-key');
  assert.equal(f.secrets.has('custom-cn-key'), false);
  await assert.rejects(f.set('unknown', 'undeclared-key'), /unsupported provider/);
  assert.equal(f.secrets.has('undeclared-key'), false);
});
