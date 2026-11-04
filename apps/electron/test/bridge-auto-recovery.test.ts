import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';

function fixture(): { runtime: SessionRuntime; internal: any; child: EventEmitter; calls: string[]; events: any[] } {
  const runtime = new SessionRuntime({ sessionId: '11111111-2222-4333-8444-555555555555', projectPath: '/workspace', launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const internal = runtime as any;
  const child = new EventEmitter();
  const calls: string[] = [];
  const events: any[] = [];
  Object.assign(internal, { child, desiredRunning: true, state: { status: 'connected' }, sessionHasHistory: true, ownedTranscriptPath: '/workspace/original.jsonl' });
  internal.broadcastClientEvent = (event: any): void => { events.push(event); };
  internal.stopBridge = async (): Promise<void> => { calls.push('stop'); internal.child = null; ++internal.generation; };
  internal.startInternal = async (publish: boolean): Promise<void> => { assert.equal(publish, false); calls.push('start'); internal.child = new EventEmitter(); };
  internal.restoreOwnedSessionIfNeeded = async (): Promise<void> => { assert.equal(internal.ownedSessionResumeRequired, true); calls.push('restore'); internal.ownedSessionResumeRequired = false; };
  return { runtime, internal, child, calls, events };
}

test('unexpected managed exit restores once without replaying foreground or Side Chat work', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.activeTurn = true;
  f.internal.liveSideChats.add('side-1');
  f.internal.pendingSideCreates.add('create-1');
  f.internal.managedChildExited(f.child, 1, null, true);
  f.internal.managedChildExited(f.child, 1, null, true);
  assert.equal(f.internal.activeTurn, false);
  assert.equal(f.internal.liveSideChats.size, 0);
  assert.equal(f.events.filter(e => e.type === 'turn_ended').length, 1);
  assert.deepEqual(f.events.find(e => e.type === 'turn_ended').outcome, { type: 'cancelled' });
  assert.equal(f.events.filter(e => e.type === 'side_chat_ended').length, 1);
  assert.equal(f.events.find(e => e.type === 'side_chat_error').request_id, 'create-1');
  assert.match(f.events.find(e => e.type === 'system_notice').message, /Side Chats were interrupted/);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  assert.equal(f.runtime.connectionState.status, 'connected');
  assert.equal(f.runtime.needsSessionResume, false);
  await f.runtime.dispose();
});

test('startup exits, stale children and intentional stops never auto-respawn', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const scenario of ['startup', 'stale', 'intentional'] as const) {
    const f = fixture();
    if (scenario === 'intentional') f.internal.desiredRunning = false;
    f.internal.managedChildExited(scenario === 'stale' ? new EventEmitter() : f.child, 0, null, scenario !== 'startup');
    t.mock.timers.tick(10_000);
    await f.runtime.waitForLifecycle();
    assert.deepEqual(f.calls, []);
    await f.runtime.dispose();
  }
});

test('stop and dispose cancel recovery backoff', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const action of ['stop', 'dispose'] as const) {
    const f = fixture();
    f.internal.managedChildExited(f.child, 0, null, true);
    await f.runtime[action]();
    t.mock.timers.tick(10_000);
    await Promise.resolve();
    assert.deepEqual(f.calls, ['stop']);
    if (action === 'stop') await f.runtime.dispose();
  }
});

test('failed auto recovery backs off and stops after three attempts', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.startInternal = async (): Promise<void> => { f.calls.push('start'); throw new Error('engine unavailable'); };
  f.internal.managedChildExited(f.child, 1, null, true);
  for (const delay of [250, 500, 1000]) { t.mock.timers.tick(delay); await f.runtime.waitForLifecycle(); }
  t.mock.timers.tick(60_000);
  await f.runtime.waitForLifecycle();
  assert.equal(f.calls.filter(c => c === 'start').length, 3);
  assert.equal(f.runtime.connectionState.status, 'error');
  await f.runtime.dispose();
});

test('owned adopted process loss recovers; living and external peers remain untouched', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const scenario of ['dead-owned', 'live-owned', 'external'] as const) {
    const f = fixture();
    const client = Object.assign(new EventEmitter(), { sendCommand(): void {} });
    Object.assign(f.internal, { child: null, adoptedPid: 4242, adoptedProcessOwned: scenario !== 'external', processIsAlive: () => scenario !== 'dead-owned' });
    f.internal.wireClient(client, f.internal.generation);
    client.emit('close', 1006, 'peer closed');
    t.mock.timers.tick(250); t.mock.timers.tick(250);
    await f.runtime.waitForLifecycle();
    assert.deepEqual(f.calls, scenario === 'dead-owned' ? ['stop', 'start', 'restore'] : []);
    await f.runtime.dispose();
  }
});

test('recovery waits for owned history and concurrent start joins it', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  let restore!: () => void;
  const gate = new Promise<void>(resolve => { restore = resolve; });
  f.internal.restoreOwnedSessionIfNeeded = async (): Promise<void> => { f.calls.push('restore'); await gate; f.internal.ownedSessionResumeRequired = false; };
  f.internal.managedChildExited(f.child, 1, null, true);
  t.mock.timers.tick(250);
  for (let i = 0; i < 10; ++i) await Promise.resolve();
  assert.equal(f.runtime.needsSessionResume, true);
  assert.notEqual(f.runtime.connectionState.status, 'connected');
  const joining = f.runtime.start();
  restore(); await joining;
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  await f.runtime.dispose();
});

test('an owned detached process can exit after its transport closes', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  let alive = true;
  const client = Object.assign(new EventEmitter(), { sendCommand(): void {} });
  Object.assign(f.internal, { child: null, adoptedPid: 4242, adoptedProcessOwned: true, processIsAlive: () => alive });
  f.internal.wireClient(client, f.internal.generation);
  client.emit('close', 1006, 'closing before process exit');
  t.mock.timers.tick(250);
  await Promise.resolve();
  assert.deepEqual(f.calls, []);
  alive = false;
  t.mock.timers.tick(1000);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  await f.runtime.dispose();
});


test('a refused stop with a live Side Chat preserves automatic recovery', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.liveSideChats.add('side-1');
  await assert.rejects(f.runtime.stop(), /End all Side Chats/);
  f.internal.managedChildExited(f.child, 1, null, true);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  await f.runtime.dispose();
});

test('an explicit restart resets the automatic recovery retry budget', async () => {
  const f = fixture();
  f.internal.recoveryAttempts.push(Date.now(), Date.now(), Date.now());
  f.internal.startInternal = async (): Promise<void> => { f.calls.push('start'); };
  await f.runtime.restart();
  assert.equal(f.internal.recoveryAttempts.length, 0);
  await f.runtime.dispose();
});

test('a refused metadata stop preserves an already scheduled recovery', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.metadataOperations = 1;
  f.internal.managedChildExited(f.child, 1, null, true);
  await assert.rejects(f.runtime.stop(), /metadata operation/);
  assert.equal(f.internal.desiredRunning, true);
  assert.notEqual(f.internal.recoveryTimer, undefined);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  f.internal.metadataOperations = 0;
  await f.runtime.dispose();
});

test('automatic recovery verifies the fixed draft mode before publishing connected', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  let release!: () => void;
  let entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  f.internal.restoreDraftSessionModeIfNeeded = async (): Promise<void> => {
    f.calls.push('mode');
    entered();
    await new Promise<void>(resolve => { release = resolve; });
  };
  f.internal.managedChildExited(f.child, 1, null, true);
  t.mock.timers.tick(250);
  await started;
  assert.equal(f.runtime.connectionState.status, 'restarting');
  release();
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore', 'mode']);
  assert.equal(f.runtime.connectionState.status, 'connected');
  await f.runtime.dispose();
});

test('a reconnect-only waiter cannot accept a replacement automatic recovery', async () => {
  const f = fixture();
  f.internal.automaticRecovery = Promise.resolve();
  f.internal.liveConnectionRecovered = false;
  await assert.rejects(f.runtime.start({ reconnectOnly: true }), /exact transcript/);
  f.internal.automaticRecovery = undefined;
  await f.runtime.dispose();
});


test('a refused metadata restart preserves an already scheduled recovery and its budget', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.recoveryAttempts.push(Date.now());
  f.internal.metadataOperations = 1;
  f.internal.managedChildExited(f.child, 1, null, true);
  await assert.rejects(f.runtime.restart(), /metadata operation/);
  assert.equal(f.internal.desiredRunning, true);
  assert.notEqual(f.internal.recoveryTimer, undefined);
  assert.equal(f.internal.recoveryAttempts.length, 1);
  t.mock.timers.tick(500);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  f.internal.metadataOperations = 0;
  await f.runtime.dispose();
});


test('start during recovery backoff restores the exact owner before becoming ready', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.managedChildExited(f.child, 1, null, true);
  await f.runtime.start();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  assert.equal(f.runtime.needsSessionResume, false);
  assert.equal(f.runtime.connectionState.status, 'connected');
  t.mock.timers.tick(1000);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  await f.runtime.dispose();
});

test('a reconnect-only backoff probe rejects replacement without canceling recovery', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.managedChildExited(f.child, 1, null, true);
  await assert.rejects(f.runtime.start({ reconnectOnly: true }), /exact transcript/);
  assert.notEqual(f.internal.recoveryTimer, undefined);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  assert.equal(f.runtime.connectionState.status, 'connected');
  await f.runtime.dispose();
});


test('automatic recovery never starts a replacement for history without an exact source', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.ownedTranscriptPath = undefined;
  f.internal.managedChildExited(f.child, 1, null, true);
  for (const delay of [250, 500, 1000]) {
    t.mock.timers.tick(delay);
    await f.runtime.waitForLifecycle();
  }
  assert.deepEqual(f.calls, []);
  assert.equal(f.internal.recoveryAttempts.length, 3);
  assert.equal(f.runtime.connectionState.status, 'error');
  await assert.rejects(SessionRuntime.prototype.restoreOwnedSessionIfNeeded.call(f.runtime), /exact transcript/);
  await f.runtime.dispose();
});
