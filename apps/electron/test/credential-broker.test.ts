import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { buildBridgeArguments, buildCredentialEnvelope } from '../src/main/host-utils';

import {
  createMacCredentialBrokerClient,
  deletePluginSecretsForPlugin,
  resolveProviderCredential,
  resolveProviderIdForModel,
  resolveModelCredentialProviderIds,
  resolveProfileCredentialIds,
  configuredCredentialProviderIds,
  fusionCredentialSelection,
  resolveFusionCredentialProviderIds,
  resolveProviderTestCredential,
  resolveSessionLaunchCredentials,
  resolveSessionLaunchPluginSecrets,
  SessionLaunchCache,
} from '../src/main/credential-broker';

test('session launch cache single-flights repeated runtime startup reads and invalidates', async () => {
  let reads = 0;
  const broker = { resolve: async () => { reads += 1; await new Promise((resolve) => setTimeout(resolve, 2)); return 'cached-secret'; } };
  const cache = new SessionLaunchCache(60_000);

  const [first, second] = await Promise.all([
    cache.credentials('openrouter/model', broker),
    cache.credentials('openrouter/model', broker),
  ]);
  assert.deepEqual(first, { providerCredentials: { openrouter: 'cached-secret' } });
  assert.deepEqual(second, first);
  assert.equal(reads, 1);
  await cache.credentials('openrouter/model', broker);
  assert.equal(reads, 1);

  cache.invalidate();
  await cache.credentials('openrouter/model', broker);
  assert.equal(reads, 2);
});

test('plugin secrets use an independent broker service and launch envelope map', async () => {
  const requests: Array<Record<string, unknown>> = [];
  const broker = createMacCredentialBrokerClient({
    isPackaged: true,
    transport: {
      request: async (request) => {
        requests.push(request);
        if (request.op === 'health') {
          return { ok: true, protocol_version: 1, build_version: 'test' };
        }
        if (request.op === 'list') {
          return {
            ok: true,
            protocol_version: 1,
            accounts: ['weather%40official/API_KEY'],
          };
        }
        if (request.op === 'retrieve') {
          return { ok: true, protocol_version: 1, present: true, payload: 'plugin-secret' };
        }
        throw new Error(`unexpected request: ${JSON.stringify(request)}`);
      },
    },
  });

  const secrets = await resolveSessionLaunchPluginSecrets(broker);
  assert.deepEqual(JSON.parse(JSON.stringify(secrets)), {
    'weather@official': { API_KEY: 'plugin-secret' },
  });
  assert.equal(Object.getPrototypeOf(secrets), null);
  assert.equal(Object.getPrototypeOf(secrets['weather@official']), null);
  assert.equal(requests[1]?.['service'], 'com.lingxi.plugin-secrets.v1');
  assert.equal(requests[2]?.['service'], 'com.lingxi.plugin-secrets.v1');
  assert.equal(requests[2]?.['account'], 'weather%40official/API_KEY');
});

test('plugin secret cleanup deletes orphan keys for one plugin and leaves other plugins untouched', async () => {
  const refs = [
    { pluginId: 'weather@official', key: 'API_KEY' },
    { pluginId: 'weather@official', key: 'RENAMED_SCHEMA_KEY' },
    { pluginId: 'other@official', key: 'API_KEY' },
  ];
  const deleted: string[] = [];
  const broker = {
    listPluginSecrets: async () => refs.slice(),
    deletePluginSecret: async (pluginId: string, key: string) => {
      deleted.push(`${pluginId}/${key}`);
      const index = refs.findIndex(ref => ref.pluginId === pluginId && ref.key === key);
      if (index >= 0) refs.splice(index, 1);
    },
  };

  const result = await deletePluginSecretsForPlugin('weather@official', broker);

  assert.deepEqual(result, { deletedCount: 2, failedCount: 0, aborted: false });
  assert.deepEqual(deleted.sort(), ['weather@official/API_KEY', 'weather@official/RENAMED_SCHEMA_KEY']);
  assert.deepEqual(refs, [{ pluginId: 'other@official', key: 'API_KEY' }]);
});

test('plugin secret cleanup aborts if the registry guard changes before deleting', async () => {
  const deleted: string[] = [];
  let guardCalls = 0;
  const broker = {
    listPluginSecrets: async () => [
      { pluginId: 'weather@official', key: 'API_KEY' },
    ],
    deletePluginSecret: async (pluginId: string, key: string) => { deleted.push(`${pluginId}/${key}`); },
  };

  const result = await deletePluginSecretsForPlugin('weather@official', broker, async () => ++guardCalls === 1);

  assert.deepEqual(result, { deletedCount: 0, failedCount: 0, aborted: true });
  assert.deepEqual(deleted, []);
  assert.equal(guardCalls, 2, 'the post-list and per-key guards prevent broker deletes');
});

test('OpenRouter nested model references resolve to the OpenRouter credential', async () => {
  const resolved: string[] = [];
  const credential = await resolveProviderCredential('openrouter', {
    environment: { OPENROUTER_API_KEY: '' },
    credentialBroker: {
      health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
      listStatus: async () => [],
      preview: async () => ({ providerId: 'openrouter', configured: true }),
      resolve: async (providerId) => { resolved.push(providerId); return 'or-secret'; },
      set: async () => ({ providerId: 'openrouter', configured: true }),
      delete: async () => undefined,
    },
  });

  assert.equal(resolveProviderIdForModel('openrouter/minimax/minimax-m3:free'), 'openrouter');
  assert.equal(credential, 'or-secret');
  assert.deepEqual(resolved, ['openrouter']);
});

test('session launch resolves only the current provider from the broker', async () => {
  const resolved: string[] = [];
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async (request) => {
        if ((request as { op: string }).op === 'health') {
          return { ok: true, protocol_version: 1, build_version: 'test' };
        }
        if ((request as { op: string }).op === 'retrieve') {
          resolved.push((request as { account: string }).account);
          return {
            ok: true,
            protocol_version: 1,
            present: true,
            payload: 'or-secret',
          };
        }
        throw new Error(`unexpected request: ${JSON.stringify(request)}`);
      },
    },
  });

  const launch = await resolveSessionLaunchCredentials('openrouter/openai/gpt-5.4', {
    credentialBroker: broker,
    environment: {
      OPENAI_API_KEY: 'must-not-be-read',
      OPENROUTER_API_KEY: '',
    },
  });

  assert.deepEqual(resolved, ['openrouter']);
  assert.deepEqual(launch, {
    providerCredentials: { openrouter: 'or-secret' },
  });
});

test('session launch resolves a normalized custom provider prefix', async () => {
  const resolved: string[] = [];
  const launch = await resolveSessionLaunchCredentials('my-provider/model-a', {
    environment: {},
    credentialBroker: {
      health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
      listStatus: async () => [],
      preview: async () => ({ providerId: 'my-provider', configured: true, maskedValue: '••••cret' }),
      resolve: async (providerId) => { resolved.push(providerId); return 'custom-secret'; },
      set: async () => ({ providerId: 'my-provider', configured: true }),
      delete: async () => undefined,
    },
  });
  assert.deepEqual(resolved, ['my-provider']);
  assert.deepEqual(launch, { providerCredentials: { 'my-provider': 'custom-secret' } });
});

test('session launch prefers environment credentials and maps Claude references to Anthropic', async () => {
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async (request) => {
        if ((request as { op: string }).op === 'health') {
          return { ok: true, protocol_version: 1, build_version: 'test' };
        }
        if ((request as { op: string }).op === 'retrieve') {
          throw new Error('broker resolve should not run when the env already provides the key');
        }
        throw new Error(`unexpected request: ${JSON.stringify(request)}`);
      },
    },
  });

  const builtIn = await resolveSessionLaunchCredentials('builtin/claude-sonnet-5', {
    credentialBroker: broker,
    environment: { ANTHROPIC_API_KEY: 'env-anthropic' },
  });
  const unqualified = await resolveSessionLaunchCredentials('claude-opus-5', {
    credentialBroker: broker,
    environment: { ANTHROPIC_API_KEY: 'env-anthropic' },
  });

  assert.deepEqual(builtIn, { providerCredentials: { anthropic: 'env-anthropic' } });
  assert.deepEqual(unqualified, { providerCredentials: { anthropic: 'env-anthropic' } });
});

test('launch cache retries a credential read invalidated by a concurrent save or delete', async () => {
  let finish!: (value: string | undefined) => void;
  let reads = 0;
  let current: string | undefined = 'replacement';
  const broker = { resolve: () => ++reads === 1
    ? new Promise<string | undefined>((resolve) => { finish = resolve; })
    : Promise.resolve(current) };
  const cache = new SessionLaunchCache();
  const loading = cache.credentials('deepseek/model', broker);
  cache.invalidate();
  finish('retired');
  assert.deepEqual(await loading, { providerCredentials: { deepseek: 'replacement' } });
  current = undefined;
  cache.invalidate();
  assert.deepEqual(await cache.credentials('deepseek/model', broker), {});
});

test('stored Anthropic keys use the mutable provider-key launch envelope', async () => {
  const launch = await resolveSessionLaunchCredentials('anthropic/claude-sonnet-5', {
    environment: {}, credentialBroker: { resolve: async () => 'stored-anthropic' },
  });
  const envelope = JSON.parse(buildCredentialEnvelope(launch));
  assert.equal(envelope.api_key, null);
  assert.deepEqual(envelope.provider_keys, { anthropic: 'stored-anthropic' });
  const args = buildBridgeArguments({
    workspace: '/workspace', bridgeDir: '/bridge', trusted: true,
    hasApiKey: !!launch.apiKey, hasCredentialStdin: !!launch.providerCredentials,
  });
  assert.equal(args.includes('--credential-stdin'), true);
  assert.equal(args.includes('--api-key-stdin'), false);
});

test('model aliases and qualified connections hydrate referenced provider keys and same-group failover slots', () => {
  const settings = {
    providers: {
      custom: {
        models: [{ id: 'm', aliases: ['fast'] }], credentialIds: ['inherited-key'],
        connections: [
          { id: 'cn', credentialIds: ['cn-one', 'cn-two'] },
          { id: 'us' },
        ],
      },
      backup: { models: ['b'], credentialIds: ['backup-key'] },
      unrelated: { models: ['u'], credentialIds: ['unrelated-key'] },
    },
    routing: { aliases: { picked: 'custom:cn#0/m' }, fallback: { m: ['backup/b'] } },
  };
  assert.deepEqual(resolveProfileCredentialIds('custom:cn#0', settings), ['cn-one', 'cn-two', 'inherited-key']);
  for (const reference of ['fast', 'custom/m', 'custom:cn#0/m', 'picked']) {
    assert.deepEqual(resolveModelCredentialProviderIds(reference, settings), ['cn-one', 'cn-two', 'inherited-key', 'backup-key']);
  }
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, {
    config: { enabled: true, panelModels: [{ profile: 'custom:cn#0', model: 'm' }] },
  }), ['cn-one', 'cn-two', 'inherited-key'], 'Fusion loads exact chosen groups without ordinary routing fallback');
  assert.deepEqual(resolveProfileCredentialIds('custom:missing', settings), []);
  assert.deepEqual(configuredCredentialProviderIds(settings), [
    'custom', 'cn-one', 'cn-two', 'inherited-key', 'backup', 'backup-key', 'unrelated', 'unrelated-key',
  ]);
});

test('connection-level model overrides hydrate the selected group even without top-level models', () => {
  const settings = { providers: { custom: { connections: [
    { id: 'cn', models: ['cn-model'], credentialIds: ['cn-key'] },
    { id: 'us', models: ['us-model'] },
  ] } } };
  assert.deepEqual(resolveModelCredentialProviderIds('custom:cn#0/cn-model', settings), ['cn-key', 'custom']);
  assert.deepEqual(resolveModelCredentialProviderIds('us-model', settings), ['cn-key', 'custom']);
});

test('provider connection tests resolve only the selected broker credential', async () => {
  const resolved: string[] = [];
  const broker = {
    health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
    listStatus: async () => [],
    preview: async (providerId: string) => ({ providerId, configured: true }),
    resolve: async (providerId: string) => {
      resolved.push(providerId);
      return 'stored-test-key';
    },
    set: async (providerId: string) => ({ providerId, configured: true }),
    delete: async () => undefined,
  };

  assert.equal(
    await resolveProviderTestCredential('deepseek', undefined, broker),
    'stored-test-key',
  );
  assert.deepEqual(resolved, ['deepseek']);
  assert.equal(
    await resolveProviderTestCredential('deepseek', 'draft-test-key', broker),
    'draft-test-key',
  );
  assert.deepEqual(resolved, ['deepseek']);
});

test('provider status and preview use the channel-scoped generic broker protocol', async () => {
  const requests: Array<Record<string, unknown>> = [];
  const broker = createMacCredentialBrokerClient({
    isPackaged: true,
    transport: {
      request: async (request) => {
        requests.push(request);
        if (request.op === 'health') {
          return { ok: true, protocol_version: 1, build_version: 'test' };
        }
        if (request.op === 'list') {
          return { ok: true, protocol_version: 1, accounts: ['deepseek'] };
        }
        if (request.op === 'preview') {
          return { ok: true, protocol_version: 1, present: true, payload: '••••abcd' };
        }
        throw new Error(`unexpected request: ${JSON.stringify(request)}`);
      },
    },
  });

  assert.deepEqual(await broker!.listStatus(['deepseek', 'openai']), [
    { providerId: 'deepseek', configured: true },
    { providerId: 'openai', configured: false },
  ]);
  assert.deepEqual(await broker!.preview('deepseek'), {
    providerId: 'deepseek',
    configured: true,
    maskedValue: '••••abcd',
  });
  assert.equal(requests[1]?.['service'], 'com.lingxi.provider-credentials.v1');
  assert.equal(requests[2]?.['service'], 'com.lingxi.provider-credentials.v1');
  assert.equal(requests[2]?.['account'], 'deepseek');
});

test('broker protocol mismatch fails with an upgrade instruction', async () => {
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async () => ({ ok: true, protocol_version: 2, build_version: 'future' }),
    },
  });
  await assert.rejects(
    broker!.health(),
    /protocol mismatch.*upgrade LingXi Desktop/i,
  );
});

test('every broker response is checked for protocol compatibility', async () => {
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async (request) => request.op === 'health'
        ? { ok: true, protocol_version: 1, build_version: 'test' }
        : { ok: true, protocol_version: 2, accounts: [] },
    },
  });

  await assert.rejects(
    () => broker!.listStatus(['openai']),
    /protocol mismatch.*upgrade LingXi Desktop/i,
  );
});

test('preview rejects an unmasked broker payload before it can reach the renderer', async () => {
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async (request) => request.op === 'health'
        ? { ok: true, protocol_version: 1, build_version: 'test' }
        : { ok: true, protocol_version: 1, present: true, payload: 'sk-full-secret' },
    },
  });

  await assert.rejects(
    () => broker!.preview('openai'),
    /invalid masked preview/i,
  );
});

test('unpackaged macOS refuses to execute an unverified helper path', async () => {
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin',
    isPackaged: false,
    binaryPath: '/tmp/untrusted/lingxi-credential-client',
  });
  await assert.rejects(broker!.health(), /Apple Development signed package/);
});

test('custom aliases resolve only the selected profile and its model fallback chain', () => {
  const settings = {
    providers: {
      primary: { models: [{ id: 'model', aliases: ['fast', 'vendor/wire'] }] },
      backup: { models: ['backup-model'] },
      unrelated: { models: ['other-model'] },
    },
    routing: { aliases: { boss: 'primary/model' }, fallback: { model: ['backup/backup-model', 'missing/model'] } },
  };
  for (const model of ['boss', 'fast', 'model', 'primary/model', 'primary/fast', 'vendor/wire']) {
    assert.deepEqual(resolveModelCredentialProviderIds(model, settings), ['primary', 'backup']);
  }
  assert.deepEqual(resolveModelCredentialProviderIds('unknown', settings), []);
  assert.deepEqual(resolveModelCredentialProviderIds('unrelated/other-model', settings), ['unrelated']);
  assert.deepEqual(resolveModelCredentialProviderIds('fast', { providers: { ...settings.providers, duplicate: { models: [{ id: 'second', aliases: ['fast'] }] } } }), []);
});

test('Fusion credential routes require opt-in and use exact profiles without ordinary fallbacks', () => {
  const settings = {
    providers: { custom: { models: [{ id: 'real-model', aliases: ['fast'] }] }, backup: { models: ['backup-model'] } },
    routing: { fallback: { 'real-model': ['backup/backup-model'] } },
    fusion: { strategies: [{ id: 'unselected', name: 'Unselected', config: { panelModels: [{ profile: 'openai-chatgpt', model: 'gpt' }] } }] },
  };
  const selection = { config: { enabled: false, panelModels: [{ profile: 'custom', model: 'real-model' }], analystModel: { profile: 'kimi', model: 'kimi-k3' } } };
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, selection), []);
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, selection, true), ['custom', 'kimi']);
  selection.config.enabled = true;
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, selection), ['custom', 'kimi']);
  assert.deepEqual(resolveFusionCredentialProviderIds({}, { config: { enabled: true, panelModels: [null, 'broken', { profile: 42 }] } }), []);
});


test('plugin launch secrets preserve reserved property names without modifying inherited objects', async () => {
  const broker = createMacCredentialBrokerClient({
    transport: {
      request: async (request) => {
        if (request.op === 'health') return { ok: true, protocol_version: 1, build_version: 'test' };
        if (request.op === 'list') return { ok: true, protocol_version: 1, accounts: [
          'constructor/API_KEY', 'weather/__proto__', 'weather/constructor',
        ] };
        if (request.op === 'retrieve') return { ok: true, protocol_version: 1, present: true, payload: `fake:${request.account}` };
        throw new Error('unexpected fake broker operation');
      },
    },
  });
  const inheritedDescriptor = Object.getOwnPropertyDescriptor(Object, 'API_KEY');
  const secrets = await resolveSessionLaunchPluginSecrets(broker);
  const decoded = JSON.parse(JSON.stringify(secrets));
  assert.equal(decoded.constructor.API_KEY, 'fake:constructor/API_KEY');
  assert.equal(decoded.weather.__proto__, 'fake:weather/__proto__');
  assert.equal(decoded.weather.constructor, 'fake:weather/constructor');
  assert.equal(Object.getPrototypeOf(secrets), null);
  assert.equal(Object.getPrototypeOf(secrets['constructor']), null);
  assert.equal(Object.getPrototypeOf(secrets['weather']), null);
  assert.deepEqual(Object.getOwnPropertyDescriptor(Object, 'API_KEY'), inheritedDescriptor);
});

function fakeCredentialChild(onRequest: (request: { op: string }, child: ChildProcessWithoutNullStreams) => void) {
  const child = new EventEmitter() as ChildProcessWithoutNullStreams;
  const stdin = new PassThrough();
  Object.assign(child, {
    stdin,
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    kill: () => true,
  });
  const chunks: Buffer[] = [];
  stdin.on('data', (chunk: Buffer) => chunks.push(chunk));
  stdin.on('finish', () => onRequest(JSON.parse(Buffer.concat(chunks).toString('utf8')), child));
  return child;
}

test('credential helper preserves Unicode payloads split at every stdout byte', async () => {
  const fakeSecret = 'sëcret-密钥-🔑';
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => fakeCredentialChild((request, child) => {
      const response = request.op === 'health'
        ? { ok: true, protocol_version: 1, build_version: 'test' }
        : { ok: true, protocol_version: 1, present: true, payload: fakeSecret };
      for (const byte of Buffer.from(JSON.stringify(response))) child.stdout.emit('data', Buffer.from([byte]));
      child.emit('close', 0, null);
    }),
  });
  assert.equal(await broker!.resolve('openai'), fakeSecret);
});

test('credential helper enforces its raw response byte limit before decoding', async () => {
  let killed = false;
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => {
      const child = fakeCredentialChild((_request, process) => {
        process.stdout.emit('data', Buffer.alloc(128 * 1024, 0xc3));
        process.stdout.emit('data', Buffer.from([0xa9]));
      });
      child.kill = () => { killed = true; return true; };
      return child;
    },
  });
  await assert.rejects(broker!.health(), /response too large/);
  assert.equal(killed, true);
});

test('credential helper joins split UTF-8 stderr bytes for error diagnostics', async () => {
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => fakeCredentialChild((_request, child) => {
      for (const byte of Buffer.from('fake erreur: verrouillé')) child.stderr.emit('data', Buffer.from([byte]));
      child.emit('close', 1, null);
    }),
  });
  await assert.rejects(broker!.health(), /fake erreur: verrouillé/);
});


test('credential helper accepts a response exactly at its raw byte limit with a split Unicode character', async () => {
  const health = Buffer.from(JSON.stringify({ ok: true, protocol_version: 1, build_version: 'tëst' }));
  const response = Buffer.concat([health, Buffer.alloc(128 * 1024 - health.length, 0x20)]);
  const split = response.indexOf(Buffer.from('ë')) + 1;
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => fakeCredentialChild((_request, child) => {
      child.stdout.emit('data', response.subarray(0, split));
      child.stdout.emit('data', response.subarray(split));
      child.emit('close', 0, null);
    }),
  });
  assert.deepEqual(await broker!.health(), { protocolVersion: 1, buildVersion: 'tëst' });
});


test('Fusion credential views apply preset counts and max-panel without mutating the snapshot revision', () => {
  const selection = { reference: { scope: 'user', id: 'review' }, revision: 'original-revision', config: {
    enabled: true, panelModels: [
      { profile: 'deepseek', model: 'a' }, { profile: 'kimi', model: 'b' },
      { profile: 'openai-chatgpt', model: 'c' }, { profile: 'openai', model: 'd' },
    ], analystModel: { profile: 'deepseek', model: 'judge' }, qualityPanelCount: 4,
  } };
  const fast = fusionCredentialSelection(selection, { preset: 'fast' });
  assert.deepEqual(resolveFusionCredentialProviderIds({}, fast, true), ['deepseek', 'kimi']);
  assert.equal(fast.revision, 'original-revision');
  assert.equal(selection.config.panelModels.length, 4);
  assert.notEqual(fast.config, selection.config);
  const capped = fusionCredentialSelection(selection, { maxPanel: 2 });
  assert.deepEqual(resolveFusionCredentialProviderIds({}, capped, true), ['deepseek', 'kimi']);
  const required = fusionCredentialSelection({ config: { ...selection.config, minSuccessfulPanels: 3 } }, { preset: 'fast' });
  assert.equal(required.config.panelModels.length, 3);
  assert.deepEqual(resolveFusionCredentialProviderIds({}, fusionCredentialSelection({ config: { ...selection.config, preset: 'fast' } }), true), ['deepseek', 'kimi']);
});

test('Fusion explicit models replace the roster and bare ids inherit the captured parent profile', () => {
  const settings = { providers: { primary: { models: [{ id: 'actual', aliases: ['fast'] }] }, alternate: { models: ['other'] } }, routing: { aliases: { boss: 'alternate/other' } } };
  const snapshot = { config: { enabled: true, panelModels: [{ profile: 'openai-chatgpt', model: 'not-selected' }], analystModel: { profile: 'deepseek', model: 'judge' } } };
  const inherited = fusionCredentialSelection(snapshot, { models: ['fast', 'boss'] }, 'primary');
  assert.deepEqual(inherited.config.panelModels, [{ profile: 'primary', model: 'fast' }, { profile: 'primary', model: 'boss' }]);
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, inherited, true), ['primary', 'deepseek']);
  assert.throws(() => fusionCredentialSelection(snapshot, { models: ['fast', 'boss'] }), /conversation provider is unavailable/);
  const explicit = fusionCredentialSelection(snapshot, { models: ['openai-chatgpt:chosen', 'kimi:kimi-k3'], preset: 'fast', maxPanel: 2 });
  assert.deepEqual(resolveFusionCredentialProviderIds({}, explicit, true), ['openai-chatgpt', 'kimi', 'deepseek']);
  assert.deepEqual(snapshot.config.panelModels, [{ profile: 'openai-chatgpt', model: 'not-selected' }]);
});

test('Fusion configured pairs trim whitespace and cannot be shadowed by another profile model id', () => {
  const settings = { providers: { unrelated: { models: ['deepseek/a'] } }, routing: { fallback: { a: ['openai-chatgpt/unused'] } } };
  const selection = { config: { panelModels: [{ profile: ' deepseek ', model: ' a ' }], analystModel: { profile: ' kimi ', model: ' judge ' } } };
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, selection, true), ['deepseek', 'kimi']);
  assert.deepEqual(resolveFusionCredentialProviderIds({ providers: { claude: { models: ['custom'] } } },
    { config: { panelModels: [{ profile: 'claude', model: 'custom' }] } }, true), ['claude']);
});

test('repeated Anthropic status and preview queries never send credential mutations', async () => {
  const requests: string[] = [];
  const broker = createMacCredentialBrokerClient({
    isPackaged: true,
    transport: {
      request: async (request) => {
        requests.push(request.op as string);
        if (request.op === 'health') return { ok: true, protocol_version: 1, build_version: 'test' };
        if (request.op === 'list') return { ok: true, protocol_version: 1, accounts: ['anthropic'] };
        if (request.op === 'preview') return { ok: true, protocol_version: 1, present: true, payload: '••••abcd' };
        throw new Error('Status query attempted a non-query broker operation');
      },
    },
  });
  for (let refresh = 0; refresh < 3; refresh += 1) {
    assert.deepEqual(await broker.listStatus(['anthropic']), [{ providerId: 'anthropic', configured: true }]);
    assert.deepEqual(await broker.preview('anthropic'), {
      providerId: 'anthropic', configured: true, maskedValue: '••••abcd',
    });
  }
  assert.deepEqual(requests, ['health', 'list', 'preview', 'list', 'preview', 'list', 'preview']);
});

test('cold credential helper may finish after the former process deadline but remains bounded', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let child: ChildProcessWithoutNullStreams | undefined;
  let killed = false;
  const broker = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => {
      child = fakeCredentialChild(() => {});
      child.kill = () => { killed = true; return true; };
      return child;
    },
  });
  const pending = broker!.health();
  t.mock.timers.tick(20_001);
  assert.equal(killed, false, 'cold trust validation must not be killed by the old outer deadline');
  child!.stdout.emit('data', JSON.stringify({ ok: true, protocol_version: 1, build_version: 'cold' }));
  child!.emit('close', 0, null);
  assert.deepEqual(await pending, { protocolVersion: 1, buildVersion: 'cold' });
  const stalled = createMacCredentialBrokerClient({
    platform: 'darwin', isPackaged: true, channel: 'development', binaryPath: '/fake-credential-helper',
    spawnProcess: () => {
      const process = fakeCredentialChild(() => {});
      process.kill = () => { killed = true; return true; };
      return process;
    },
  });
  const rejection = assert.rejects(stalled!.health(), /credential broker timed out/);
  t.mock.timers.tick(90_000);
  await rejection;
  assert.equal(killed, true, 'a stalled helper must still be terminated');
});
