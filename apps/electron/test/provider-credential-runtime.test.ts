import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { BridgeClient } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

function connectedRuntime(resolveCredential: () => Promise<string | undefined>) {
  const commands: Array<Record<string, any>> = [];
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    resolveProviderCredential: resolveCredential,
  });
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: Record<string, any>): void };
  const acknowledge = (command: Record<string, any>, error?: string) => client.emit('event', {
    type: 'provider_credential_status', operation_id: command.operation_id,
    configured_provider_ids: command.type === 'set_provider_credential' ? [command.provider_id] : [],
    storage_encrypted: true, credential_previews: {}, ...(error ? { error } : {}),
  });
  client.sendCommand = (command) => {
    commands.push(command);
    if (command.type === 'set_provider_credential' || command.type === 'delete_provider_credential') {
      queueMicrotask(() => acknowledge(command));
    }
  };
  (runtime as any).client = client;
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).state = { status: 'connected' };
  (runtime as any).wireClient(client, 0);
  return { runtime, client, commands, acknowledge };
}

test('a late credential read cannot overwrite an acknowledged replacement', async () => {
  const oldRead = deferred<string>();
  const { runtime, commands } = connectedRuntime(() => oldRead.promise);
  const loading = (runtime as any).ensureProviderCredentialCached('deepseek');
  await runtime.refreshCachedProviderCredential('deepseek', 'replacement');
  oldRead.resolve('retired');
  await loading;
  assert.deepEqual(commands.map((command) => command.credential), ['replacement']);
  assert.equal(runtime.hasCachedProviderCredential('deepseek'), true);
});

test('a selected custom connection injects referenced keys and future rotation uses their actual IDs', async () => {
  const loaded: string[] = [];
  const { runtime, commands } = connectedRuntime(async () => undefined);
  (runtime as any).opts.resolveProviderCredential = async (id: string) => { loaded.push(id); return `secret-${id}`; };
  (runtime as any).credentialRoutingSettings = { providers: {
    custom: { models: ['m'], connections: [{ id: 'cn', credentialIds: ['cn-key'] }, { id: 'us', credentialIds: ['us-key'] }] },
    unrelated: { models: ['u'], credentialIds: ['private-key'] },
  } };
  await (runtime as any).ensureModelProviderCredential('custom:cn#0/m');
  assert.deepEqual(loaded, ['cn-key', 'us-key']);
  assert.deepEqual(commands.map((command) => command.provider_id), ['cn-key', 'us-key']);
  await runtime.refreshCachedProviderCredential('cn-key', 'replacement');
  await runtime.refreshCachedProviderCredential('cn-key');
  assert.equal(commands.at(-2)?.credential, 'replacement');
  assert.equal(commands.at(-1)?.type, 'delete_provider_credential');
  assert.equal(runtime.hasCachedProviderCredential('cn-key'), false);
});

test('a late credential read cannot resurrect an acknowledged deletion', async () => {
  const oldRead = deferred<string>();
  let reads = 0;
  const { runtime, commands } = connectedRuntime(() => ++reads === 1 ? oldRead.promise : Promise.resolve(undefined));
  const loading = (runtime as any).ensureProviderCredentialCached('deepseek');
  await runtime.refreshCachedProviderCredential('deepseek');
  oldRead.resolve('retired');
  await loading;
  assert.deepEqual(commands.map((command) => command.type), ['delete_provider_credential']);
  assert.equal(runtime.hasCachedProviderCredential('deepseek'), false);
});

test('an update waits for an earlier credential command and prevents late ACK cache resurrection', async () => {
  const { runtime, client, commands, acknowledge } = connectedRuntime(async () => 'retired');
  client.sendCommand = (command) => { commands.push(command); };
  const loading = (runtime as any).ensureProviderCredentialCached('deepseek');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(commands[0]?.credential, 'retired');
  const replacement = runtime.refreshCachedProviderCredential('deepseek', 'replacement');
  acknowledge(commands[0]!);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(commands[1]?.credential, 'replacement');
  acknowledge(commands[1]!);
  await replacement;
  await loading;
  assert.equal(runtime.hasCachedProviderCredential('deepseek'), true);
  assert.deepEqual(commands.map((command) => command.credential), ['retired', 'replacement']);
});

test('failed or superseded credential ACKs never mark the key as cached', async () => {
  for (const superseded of [false, true]) {
    const { runtime, client, commands, acknowledge } = connectedRuntime(async () => 'replacement');
    client.sendCommand = (command) => { commands.push(command); };
    const update = runtime.cacheProviderCredential('deepseek', 'replacement');
    const rejected = assert.rejects(update, superseded ? /interrupted/ : /storage failed/);
    await new Promise((resolve) => setImmediate(resolve));
    if (superseded) (runtime as any).generation += 1;
    if (superseded) {
      (runtime as any).handleProviderCredentialStatus({
        type: 'provider_credential_status', operation_id: commands[0]!.operation_id,
        configured_provider_ids: ['deepseek'], storage_encrypted: true, credential_previews: {},
      });
    } else acknowledge(commands[0]!, 'storage failed');
    await rejected;
    assert.equal(runtime.hasCachedProviderCredential('deepseek'), false);
  }
});

test('credential mutation reaches active and idle engines even when host cache bookkeeping is absent', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const active = connectedRuntime(async () => 'replacement');
  const idle = connectedRuntime(async () => 'replacement');
  for (const [id, fixture] of [['active', active], ['idle', idle]] as const) {
    (manager as any).runtimes.set(id, fixture.runtime);
  }
  (active.runtime as any).runtimeCredentialProviders.add('deepseek');
  (active.runtime as any).activeTurn = true;
  await manager.refreshCachedProviderCredential('deepseek', 'replacement');
  await manager.clearCachedProviderCredential('deepseek');
  for (const fixture of [active, idle]) {
    assert.deepEqual(fixture.commands.map((command) => command.type), ['set_provider_credential', 'delete_provider_credential']);
    assert.equal(fixture.runtime.hasCachedProviderCredential('deepseek'), false);
  }
  assert.equal(active.runtime.turnActive, true);
});

test('a failed pool update still waits for acknowledgements from all other engines', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const pending = deferred<void>();
  (manager as any).runtimes.set('failed', { refreshCachedProviderCredential: async () => { throw new Error('ACK failed'); } });
  (manager as any).runtimes.set('pending', { refreshCachedProviderCredential: () => pending.promise });
  let settled = false;
  const mutation = manager.refreshCachedProviderCredential('deepseek', 'replacement').finally(() => { settled = true; });
  const rejected = assert.rejects(mutation, /ACK failed/);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled, false);
  pending.resolve();
  await rejected;
});

test('a disconnected living engine reconciles deleted credentials before publishing connected', async () => {
  const { runtime, commands } = connectedRuntime(async () => undefined);
  (runtime as any).runtimeCredentialProviders.add('deepseek');
  (runtime as any).state = { status: 'disconnected' };
  await runtime.refreshCachedProviderCredential('deepseek');
  assert.equal(commands.length, 0);
  const originalConnect = BridgeClient.prototype.connect;
  const states: string[] = [];
  (runtime as any).broadcast = (_channel: string, event: { status?: string }) => {
    if (event.status) states.push(event.status);
  };
  BridgeClient.prototype.connect = async function () {
    this.sendCommand = (command) => {
      if (command.type !== 'delete_provider_credential') return;
      commands.push(command);
      queueMicrotask(() => this.emit('event', {
        type: 'provider_credential_status', operation_id: command.operation_id,
        configured_provider_ids: [], storage_encrypted: true, credential_previews: {},
      }));
    };
    return { server_name: 'test', protocol_version: '0.2.0', capabilities: { client_protocol_version: '11.0.0' } } as any;
  };
  try {
    await (runtime as any).connectBridgeClient('/unused/test.lock', 0, false);
    assert.equal(runtime.hasCachedProviderCredential('deepseek'), false);
    assert.equal(commands[0]?.type, 'delete_provider_credential');
    assert.equal(states.at(-1), 'connected');
  } finally {
    BridgeClient.prototype.connect = originalConnect;
  }
});

for (const deleted of [false, true]) {
  test(`a newly adopted engine reconciles an earlier credential ${deleted ? 'deletion' : 'replacement'} before readiness`, async () => {
    const commands: Array<Record<string, any>> = [];
    const loaded: string[] = [];
    const states: string[] = [];
    const manager = new SessionRuntimeManager({
      launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
      resolveProviderCredential: async (id) => { loaded.push(id); return deleted ? undefined : 'replacement'; },
    });
    if (deleted) await manager.clearCachedProviderCredential('deepseek');
    else await manager.refreshCachedProviderCredential('deepseek', 'replacement');
    const runtime = await manager.ensure({ projectPath: '/workspace', sessionId: '00000000-0000-4000-8000-000000000001' }, false);
    (runtime as any).activeWorkspace = '/workspace';
    (runtime as any).activeWorkspaceTrusted = true;
    (runtime as any).broadcast = (_channel: string, event: { status?: string }) => {
      if (event.status) states.push(event.status);
    };
    const originalConnect = BridgeClient.prototype.connect;
    let client!: BridgeClient;
    BridgeClient.prototype.connect = async function () {
      client = this;
      this.sendCommand = (command) => {
        if (command.type === 'set_provider_credential' || command.type === 'delete_provider_credential') commands.push(command);
      };
      return { server_name: 'test', protocol_version: '0.2.0', capabilities: { client_protocol_version: '11.0.0' } } as any;
    };
    try {
      // Adoption skips launchConfig, connecting directly to the living engine.
      const connecting = (runtime as any).connectBridgeClient('/unused/adopted.lock', 0, false);
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(loaded, ['deepseek']);
      assert.equal(commands[0]?.type, deleted ? 'delete_provider_credential' : 'set_provider_credential');
      assert.equal(states.includes('connected'), false);
      client.emit('event', {
        type: 'provider_credential_status', operation_id: commands[0]!.operation_id,
        configured_provider_ids: deleted ? [] : ['deepseek'], storage_encrypted: true, credential_previews: {},
      });
      await connecting;
      assert.equal(runtime.hasCachedProviderCredential('deepseek'), !deleted);
      assert.equal(states.at(-1), 'connected');
    } finally {
      BridgeClient.prototype.connect = originalConnect;
    }
  });
}
