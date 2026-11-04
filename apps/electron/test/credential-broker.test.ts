import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { buildBridgeArguments, buildCredentialEnvelope } from '../src/main/host-utils';

import {
  createMacCredentialBrokerClient,
  deletePluginSecretsForPlugin,
  resolveSessionLaunchPluginSecrets,
} from '../src/main/credential-broker';

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
