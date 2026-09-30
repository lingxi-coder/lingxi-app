import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test, type TestContext } from 'node:test';
import type { PermissionRequest } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';

type Scope = { request_id: number; background_owned: boolean } | null;
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

function fixture(t: TestContext) {
  const diagnostics = new DiagnosticBuffer();
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/permission-scope', trusted: true }),
    projectPath: '/permission-scope', sessionId: '11111111-2222-4333-8444-555555555555', diagnostics, registerIpc: false });
  const internal = runtime as any;
  const pending = new Map<number, ReturnType<typeof deferred<Scope>>>();
  const queried: number[] = [];
  const decisions: Array<{ request_id: number; allow: boolean }> = [];
  const client = Object.assign(new EventEmitter(), {
    close: () => undefined,
    cancel: () => undefined,
    requestPermissionScope: (id: number) => {
      queried.push(id);
      const query = deferred<Scope>(); pending.set(id, query); return query.promise;
    },
    approvePermission: (id: number) => decisions.push({ request_id: id, allow: true }),
    denyPermission: (id: number) => decisions.push({ request_id: id, allow: false }),
  });
  internal.client = client;
  internal.activeWorkspace = '/permission-scope';
  internal.activeWorkspaceTrusted = true;
  internal.setState({ status: 'connected' });
  internal.wireClient(client, internal.generation);
  const sent: Array<{ channel: string; payload: any }> = [];
  const window = { send: (channel: string, payload: unknown) => sent.push({ channel, payload }),
    once: () => undefined, isDestroyed: () => false };
  runtime.registerWindow(window as any, 'app://desktop/index.html');
  t.after(() => runtime.dispose());
  const ask = (id: number, named = false) => {
    const request: PermissionRequest = { request_id: id, kind: { type: 'exit_plan_mode' },
      ...(named ? { worker: { name: 'Named runner', color: 'blue' } } : {}) };
    client.emit('permission', request);
    return request;
  };
  const scope = async (id: number, background_owned: boolean) => {
    pending.get(id)!.resolve({ request_id: id, background_owned }); await tick();
  };
  const foregroundEnds = () => client.emit('event', { type: 'turn_ended', outcome: { type: 'completed' },
    cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '' } });
  return { runtime, internal, client, diagnostics, sent, ask, scope, pending, queried, decisions, foregroundEnds };
}

for (const named of [false, true]) test(`${named ? 'named' : 'unnamed'} background permission is visible and answerable with no foreground turn`, async (t) => {
  const h = fixture(t);
  h.ask(1, named);
  assert.equal(h.runtime.pendingInteractions, 1, 'the bounded ownership query pins lifecycle operations');
  assert.equal(h.sent.filter((entry) => entry.channel === 'lingxi:permission').length, 0);
  await h.scope(1, true);
  assert.equal(h.runtime.turnActive, false);
  assert.equal(h.runtime.pendingInteractions, 1);
  const card = h.sent.find((entry) => entry.channel === 'lingxi:permission')!.payload;
  assert.equal(card.request_id, 1);
  assert.equal(card.backgroundOwned, true);
  assert.throws(() => h.runtime.beginArchive(), /active work/i);
  const replay: any[] = [];
  h.runtime.registerWindow({ send: (_channel: string, value: unknown) => replay.push(value),
    once: () => undefined, isDestroyed: () => false } as any, 'app://desktop/index.html');
  assert.equal(replay[0].backgroundOwned, true);
  h.runtime.approvePermission(1);
  assert.deepEqual(h.decisions, [{ request_id: 1, allow: true }]);
  assert.equal(h.runtime.pendingInteractions, 0);
  assert.equal(h.internal.backgroundPermissionIds.size, 0);
});

test('foreground completion and targeted cancellation preserve independently owned background permission cards', async (t) => {
  const h = fixture(t);
  h.client.emit('event', { type: 'turn_started', turn_id: 8 });
  h.ask(1); await h.scope(1, false);
  h.ask(2); await h.scope(2, true);
  h.foregroundEnds();
  assert.equal(h.runtime.pendingInteractions, 1);
  assert.equal(h.internal.pendingPermissionIds.has(1), false);
  assert.equal(h.internal.pendingPermissionIds.get(2).backgroundOwned, true);
  h.client.emit('event', { type: 'turn_started', turn_id: 9 });
  h.ask(3); await h.scope(3, false);
  h.runtime.cancelTurn(9);
  assert.equal(h.runtime.pendingInteractions, 1);
  assert.equal(h.internal.pendingPermissionIds.has(3), false);
  h.runtime.denyPermission(2);
  assert.deepEqual(h.decisions, [{ request_id: 2, allow: false }]);
  assert.equal(h.runtime.pendingInteractions, 0);
});

test('background scope validation can finish after the foreground owner ends', async (t) => {
  const h = fixture(t);
  h.client.emit('event', { type: 'turn_started' });
  h.ask(1);
  h.foregroundEnds();
  await h.scope(1, true);
  assert.equal(h.runtime.pendingInteractions, 1);
  assert.equal(h.internal.pendingPermissionIds.get(1).backgroundOwned, true);
});

test('a named synchronous worker remains foreground-scoped and cannot attach its delayed ask to the next turn', async (t) => {
  const h = fixture(t);
  h.client.emit('event', { type: 'turn_started' });
  h.ask(1, true);
  h.foregroundEnds();
  h.client.emit('event', { type: 'turn_started' });
  await h.scope(1, false);
  assert.equal(h.runtime.turnActive, true);
  assert.equal(h.runtime.pendingInteractions, 0);
  assert.equal(h.sent.filter((entry) => entry.channel === 'lingxi:permission').length, 0);
  assert.deepEqual(h.decisions, []);
});

for (const interruption of ['resolved', 'close', 'generation', 'client', 'null'] as const) {
  test(`a ${interruption} scope query cannot recreate a retired or replaced permission`, async (t) => {
    const h = fixture(t);
    h.ask(1);
    if (interruption === 'resolved') h.client.emit('event', { type: 'permission_request_resolved', request_id: 1, resolution: 'expired' });
    else if (interruption === 'close') h.client.emit('close', 1006, 'lost connection');
    else if (interruption === 'generation') h.internal.generation++;
    else if (interruption === 'client') h.internal.client = { close: () => undefined };
    h.pending.get(1)!.resolve(interruption === 'null' ? null : { request_id: 1, background_owned: true });
    await tick();
    assert.equal(h.runtime.pendingInteractions, 0);
    assert.equal(h.sent.filter((entry) => entry.channel === 'lingxi:permission').length, 0);
    assert.deepEqual(h.decisions, []);
  });
}

test('scope failures are observed and sanitized without approving an unverified request', async (t) => {
  const h = fixture(t);
  h.ask(1);
  h.pending.get(1)!.reject(new Error('token=fake-secret-token is unavailable'));
  await tick();
  assert.equal(h.runtime.pendingInteractions, 0);
  assert.deepEqual(h.decisions, []);
  const log = h.diagnostics.snapshot().map((entry) => entry.message).join('\n');
  assert.match(log, /scope unavailable/);
  assert.doesNotMatch(log, /fake-secret-token/);
});

test('full session and connection teardown remove background cards and pending scope queries', async (t) => {
  const h = fixture(t);
  h.ask(1); await h.scope(1, true);
  h.ask(2);
  h.client.emit('event', { type: 'session_ended', session_id: h.runtime.sessionId });
  assert.equal(h.runtime.pendingInteractions, 0);
  await h.scope(2, true);
  assert.equal(h.runtime.pendingInteractions, 0);
  h.ask(3); await h.scope(3, true);
  h.internal.clearPendingConnectionOperations();
  assert.equal(h.runtime.pendingInteractions, 0);
  assert.equal(h.internal.backgroundPermissionIds.size, 0);
});

test('permission scope lookups share the existing pending request limit', (t) => {
  const h = fixture(t);
  for (let id = 0; id < 1_000; id++) h.internal.permissionScopeChecks.set(id, { request_id: id, kind: { type: 'exit_plan_mode' } });
  h.ask(1_000);
  assert.deepEqual(h.queried, []);
  assert.match(h.diagnostics.snapshot().map((entry) => entry.message).join('\n'), /permission request limit reached/);
});
