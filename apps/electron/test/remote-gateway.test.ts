import assert from 'node:assert/strict';
import { test } from 'node:test';

import { RemoteRpcError } from '@lingxi/bridge-client/remote';

import { LIVE_BATCH_MS } from '../src/main/remoteControl/sessionAttachment.js';
import { LIST_DEBOUNCE_MS, SessionGateway } from '../src/main/remoteControl/sessionGateway.js';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands.js';
import { RemotePush } from '../src/main/remoteControl/pushRequests.js';
import {
  FakeRuntime,
  LEASE_GRACE_MS,
  RecordingChannel,
  SESSION_A,
  SESSION_B,
  SESSION_C,
  SESSION_D,
  flush,
  gatewayFixture,
  permissionRequest,
  questionRequest,
  rpcCode,
  viewer,
} from './remote-session-helpers.js';

async function attached(options: Parameters<typeof gatewayFixture>[0] = {}, sessionId = SESSION_A) {
  const f = gatewayFixture(options);
  f.enable(sessionId);
  const runtime = f.runtimes.add(new FakeRuntime(sessionId));
  runtime.resume();
  await f.call('session.attach', { session_id: sessionId });
  return { f, runtime };
}

test('only a declared native media handler registers a route and receives correlated offer/control replies', async () => {
  const f = gatewayFixture();
  f.phone = viewer({
    account_uid: 'account-1',
    media_capabilities: {
      version: 1,
      modes: ['audio', 'video', 'audio_video'],
      source_kinds: ['microphone', 'camera'],
    },
  });
  f.enable(SESSION_A);
  const runtime = f.runtimes.add(new FakeRuntime(SESSION_A));
  runtime.resume();
  await f.gateway.handle(f.phone, 'session.attach', { session_id: SESSION_A }, f.channel);

  const registration = runtime.callsOf('registerMediaConnection')[0];
  assert.ok(registration);
  const routeId = registration[0] as string;
  assert.match(routeId, /^[0-9a-f-]{36}$/iu);
  assert.deepEqual(registration[3], ['microphone', 'camera'], 'the route does not infer screen sharing');

  const mediaSessionRef = 'media_session_gateway_test';
  runtime.emitMediaConnectionRequest({
    request_id: 'AAAAAAAAAAAAAAAAAAAAAA',
    connection_id: routeId,
    session_id: SESSION_A,
    media_session_ref: { id: mediaSessionRef },
    operation_id: { id: 'operation-gateway-1', generation: 7, service_epoch: 1 },
    owner: { type: 'session', session_id: SESSION_A },
    mode: 'audio',
    intent: 'conversation',
    audio_source: { id: `remote_${routeId}_microphone` },
    video_source: null,
    description: { kind: 'offer', sdp: 'v=0\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n' },
  });
  await flush();
  assert.equal(runtime.mediaConnectionResponses[0]?.description?.kind, 'answer');
  assert.equal(runtime.mediaConnectionResponses[0]?.error, undefined);

  runtime.emitMediaControlRequest({
    request_id: 'BBBBBBBBBBBBBBBBBBBBBB',
    connection_id: routeId,
    media_session_ref: { id: mediaSessionRef },
    generation: 7,
    action: { type: 'flush_audio', target_sequence: 2, target_sample_frames: 1_920 },
  });
  await flush();
  assert.equal(runtime.mediaControlResponses[0]?.receipt?.accepted_sample_frames, 0);
  assert.equal(runtime.mediaControlResponses[0]?.receipt?.played_sample_frames, 1_920);

  await f.gateway.handle(f.phone, 'session.detach', { session_id: SESSION_A }, f.channel);
  assert.deepEqual(runtime.callsOf('unregisterMediaConnection').map(([id]) => id), [routeId]);
  f.gateway.dispose();
});

// ─────────────────────────────────────────────────────────────────────────────
// Which sessions, and the error codes
// ─────────────────────────────────────────────────────────────────────────────

test('sessions.list shows only the sessions Remote Control is on for, never an archived one', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  f.enable(SESSION_B, '/work/other');
  f.enable(SESSION_C);
  f.settings.archived.add(SESSION_C);
  f.titles.set(SESSION_A, 'Fix the build');
  const { sessions } = await f.call('sessions.list', {});
  assert.deepEqual(sessions.map((item: any) => item.session_id).sort(), [SESSION_A, SESSION_B].sort());
  const a = sessions.find((item: any) => item.session_id === SESSION_A);
  assert.deepEqual(a, { session_id: SESSION_A, title: 'Fix the build', project_label: 'project-one', status: 'dormant', viewers: 0, model: 'saved-model' });
  assert.equal(sessions.find((item: any) => item.session_id === SESSION_B).project_label, 'other');
  assert.equal(sessions.find((item: any) => item.session_id === SESSION_B).title, 'Untitled session');
});

test('a session unshared while its title loads is absent from the eventual list', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let finishTitle!: (title: string) => void;
  const gateway = new SessionGateway({
    runtimes: f.runtimes,
    policy: f.policy,
    commands: new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => undefined, clock: f.clock }),
    titleOf: () => new Promise<string>((resolve) => { finishTitle = resolve; }),
    clock: f.clock,
    diagnostics: () => undefined,
  });
  const pending = gateway.handle(f.phone, 'sessions.list', {}, f.channel);
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  finishTitle('withdrawn session title');
  assert.deepEqual(await pending, { sessions: [] });
  gateway.dispose();
  f.gateway.dispose();
});

test('the status of a session: dormant, idle, running, or requires action with the first pending kind', async () => {
  const f = gatewayFixture();
  for (const id of [SESSION_A, SESSION_B, SESSION_C]) f.enable(id);
  const b = f.runtimes.add(new FakeRuntime(SESSION_B));
  const c = f.runtimes.add(new FakeRuntime(SESSION_C));
  b.resume(); c.resume();
  c.turnActive = true;
  const byId = async () => Object.fromEntries((await f.call('sessions.list', {})).sessions.map((item: any) => [item.session_id, item]));
  let list = await byId();
  assert.equal(list[SESSION_A].status, 'dormant');
  assert.equal(list[SESSION_B].status, 'idle');
  assert.equal(list[SESSION_C].status, 'running');
  c.askQuestion(questionRequest(3));
  list = await byId();
  assert.equal(list[SESSION_C].status, 'requires_action');
  assert.equal(list[SESSION_C].pending_action, 'question');
  c.askPermission(permissionRequest(4));
  assert.equal((await byId())[SESSION_C].pending_action, 'permission');
  b.connectionState = { status: 'disconnected' };
  assert.equal((await byId())[SESSION_B].status, 'dormant');
});

test('error codes: an unknown session, a session that is off, a malformed request, an unknown method', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_D })), 'unknown_session');
  // A session that was on and was switched off is a different answer from one nobody knows.
  f.enable(SESSION_B);
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_B }, false);
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_B })), 'not_remote_enabled');
  assert.equal(await rpcCode(f.call('session.attach', {})), 'invalid_params');
  assert.equal(await rpcCode(f.call('session.attach', { session_id: '../../etc' })), 'invalid_params');
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_A, resume: 'x' })), 'invalid_params');
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_A, resume: { epoch: 1, after_seq: 0 } })), 'invalid_params');
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_A, resume: { epoch: 'e', after_seq: -1 } })), 'invalid_params');
  assert.equal(await rpcCode(f.call('no.such.method', {})), 'invalid_params');
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'r' })), 'invalid_params', 'prompting needs an attachment first');
});

test('a session that cannot be opened is an internal error, and the open is tried again next time', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  f.runtimes.failOpen = new Error('bridge crashed at /Users/someone/secret/path');
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_A })), 'internal');
  assert.ok(!f.diagnostics.some((line) => line.includes('undefined')));
  f.runtimes.failOpen = undefined;
  const result = await f.call('session.attach', { session_id: SESSION_A });
  assert.equal(result.mode, 'snapshot');
});

test('capacity: beyond three remote-opened runtimes the fourth attach says capacity', async () => {
  const f = gatewayFixture();
  f.runtimes.capacity = 3;
  const ids = [SESSION_A, SESSION_B, SESSION_C, SESSION_D];
  for (const id of ids) f.enable(id);
  for (const id of ids.slice(0, 3)) await f.call('session.attach', { session_id: id });
  assert.equal(await rpcCode(f.call('session.attach', { session_id: SESSION_D })), 'capacity');
  // Letting go of one (after the grace) makes room.
  await f.call('session.detach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
});

test('one device may not attach to an unbounded number of sessions', async () => {
  const f = gatewayFixture();
  const ids = Array.from({ length: 9 }, (_, i) => `12121212-3434-4567-8899-0000000000${String(i).padStart(2, '0')}`);
  for (const id of ids) { f.enable(id); f.runtimes.add(new FakeRuntime(id)).resume(); }
  const results: string[] = [];
  for (const id of ids) results.push(await f.call('session.attach', { session_id: id }).then(() => 'ok', (error) => (error as RemoteRpcError).code));
  assert.ok(results.includes('capacity'));
  assert.equal(results[0], 'ok');
});

test('concurrent attaches reserve the per-viewer capacity before opening runtimes', async () => {
  const f = gatewayFixture();
  const ids = Array.from({ length: 9 }, (_, i) => `12121212-3434-4567-8899-0000000000${String(i).padStart(2, '0')}`);
  for (const id of ids) { f.enable(id); f.runtimes.add(new FakeRuntime(id)).resume(); }
  const results = await Promise.all(ids.map((id) => f.call('session.attach', { session_id: id })
    .then(() => 'ok', (error) => (error as RemoteRpcError).code)));
  assert.equal(results.filter((result) => result === 'ok').length, 8);
  assert.equal(results.filter((result) => result === 'capacity').length, 1);
  assert.equal(f.runtimes.opened.length, 8, 'rejected attaches never open a runtime');
  f.gateway.dispose();
});

test('failed opens give back pending attachment capacity', async () => {
  const f = gatewayFixture();
  const ids = Array.from({ length: 9 }, (_, i) => `12121212-3434-4567-8899-0000000000${String(i).padStart(2, '0')}`);
  for (const id of ids) f.enable(id);
  f.runtimes.failOpen = new Error('unavailable');
  assert.deepEqual(await Promise.all(ids.slice(0, 8).map((id) => rpcCode(f.call('session.attach', { session_id: id })))), Array(8).fill('internal'));
  f.runtimes.failOpen = undefined;
  assert.equal((await f.call('session.attach', { session_id: ids[8] })).mode, 'snapshot');
  f.gateway.dispose();
});

// ─────────────────────────────────────────────────────────────────────────────
// Leases
// ─────────────────────────────────────────────────────────────────────────────

test('attaching a dormant session opens it once for remote, and two viewers share the lease', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  const other = { viewer: viewer({ device_id: 'device-2', conn_id: 'conn-2' }), channel: new RecordingChannel() };
  const [first, second] = await Promise.all([
    f.call('session.attach', { session_id: SESSION_A }),
    f.call('session.attach', { session_id: SESSION_A }, other),
  ]);
  assert.equal(first.mode, 'snapshot');
  assert.equal(second.mode, 'snapshot');
  assert.equal(f.runtimes.opened.length, 1);
  assert.deepEqual(f.runtimes.opened[0], { projectPath: '/work/project-one', sessionId: SESSION_A });
  assert.equal(f.gateway.attachedViewers(SESSION_A), 2);
});

test('the lease is released two minutes after the last viewer leaves, and not before', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  const other = { viewer: viewer({ device_id: 'device-2', conn_id: 'conn-2' }), channel: new RecordingChannel() };
  await f.call('session.attach', { session_id: SESSION_A });
  await f.call('session.attach', { session_id: SESSION_A }, other);
  await f.call('session.detach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS * 2);
  assert.deepEqual(f.runtimes.releases, [], 'one viewer is still attached');
  f.gateway.viewerClosed(other.viewer);
  f.clock.advance(LEASE_GRACE_MS - 1);
  assert.deepEqual(f.runtimes.releases, []);
  f.clock.advance(1);
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
  assert.equal(f.gateway.attachedViewers(), 0);
});

test('coming back within the grace keeps the same lease; attaching again does not leak a share', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  await f.call('session.attach', { session_id: SESSION_A });
  await f.call('session.attach', { session_id: SESSION_A }); // replaces the attachment
  await f.call('session.detach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS - 1000);
  await f.call('session.attach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [], 'still attached');
  await f.call('session.detach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [SESSION_A], 'released exactly once');
  assert.equal(f.runtimes.opened.length, 1);
});

test('switching a session off closes its viewers at once and gives the session back', async () => {
  const { f, runtime } = await attached();
  f.channel.clear();
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  assert.equal(f.channel.of('session.closed')[0].reason, 'remote_disabled');
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
  assert.equal(runtime.observers.size, 1, 'only the gateway\'s own watcher remains');
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'r' })), 'not_remote_enabled');
});

test('a session switched off while it was waking up is not attached', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let wake!: () => void;
  f.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const pending = f.call('session.attach', { session_id: SESSION_A });
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  wake();
  assert.equal(await rpcCode(pending), 'not_remote_enabled');
  assert.equal(f.gateway.attachedViewers(), 0);
});

test('detaching while a session wakes cancels the attachment and releases its lease', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let wake!: () => void;
  f.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const pending = rpcCode(f.call('session.attach', { session_id: SESSION_A }));
  await f.call('session.detach', { session_id: SESSION_A });
  wake();
  assert.equal(await pending, 'invalid_params');
  assert.equal(f.gateway.attachedViewers(), 0);
  assert.deepEqual(f.channel.of('session.snapshot'), [], 'the cancelled viewer receives no snapshot');
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
});

test('the newest concurrent attach replaces an older pending attach without leaking a lease', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let wake!: () => void;
  f.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const first = rpcCode(f.call('session.attach', { session_id: SESSION_A }));
  const second = f.call('session.attach', { session_id: SESSION_A });
  wake();
  assert.equal(await first, 'invalid_params');
  assert.equal((await second).mode, 'snapshot');
  assert.equal(f.gateway.attachedViewers(), 1);
  await f.call('session.detach', { session_id: SESSION_A });
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
});

test('withdrawing and restoring sharing does not resurrect an earlier pending attach', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let wake!: () => void;
  f.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const pending = rpcCode(f.call('session.attach', { session_id: SESSION_A }));
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  f.enable(SESSION_A);
  wake();
  assert.equal(await pending, 'invalid_params');
  assert.equal(f.gateway.attachedViewers(), 0);
  assert.deepEqual(f.channel.of('session.snapshot'), []);
});

test('a disposed runtime ends the attachment and the next attach opens the session again', async () => {
  const { f, runtime } = await attached();
  f.channel.clear();
  runtime.dispose();
  assert.equal(f.channel.of('session.closed')[0].reason, 'ended');
  assert.equal(f.gateway.attachedViewers(SESSION_A), 0);
  f.runtimes.runtimes.delete(SESSION_A);
  await f.call('session.attach', { session_id: SESSION_A });
  assert.equal(f.runtimes.opened.length, 2);
});

test('disposed runtime observers are released instead of retained until gateway shutdown', () => {
  const f = gatewayFixture();
  let unsubscriptions = 0;
  class TrackedRuntime extends FakeRuntime {
    override observeRemote(observer: Parameters<FakeRuntime['observeRemote']>[0]): () => void {
      const stop = super.observeRemote(observer);
      return () => { unsubscriptions += 1; stop(); };
    }
  }
  const runtime = f.runtimes.add(new TrackedRuntime(SESSION_A));
  runtime.dispose();
  assert.equal(unsubscriptions, 1, 'the gateway drops its closure that retains the disposed runtime');
  f.gateway.dispose();
  assert.equal(unsubscriptions, 1, 'a disposed runtime is no longer in the gateway cleanup set');
});

test('dispose closes everything and releases every lease', async () => {
  const { f } = await attached();
  f.gateway.dispose();
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
  assert.equal(await rpcCode(f.call('sessions.list', {})), 'internal');
});

test('disposing during a pending open releases the eventual lease without a grace timer', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  let wake!: () => void;
  f.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const pending = rpcCode(f.call('session.attach', { session_id: SESSION_A }));
  f.gateway.dispose();
  wake();
  assert.equal(await pending, 'internal');
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
  assert.equal(f.gateway.attachedViewers(), 0);
  assert.deepEqual(f.channel.of('session.snapshot'), []);
  f.clock.advance(LEASE_GRACE_MS * 2);
  assert.deepEqual(f.runtimes.releases, [SESSION_A], 'the lease is released exactly once');
});

// ─────────────────────────────────────────────────────────────────────────────
// The session list, watched
// ─────────────────────────────────────────────────────────────────────────────

test('sessions.watch pushes the list when something visible changes, debounced, and not when nothing did', async () => {
  const { f, runtime } = await attached();
  await f.call('sessions.watch', {});
  f.channel.clear();
  runtime.turnActive = true;
  runtime.push({ type: 'turn_started', turn_id: 1 } as any);
  runtime.push({ type: 'turn_ended', outcome: { type: 'completed' }, cost: {} } as any);
  f.clock.advance(LIST_DEBOUNCE_MS);
  await flush();
  const changes = f.channel.of('sessions.changed');
  assert.equal(changes.length, 1);
  assert.equal(changes[0].sessions[0].status, 'running');
  f.channel.clear();
  runtime.push({ type: 'text_delta', text: 'noise' } as any);
  f.clock.advance(LIST_DEBOUNCE_MS * 3);
  await flush();
  assert.equal(f.channel.of('sessions.changed').length, 0);
});

test('the viewer count in the list follows attaching and detaching', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  const watcher = { viewer: viewer({ device_id: 'device-2', conn_id: 'conn-2' }), channel: new RecordingChannel() };
  await f.call('sessions.watch', {}, watcher);
  await f.call('session.attach', { session_id: SESSION_A });
  f.clock.advance(LIST_DEBOUNCE_MS);
  await flush();
  assert.equal(watcher.channel.of('sessions.changed').at(-1).sessions[0].viewers, 1);
  f.gateway.viewerClosed(f.phone);
  f.clock.advance(LIST_DEBOUNCE_MS);
  await flush();
  assert.equal(watcher.channel.of('sessions.changed').at(-1).sessions[0].viewers, 0);
});

test('a session switched on or off reaches watchers', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  await f.call('sessions.watch', {});
  f.channel.clear();
  f.enable(SESSION_B);
  f.clock.advance(LIST_DEBOUNCE_MS);
  await flush();
  assert.equal(f.channel.of('sessions.changed')[0].sessions.length, 2);
});

// ─────────────────────────────────────────────────────────────────────────────
// Answers: first answer wins
// ─────────────────────────────────────────────────────────────────────────────

test('permission.respond: the first answer wins, and the second gets not_pending', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(5));
  await f.call('permission.respond', { session_id: SESSION_A, request_id: 5, decision: { type: 'allow_once' } });
  assert.deepEqual(runtime.callsOf('approvePermission'), [[5, { type: 'allow_once' }]]);
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 5, decision: { type: 'allow_once' } })), 'not_pending');
});

test('permission.respond: the desktop answered first, so the viewer gets not_pending and nothing is sent twice', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(5));
  runtime.denyPermission(5); // the desktop window
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 5, decision: { type: 'allow_once' } })), 'not_pending');
  assert.equal(runtime.callsOf('approvePermission').length, 0);
});

test('permission.respond: a desktop answer that lands between the check and the call is still not_pending', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(5));
  const approve = runtime.approvePermission.bind(runtime);
  runtime.approvePermission = (id, response) => { runtime.settle(id); approve(id, response); }; // the desktop wins the race
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 5, decision: { type: 'allow_once' } })), 'not_pending');
});

test('permission.respond: deny, and the same for questions (first answer wins, and cancel)', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(5));
  await f.call('permission.respond', { session_id: SESSION_A, request_id: 5, decision: { type: 'deny' } });
  assert.equal(runtime.callsOf('denyPermission').length, 1);
  runtime.askQuestion(questionRequest(6));
  await f.call('question.answer', { session_id: SESSION_A, request_id: 6, answers: { 'Which one?': 'A' } });
  assert.deepEqual(runtime.callsOf('answerAskUserQuestion'), [[6, { 'Which one?': 'A' }]]);
  assert.equal(await rpcCode(f.call('question.answer', { session_id: SESSION_A, request_id: 6, answers: { 'Which one?': 'B' } })), 'not_pending');
  runtime.askQuestion(questionRequest(7));
  runtime.settle(7); // the desktop answered
  assert.equal(await rpcCode(f.call('question.cancel', { session_id: SESSION_A, request_id: 7 })), 'not_pending');
  runtime.askQuestion(questionRequest(8));
  await f.call('question.cancel', { session_id: SESSION_A, request_id: 8 });
  assert.equal(runtime.callsOf('cancelAskUserQuestion').length, 1);
});

test('bypassPermissions can never be approved from a viewer, not even with allow_once', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(9, { type: 'bypass_permissions_mode' }));
  for (const type of ['allow_once', 'allow_always', 'allow_auto']) {
    assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 9, decision: { type } })), 'forbidden', type);
  }
  assert.equal(runtime.callsOf('approvePermission').length, 0);
  assert.equal(runtime.permissions.has(9), true, 'still waiting for the desktop');
  await f.call('permission.respond', { session_id: SESSION_A, request_id: 9, decision: { type: 'deny' } });
  assert.equal(runtime.permissions.has(9), false, 'a viewer may refuse it');
  assert.equal(await rpcCode(f.call('controls.set_permission_mode', { session_id: SESSION_A, mode: 'bypassPermissions' })), 'forbidden');
  assert.equal(runtime.callsOf('dispatchCommand').length, 0);
});

test('allow_always is refused by default and allowed once the setting says so', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(1));
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 1, decision: { type: 'allow_always' } })), 'forbidden');
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 1, decision: { type: 'allow_auto' } })), 'forbidden');
  assert.equal(runtime.permissions.has(1), true);
  f.settings.allowAlways = true;
  await f.call('permission.respond', { session_id: SESSION_A, request_id: 1, decision: { type: 'allow_always' } });
  assert.deepEqual(runtime.callsOf('approvePermission'), [[1, { type: 'allow_always' }]]);
});

test('malformed answers are invalid_params', async () => {
  const { f, runtime } = await attached();
  runtime.askPermission(permissionRequest(1));
  for (const decision of [undefined, 'allow_once', { type: 'allow_everything' }, { type: 'allow_once', extra: 1 }, null]) {
    assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: 1, decision })), 'invalid_params', JSON.stringify(decision));
  }
  assert.equal(await rpcCode(f.call('permission.respond', { session_id: SESSION_A, request_id: -1, decision: { type: 'deny' } })), 'invalid_params');
  assert.equal(await rpcCode(f.call('question.answer', { session_id: SESSION_A, request_id: 1, answers: { q: 5 } })), 'invalid_params');
});

// ─────────────────────────────────────────────────────────────────────────────
// Prompts
// ─────────────────────────────────────────────────────────────────────────────

test('prompt.send sends the prompt as a device, journals it, echoes it to the window, and returns the host-assigned turn id', async () => {
  const { f, runtime } = await attached({ turnIds: [4242] });
  f.channel.clear();
  const result = await f.call('prompt.send', { session_id: SESSION_A, text: 'run the tests', client_request_id: 'req-1' });
  assert.deepEqual(result, { turn_id: 4242 });
  assert.deepEqual(runtime.callsOf('sendPrompt'), [['run the tests', [], 4242, { origin: 'device', deviceName: 'Test phone' }]]);
  assert.deepEqual(f.echoes, [{ sessionId: SESSION_A, turnId: 4242, text: 'run the tests', images: [], origin: 'device', deviceName: 'Test phone' }]);
  f.clock.advance(LIVE_BATCH_MS);
  const [record] = f.channel.records();
  assert.deepEqual(record!.record, { type: 'prompt', turn_id: 4242, text: 'run the tests', origin: 'device', device_name: 'Test phone' });
});

test('a retry with the same client_request_id gets the first answer and sends nothing again', async () => {
  const { f, runtime } = await attached();
  const params = { session_id: SESSION_A, text: 'once', client_request_id: 'req-1' };
  const first = await f.call('prompt.send', params);
  const retry = await f.call('prompt.send', params);
  assert.deepEqual(retry, first);
  assert.equal(runtime.callsOf('sendPrompt').length, 1);
  assert.equal(f.echoes.length, 1);
  // The same id from another device, or another id, is another prompt.
  const other = { viewer: viewer({ device_id: 'device-2', conn_id: 'conn-2' }), channel: new RecordingChannel() };
  await f.call('session.attach', { session_id: SESSION_A }, other);
  await f.call('prompt.send', params, other);
  await f.call('prompt.send', { ...params, client_request_id: 'req-2' });
  assert.equal(runtime.callsOf('sendPrompt').length, 3);
});

test('the idempotency window ends after five minutes, and a refused prompt can be sent again', async () => {
  const { f, runtime } = await attached();
  const params = { session_id: SESSION_A, text: 'x', client_request_id: 'req-1' };
  runtime.failSend = new Error('engine is busy');
  assert.equal(await rpcCode(f.call('prompt.send', params)), 'invalid_params');
  runtime.failSend = undefined;
  await f.call('prompt.send', params);
  assert.equal(runtime.callsOf('sendPrompt').length, 2);
  f.clock.advance(5 * 60_000 + 1);
  await f.call('prompt.send', params);
  assert.equal(runtime.callsOf('sendPrompt').length, 3);
});

test('prompt validation: client_request_id is required, text must be a string, images are limited', async () => {
  const { f } = await attached();
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x' })), 'invalid_params');
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 5, client_request_id: 'a' })), 'invalid_params');
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'a', images: Array(5).fill({ media_type: 'image/png', base64: 'AA==' }) })), 'invalid_params');
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'b', images: [{ media_type: 'image/png', base64: 'AAAA' }] })), 'invalid_params', 'not a PNG');
});

test('images in a prompt are journaled and served back through image.get', async () => {
  const { PNG_BASE64 } = await import('./remote-session-helpers.js');
  const { f, runtime } = await attached();
  f.channel.clear();
  await f.call('prompt.send', { session_id: SESSION_A, text: 'see', client_request_id: 'img', images: [{ media_type: 'image/png', base64: PNG_BASE64 }] });
  f.clock.advance(LIVE_BATCH_MS);
  const [record] = f.channel.records();
  const ref = (record!.record as any).images[0].ref;
  assert.deepEqual(await f.call('image.get', { ref }), { media_type: 'image/png', base64: PNG_BASE64 });
  assert.equal(f.echoes[0]!.images.length, 1);
  assert.equal(await rpcCode(f.call('image.get', { ref: ref.replace(':0', ':1') })), 'invalid_params');
  assert.equal(await rpcCode(f.call('image.get', { ref: 'garbage' })), 'invalid_params');
  runtime.dispose();
});

test('images are refused while a turn is running', async () => {
  const { PNG_BASE64 } = await import('./remote-session-helpers.js');
  const { f, runtime } = await attached();
  runtime.turnActive = true;
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'a', images: [{ media_type: 'image/png', base64: PNG_BASE64 }] })), 'invalid_params');
  await f.call('prompt.send', { session_id: SESSION_A, text: 'steer', client_request_id: 'b' });
});

test('image.get shrinks an oversized image with the codec, or says internal when it cannot', async () => {
  const big = Buffer.alloc(3 * 1024 * 1024, 7);
  const prompt = (f: Awaited<ReturnType<typeof attached>>['f'], runtime: FakeRuntime) => {
    const entry = runtime.journal('pic', { origin: 'desktop' }, [{ media_type: 'image/png', base64: big.toString('base64') }]);
    return `${SESSION_A}:p:${entry.sequence}:0`;
  };
  const withCodec = await attached({ codec: {
    size: () => ({ width: 4000, height: 2000 }),
    scale: (_bytes, width, height, encode) => { assert.equal(width, 1568); assert.equal(height, 784); return encode.type === 'png' ? new Uint8Array(3 * 1024 * 1024) : new Uint8Array(1000); },
  } });
  const result = await withCodec.f.call('image.get', { ref: prompt(withCodec.f, withCodec.runtime) });
  assert.equal(result.media_type, 'image/jpeg');
  assert.equal(Buffer.from(result.base64, 'base64').length, 1000);
  const without = await attached();
  assert.equal(await rpcCode(without.f.call('image.get', { ref: prompt(without.f, without.runtime) })), 'internal');
});

test('a slash command from a viewer: the allowed few run as commands, the rest are forbidden', async () => {
  const { f, runtime } = await attached({ turnIds: [11, 12, 13, 14] });
  assert.deepEqual(await f.call('prompt.send', { session_id: SESSION_A, text: '/compact keep tests', client_request_id: 'a' }), { turn_id: 11 });
  assert.deepEqual(runtime.callsOf('dispatchCommand')[0], [{ type: 'run_slash_command', raw: '/compact keep tests', images: [], turn_id: 11 }, { origin: 'device', deviceName: 'Test phone' }]);
  assert.equal(f.echoes.length, 1, '/compact shows a user row');
  await f.call('prompt.send', { session_id: SESSION_A, text: '/model other', client_request_id: 'b' });
  assert.equal(f.echoes.length, 1, '/model shows none');
  for (const text of ['/login', '/clear', '/mcp add x', '/permissions']) {
    assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text, client_request_id: text })), 'forbidden', text);
  }
  assert.equal(runtime.callsOf('sendPrompt').length, 0);
  assert.equal(runtime.callsOf('dispatchCommand').length, 2);
  // A path is not a slash command.
  await f.call('prompt.send', { session_id: SESSION_A, text: '/usr/bin/env is where?', client_request_id: 'c' });
  assert.equal(runtime.callsOf('sendPrompt').length, 1);
});

test('turn.cancel, controls and models.list reach the runtime; models are reference strings', async () => {
  const { f, runtime } = await attached();
  await f.call('turn.cancel', { session_id: SESSION_A, turn_id: 5 });
  assert.deepEqual(runtime.callsOf('cancelTurn'), [[5]]);
  await f.call('controls.set_model', { session_id: SESSION_A, model: 'model-two' });
  await f.call('controls.set_permission_mode', { session_id: SESSION_A, mode: 'plan' });
  await f.call('controls.set_reasoning', { session_id: SESSION_A, selection: { type: 'effort', level: 'high' } });
  assert.deepEqual(runtime.callsOf('dispatchCommand').map(([command]) => (command as any).type), ['set_model', 'set_permission_mode', 'set_reasoning_selection']);
  assert.equal(await rpcCode(f.call('controls.set_model', { session_id: SESSION_A, model: '' })), 'invalid_params');
  assert.equal(await rpcCode(f.call('controls.set_permission_mode', { session_id: SESSION_A, mode: 'whatever' })), 'invalid_params');
  const models = await f.call('models.list', { session_id: SESSION_A });
  assert.deepEqual(models.models, ['model-one', 'model-two']);
  assert.equal(models.current, 'model-one');
  assert.deepEqual((await f.call('models.list', {})).models, ['model-one', 'model-two'], 'without a session: any attached one');
});

test('a command to a session that is not connected is an internal error', async () => {
  const { f, runtime } = await attached();
  runtime.connectionState = { status: 'disconnected' };
  assert.equal(await rpcCode(f.call('prompt.send', { session_id: SESSION_A, text: 'x', client_request_id: 'a' })), 'internal');
  assert.equal(await rpcCode(f.call('turn.cancel', { session_id: SESSION_A })), 'internal');
});

test('session.history pages older messages; its bounds are checked', async () => {
  const { f, runtime } = await attached();
  runtime.resume(Array.from({ length: 30 }, (_, i) => ({ role: 'user', blocks: [{ type: 'text', text: `m${i}` }] }) as any));
  const page = await f.call('session.history', { session_id: SESSION_A, before_index: 20, limit: 10 });
  assert.equal(page.first_index, 10);
  assert.equal(page.has_more, true);
  assert.equal(page.messages[0].blocks[0].text, 'm10');
  for (const bad of [{ before_index: -1, limit: 5 }, { before_index: 5, limit: 0 }, { before_index: 5, limit: 1000 }, { before_index: 'x', limit: 5 }]) {
    assert.equal(await rpcCode(f.call('session.history', { session_id: SESSION_A, ...bad })), 'invalid_params');
  }
});

test('presence is a notification: it is remembered, never answered, and wrong shapes are ignored', async () => {
  const { f } = await attached();
  assert.equal(await f.call('session.presence', { session_id: SESSION_A, foreground: true }), undefined);
  assert.equal(f.gateway.foregroundViewers(SESSION_A), 1);
  await f.call('session.presence', { session_id: SESSION_A, foreground: false });
  assert.equal(f.gateway.foregroundViewers(SESSION_A), 0);
  await f.call('session.presence', { session_id: SESSION_A, foreground: 'yes' });
  await f.call('session.presence', null);
  await f.call('session.presence', { session_id: SESSION_B, foreground: true });
  assert.equal(f.gateway.foregroundViewers(SESSION_A), 0);
});

test('a viewer whose connection closed leaves nothing behind', async () => {
  const { f, runtime } = await attached();
  f.gateway.viewerClosed(f.phone);
  assert.equal(f.gateway.attachedViewers(), 0);
  assert.equal(runtime.observers.size, 1);
  f.gateway.viewerClosed(f.phone); // idempotent
  f.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(f.runtimes.releases, [SESSION_A]);
});

test('push presence: which sessions are on, who looks, which installations are attached', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  const phone = viewer({ push_installation_id: 'inst-phone' });
  const web = { viewer: viewer({ device_id: 'device-2', conn_id: 'conn-2' }), channel: new RecordingChannel() };
  await f.call('session.attach', { session_id: SESSION_A }, { viewer: phone, channel: new RecordingChannel() });
  await f.call('session.attach', { session_id: SESSION_A }, web);
  assert.equal(f.gateway.isEnabled(SESSION_A), true);
  assert.equal(f.gateway.isEnabled(SESSION_B), false);
  assert.deepEqual(f.gateway.attachedInstallationIds(SESSION_A), ['inst-phone']);
  assert.deepEqual(f.gateway.attachedInstallationIds(SESSION_B), []);
});

test('an attached phone moved to the background remains eligible for push', async () => {
  const f = gatewayFixture();
  f.enable(SESSION_A);
  const phone = { viewer: viewer({ push_installation_id: 'inst-phone' }), channel: new RecordingChannel() };
  await f.call('session.attach', { session_id: SESSION_A }, phone);
  await f.call('session.presence', { session_id: SESSION_A, foreground: true }, phone);
  const exclusions: string[][] = [];
  const push = new RemotePush({
    requester: { request: async (_kind, _session, exclude) => { exclusions.push([...exclude]); return true; } },
    presence: f.gateway, active: () => true, clock: f.clock,
  });
  push.notify('question', SESSION_A);
  assert.deepEqual(exclusions, [], 'an actively viewed session needs no push');
  await f.call('session.presence', { session_id: SESSION_A, foreground: false }, phone);
  assert.equal(f.gateway.attachedViewers(SESSION_A), 1, 'backgrounding does not close the connection');
  push.notify('question', SESSION_A);
  assert.deepEqual(exclusions, [[]], 'the background phone must not be excluded');
});
