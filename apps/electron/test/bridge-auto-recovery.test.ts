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
  Object.assign(internal, { child, desiredRunning: true, state: { status: 'connected' }, sessionHasHistory: true });
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
