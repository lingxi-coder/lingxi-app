import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
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

function transportFixture(t: TestContext, external = false) {
  const f = fixture();
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-compact-reconnect-'));
  const lockfile = join(directory, 'bridge.lock');
  writeFileSync(lockfile, JSON.stringify({ pid: process.pid, workspaceFolders: ['/workspace'],
    ideName: 'LingXi-Bridge', transport: 'ws', runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef' }));
  const sent: string[] = [];
  const previousClient = Object.assign(new EventEmitter(), { sendCommand() {}, close() {} });
  const recoveredClient = Object.assign(new EventEmitter(), {
    sendCommand() {}, close() {},
    sendPrompt: (text: string) => { sent.push(text); },
    requestRuntimeSnapshot: async (accept: (events: any[]) => void) => {
      const events = [
        { type: 'session_agent_list', session_id: f.runtime.sessionId, agents: [] },
        { type: 'coordinator_status', active_workers: 0 },
        { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot' },
      ];
      accept(events);
      return events;
    },
  });
  Object.assign(f.child, { pid: process.pid });
  Object.assign(f.internal, {
    ...(external ? { child: null, adoptedPid: process.pid, adoptedProcessOwned: false } : {}),
    activeWorkspace: '/workspace', activeWorkspaceTrusted: true,
    connectionLockfilePath: lockfile, processIsAlive: () => true,
    connectBridgeClient: async (path: string, generation: number) => {
      assert.equal(path, lockfile);
      f.calls.push('reconnect');
      f.internal.wireClient(recoveredClient, generation);
      f.internal.setState({ status: 'connecting' });
    },
  });
  f.internal.wireClient(previousClient, f.internal.generation);
  t.after(async () => { await f.runtime.dispose(); rmSync(directory, { recursive: true, force: true }); });
  return { ...f, previousClient, recoveredClient, sent };
}

test('a compact transport failure reconnects the same process and permits the next message without replay', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const external of [false, true]) {
    const f = transportFixture(t, external);
    f.internal.activeTurn = true;
    f.internal.liveSideChats.add('side-1');
    const source = f.internal.ownedTranscriptPath;
    f.previousClient.emit('event', { type: 'compaction_status', phase: 'summarizing' });
    f.previousClient.emit('close', 1006, 'connection lost during compaction');
    assert.equal(f.runtime.connectionState.status, 'disconnected');
    t.mock.timers.tick(250);
    await f.internal.restartChain;
    await f.runtime.waitForLifecycle();
    assert.deepEqual(f.calls, ['reconnect'], 'transport recovery must not stop, spawn or resume a process');
    assert.equal(f.runtime.connectionState.status, 'connected');
    assert.equal(f.runtime.recoveredLiveConnection, true);
    assert.equal(f.runtime.turnActive, false);
    assert.equal(f.internal.ownedTranscriptPath, source);
    assert.equal(f.internal.liveSideChats.has('side-1'), true);
    assert.deepEqual(f.sent, [], 'the compact operation and interrupted prompt are never replayed');
    await f.runtime.sendPrompt('new message after compact failure', [], 77);
    assert.deepEqual(f.sent, ['new message after compact failure']);
    f.previousClient.emit('close', 1006, 'stale close');
    t.mock.timers.tick(10_000);
    await f.runtime.waitForLifecycle();
    assert.deepEqual(f.calls, ['reconnect']);
  }
});

test('failed socket recovery stays reconnect-only and stops after three attempts', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t, true);
  f.internal.recoverConnection = async (only: boolean) => {
    assert.equal(only, true);
    f.calls.push('reconnect');
    throw new Error('socket unavailable');
  };
  f.previousClient.emit('close', 1006, 'peer closed');
  for (const delay of [250, 500, 1000]) {
    t.mock.timers.tick(delay);
    await f.runtime.waitForLifecycle();
  }
  assert.deepEqual(f.calls, ['reconnect', 'reconnect', 'reconnect']);
  assert.equal(f.runtime.connectionState.status, 'error');
  assert.deepEqual(f.sent, []);
});

test('a socket error before close still reconnects an established process', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t);
  f.previousClient.emit('error', new Error('socket reset'));
  assert.equal(f.runtime.connectionState.status, 'error');
  f.previousClient.emit('close', 1006, 'socket reset');
  t.mock.timers.tick(250);
  await f.internal.restartChain;
  assert.deepEqual(f.calls, ['reconnect']);
  assert.equal(f.runtime.connectionState.status, 'connected');
});

test('a failed initial handshake never schedules transport recovery', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t);
  f.internal.setState({ status: 'connecting' });
  f.previousClient.emit('error', new Error('hello rejected'));
  f.previousClient.emit('close', 1006, 'hello rejected');
  t.mock.timers.tick(10_000);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, []);
});

test('an owned detached peer that exits during a socket retry restores its exact source', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t, true);
  let alive = true;
  f.internal.adoptedProcessOwned = true;
  f.internal.processIsAlive = () => alive;
  f.internal.connectBridgeClient = async () => {
    f.calls.push('reconnect');
    throw new Error('socket handshake failed');
  };
  f.previousClient.emit('close', 1006, 'peer closed');
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  alive = false;
  t.mock.timers.tick(500);
  await f.internal.restartChain;
  assert.deepEqual(f.calls, ['reconnect', 'stop', 'start', 'restore']);
  assert.equal(f.runtime.connectionState.status, 'connected');
  assert.equal(f.internal.ownedTranscriptPath, '/workspace/original.jsonl');
});

test('an external peer that exits is never replaced by socket recovery', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t, true);
  f.internal.processIsAlive = () => false;
  f.previousClient.emit('close', 1006, 'external peer exited');
  for (const delay of [250, 500, 1000]) {
    t.mock.timers.tick(delay);
    await f.runtime.waitForLifecycle();
  }
  assert.deepEqual(f.calls, []);
  assert.equal(f.runtime.connectionState.status, 'error');
});

test('managed process exit supersedes a queued socket reconnect', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t);
  f.previousClient.emit('close', 1006, 'peer closed');
  f.internal.managedChildExited(f.child, 1, null, true);
  t.mock.timers.tick(250);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore']);
  assert.equal(f.runtime.connectionState.status, 'connected');
});

test('managed process exit during a socket retry retains the queued process recovery', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = transportFixture(t);
  let reject!: (error: Error) => void;
  f.internal.recoverConnection = async () => {
    f.calls.push('reconnect');
    await new Promise<void>((_resolve, no) => { reject = no; });
  };
  f.previousClient.emit('close', 1006, 'peer closed');
  t.mock.timers.tick(250);
  for (let i = 0; i < 10 && !reject; i++) await Promise.resolve();
  assert.equal(typeof reject, 'function');
  f.internal.managedChildExited(f.child, 1, null, true);
  reject(new Error('engine exited'));
  await f.runtime.waitForLifecycle();
  t.mock.timers.tick(500);
  await f.runtime.waitForLifecycle();
  assert.deepEqual(f.calls, ['reconnect', 'stop', 'start', 'restore']);
  assert.equal(f.runtime.connectionState.status, 'connected');
});

test('unexpected managed exit restores once without replaying foreground or Side Chat work', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  f.internal.activeTurn = true;
  f.internal.liveSideChats.add('side-1');
  f.internal.pendingSideCreates.add('create-1');
  f.internal.managedChildExited(f.child, 1, null, true);
  f.internal.managedChildExited(f.child, 1, null, true);
  assert.equal(f.internal.activeTurn, false);
  assert.equal(f.internal.liveSideChats.size, 1);
  assert.equal(f.events.filter(e => e.type === 'turn_ended').length, 1);
  assert.deepEqual(f.events.find(e => e.type === 'turn_ended').outcome, { type: 'cancelled' });
  assert.equal(f.events.filter(e => e.type === 'side_chat_ended').length, 0);
  assert.equal(f.events.find(e => e.type === 'side_chat_error').request_id, 'create-1');
  assert.match(f.events.find(e => e.type === 'system_notice').message, /Side Chats are being restored/);
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
test('automatic recovery publishes connected only after Side Chat restoration settles', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture();
  let finish!: () => void;
  const gate = new Promise<void>(resolve => { finish = resolve; });
  f.internal.sideChatRecovery.restore = async (): Promise<void> => { f.calls.push('sides'); await gate; };
  f.internal.managedChildExited(f.child, 1, null, true);
  t.mock.timers.tick(250);
  for (let i = 0; i < 15; i++) await Promise.resolve();
  assert.deepEqual(f.calls, ['stop', 'start', 'restore', 'sides']);
  assert.notEqual(f.runtime.connectionState.status, 'connected');
  const joined = f.runtime.start();
  finish();
  await joined;
  assert.equal(f.runtime.connectionState.status, 'connected');
  await f.runtime.dispose();
});
