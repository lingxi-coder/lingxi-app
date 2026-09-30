import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';

import {
  createMacCredentialBrokerClient,
  resolveProviderCredential,
  resolveProviderIdForModel,
  resolveModelCredentialProviderIds,
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

  assert.deepEqual(builtIn, { apiKey: 'env-anthropic' });
  assert.deepEqual(unqualified, { apiKey: 'env-anthropic' });
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

test('Fusion credential routes require opt-in for normal prompts and include custom aliases', () => {
  const settings = {
    providers: { custom: { models: [{ id: 'real-model', aliases: ['fast'] }] }, backup: { models: ['backup-model'] } },
    routing: { fallback: { 'real-model': ['backup/backup-model'] } },
    fusion: { enabled: false, panelModels: [{ profile: 'custom', model: 'fast' }], analystModel: { profile: 'kimi', model: 'kimi-k3' }, synthesizerModel: { profile: 'custom', model: 'fast' } },
  };
  assert.deepEqual(resolveFusionCredentialProviderIds(settings), []);
  assert.deepEqual(resolveFusionCredentialProviderIds(settings, true), ['custom', 'backup', 'kimi']);
  settings.fusion.enabled = true;
  assert.deepEqual(resolveFusionCredentialProviderIds(settings), ['custom', 'backup', 'kimi']);
  assert.deepEqual(resolveFusionCredentialProviderIds({ fusion: { enabled: true, panelModels: [null, 'broken', { profile: 42 }] } }), []);
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
