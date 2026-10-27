import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';

import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager, RemoteCapacityError } from '../src/main/sessionRuntimeManager.js';
import { LIVE_BATCH_MS, SessionAttachment } from '../src/main/remoteControl/sessionAttachment.js';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands.js';
import { RecordingChannel, PNG_BASE64, SESSION_A, SESSION_B, SESSION_C, SESSION_D, newClock, permissionRequest, viewer } from './remote-session-helpers.js';
import type { RemoteObservation } from '../src/main/bridgeTypes.js';

class FakeClient extends EventEmitter {
  readonly sent: unknown[] = [];
  readonly prompts: Array<{ text: string; opts: any }> = [];
  readonly approved: unknown[] = [];
  readonly denied: number[] = [];
  sendCommand(command: unknown): void { this.sent.push(command); }
  sendPrompt(text: string, opts: unknown): void { this.prompts.push({ text, opts }); }
  approvePermission(id: number, response: unknown): void { this.approved.push([id, response]); }
  denyPermission(id: number): void { this.denied.push(id); }
  async requestPermissionScope(): Promise<{ background_owned: boolean }> { return { background_owned: false }; }
  emitEvent(event: unknown): void { this.emit('event', event); }
}

function connectedRuntime(sessionId = SESSION_A) {
  const client = new FakeClient();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).generation = 1;
  (runtime as any).client = client;
  (runtime as any).state = { status: 'connected' };
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).wireClient(client, 1);
  return { runtime, client };
}

test('a real runtime: a viewer attaches, prompts as a device, and sees the turn; the desktop sees the same journal', async () => {
  const { runtime, client } = connectedRuntime();
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [{ role: 'user', blocks: [{ type: 'text', text: 'earlier' }] }] });
  client.emitEvent({ type: 'model_changed', model: 'model-one' });

  const channel = new RecordingChannel();
  const clock = newClock();
  const echoes: unknown[] = [];
  const commands = new RemoteCommands({ allowAlwaysAllow: () => false, echo: (echo) => echoes.push(echo), clock, randomTurnId: () => 777 });
  const attachment = new SessionAttachment({ runtime, channel, clock });
  const result = attachment.attach(undefined);
  assert.equal(result.mode, 'snapshot');
  assert.equal(result.epoch, runtime.remoteView().epoch);
  const snapshot = channel.of('session.snapshot');
  assert.deepEqual(snapshot.map((frame) => frame.phase), ['begin', 'messages', 'records', 'end']);
  assert.equal(snapshot[1].messages[0].blocks[0].text, 'earlier');
  assert.deepEqual(snapshot[2].records.map((item: any) => item.record.event.type), ['model_changed']);

  channel.clear();
  const phone = viewer({ device_name: 'Pixel' });
  const sent = await commands.promptSend(runtime, phone, { session_id: SESSION_A, text: 'hello from the phone', client_request_id: 'r1' });
  assert.deepEqual(sent, { turn_id: 777 });
  assert.deepEqual(client.prompts, [{ text: 'hello from the phone', opts: { images: [], turnId: 777 } }]);
  assert.equal(runtime.turnActive, true);
  // The bridge announces the admitted turn; passive viewers receive the same
  // boundary once, ahead of its output.
  client.emitEvent({ type: 'turn_started', turn_id: 777 });
  client.emitEvent({ type: 'text_delta', text: 'Hi ' });
  client.emitEvent({ type: 'text_delta', text: 'there' });
  client.emitEvent({ type: 'provider_credential_status', secret: 'sk-not-for-viewers' });
  clock.advance(LIVE_BATCH_MS);
  const records = channel.records();
  assert.deepEqual(records.map((item) => item.record.type === 'event' ? item.record.event.type : item.record.type), ['prompt', 'turn_started', 'text_delta']);
  assert.deepEqual(records[0]!.record, { type: 'prompt', turn_id: 777, text: 'hello from the phone', origin: 'device', device_name: 'Pixel' });
  assert.equal(records[2]!.record.event.text, 'Hi there');
  assert.ok(!JSON.stringify(channel.sent).includes('sk-not-for-viewers'));
  assert.deepEqual(records.map((item) => item.seq), [...records.map((item) => item.seq)].sort((a, b) => a - b));
  assert.equal(echoes.length, 1);

  // A viewer that drops and returns with its last position gets only what it missed.
  attachment.close();
  client.emitEvent({ type: 'text_delta', text: 'while away' });
  const again = new SessionAttachment({ runtime, channel: new RecordingChannel(), clock });
  const view = runtime.remoteView();
  const resumed = again.attach({ epoch: view.epoch, after_seq: records.at(-1)!.seq });
  assert.equal(resumed.mode, 'delta');
  again.close();
  await runtime.dispose();
});

test('submitted and queued turns each publish one correlated remote start before output', async () => {
  const { runtime, client } = connectedRuntime();
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const observed: RemoteObservation[] = [];
  runtime.observeRemote((observation) => observed.push(observation));
  await runtime.sendPrompt('first', [], 1);
  await runtime.sendPrompt('steering', [], 2);
  assert.deepEqual(runtime.remoteView().turn, { running: true, turn_id: 1 });
  assert.deepEqual(runtime.remoteView().prompts.map((entry) => [entry.text, entry.turnId]), [
    ['first', 1], ['steering', 2],
  ]);
  client.emitEvent({ type: 'turn_started', turn_id: 1 });
  client.emitEvent({ type: 'text_delta', text: 'first output' });
  client.emitEvent({
    type: 'turn_ended', outcome: 'end_turn', stop_reason: 'end_turn',
    cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '$0' },
  });
  client.emitEvent({ type: 'turn_started', turn_id: 2 });
  client.emitEvent({ type: 'text_delta', text: 'queued output' });
  assert.deepEqual(observed.filter((item) => item.type === 'event').map((item) => item.envelope.event),
    [
      { type: 'turn_started', turn_id: 1 },
      { type: 'text_delta', text: 'first output' },
      {
        type: 'turn_ended', outcome: 'end_turn', stop_reason: 'end_turn',
        cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '$0' },
      },
      { type: 'turn_started', turn_id: 2 },
      { type: 'text_delta', text: 'queued output' },
    ]);
  assert.deepEqual(runtime.remoteView().turn, { running: true, turn_id: 2 });
  await runtime.dispose();
});

test('session replay stays ahead of later turn events while old media cleanup is pending', async () => {
  const { runtime, client } = connectedRuntime();
  let releaseCleanup!: () => void;
  const cleanup = new Promise<void>((resolve) => { releaseCleanup = resolve; });
  (runtime as any).nativeMediaOwner = 'old-session';
  (runtime as any).nativeMediaDevices = { dispose: () => cleanup };
  const events: string[] = [];
  runtime.observeRemote((observation) => {
    if (observation.type === 'event') events.push(observation.envelope.event.type);
  });

  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  client.emitEvent({ type: 'turn_started', turn_id: 1 });
  client.emitEvent({ type: 'text_delta', text: 'live output' });
  assert.deepEqual(events, ['session_resumed', 'turn_started', 'text_delta']);
  assert.deepEqual(runtime.replaySnapshot().map((entry) => entry.event.type), events);
  assert.deepEqual(runtime.remoteView().turn, { running: true, turn_id: 1 });

  releaseCleanup();
  await (runtime as any).nativeMediaDeviceUpdate;
  await runtime.dispose();
});

test('media device requests wait for the latest session owner during cleanup', async () => {
  const { runtime, client } = connectedRuntime();
  let releaseCleanup!: () => void;
  let reportCleanupStarted!: () => void;
  const cleanup = new Promise<void>((resolve) => { releaseCleanup = resolve; });
  const cleanupStarted = new Promise<void>((resolve) => { reportCleanupStarted = resolve; });
  (runtime as any).nativeMediaOwner = SESSION_A;
  (runtime as any).nativeMediaDevices = {
    dispose: () => { reportCleanupStarted(); return cleanup; },
  };
  const openedOwners: string[] = [];
  (runtime as any).opts.audioService = {
    executeMediaDevice: async (request: any) => {
      if (request.request.type === 'open') {
        openedOwners.push(request.request.request.owner.session_id);
        return { request_id: request.request_id, result: { type: 'opened', lease: { lease_id: 'lease-c' } } };
      }
      return { request_id: request.request_id, result: { type: 'stopped' } };
    },
  };
  const openFor = (requestId: string, sessionId: string) => ({
    request_id: requestId,
    request: { type: 'open', request: {
      identity: { id: `operation-${requestId}`, generation: 1, service_epoch: 10 },
      owner: { type: 'session', session_id: sessionId },
      max_payload_bytes: 1024,
      request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'microphone' } },
    } },
  });

  client.emitEvent({ type: 'session_started', session_id: SESSION_B });
  await cleanupStarted;
  client.emit('mediaDeviceRequest', openFor('old', SESSION_B));
  client.emitEvent({ type: 'session_started', session_id: SESSION_C });
  client.emit('mediaDeviceRequest', openFor('current', SESSION_C));
  await Promise.resolve();
  assert.deepEqual(client.sent, [], 'requests stay pending until owner cleanup settles');
  assert.deepEqual(openedOwners, []);

  releaseCleanup();
  await (runtime as any).nativeMediaDeviceUpdate;
  await new Promise<void>((resolve) => setImmediate(resolve));
  const responses = client.sent.filter((item: any) => item.type === 'media_device_response') as any[];
  assert.equal(responses.length, 2);
  assert.equal(responses.find((item) => item.response.request_id === 'old').response.result.type, 'error');
  assert.equal(responses.find((item) => item.response.request_id === 'current').response.result.type, 'opened');
  assert.deepEqual(openedOwners, [SESSION_C]);
  await runtime.dispose();
});

test('a media request waiting for owner cleanup cannot reach a replacement connection', async () => {
  const { runtime, client } = connectedRuntime();
  let releaseCleanup!: () => void;
  let reportCleanupStarted!: () => void;
  const cleanup = new Promise<void>((resolve) => { releaseCleanup = resolve; });
  const cleanupStarted = new Promise<void>((resolve) => { reportCleanupStarted = resolve; });
  (runtime as any).nativeMediaOwner = SESSION_A;
  (runtime as any).nativeMediaDevices = {
    dispose: () => { reportCleanupStarted(); return cleanup; },
  };
  let opens = 0;
  (runtime as any).opts.audioService = {
    executeMediaDevice: async (request: any) => {
      opens++;
      return { request_id: request.request_id, result: { type: 'opened', lease: { lease_id: 'unexpected' } } };
    },
  };
  client.emitEvent({ type: 'session_started', session_id: SESSION_B });
  await cleanupStarted;
  client.emit('mediaDeviceRequest', {
    request_id: 'stale', request: { type: 'open', request: {
      identity: { id: 'operation-stale', generation: 1, service_epoch: 10 },
      owner: { type: 'session', session_id: SESSION_B }, max_payload_bytes: 1024,
      request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'microphone' } },
    } },
  });
  (runtime as any).generation = 2;
  releaseCleanup();
  await (runtime as any).nativeMediaDeviceUpdate;
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(opens, 0);
  assert.deepEqual(client.sent, []);
  await runtime.dispose();
});

test('a real runtime: /clear is a new epoch, so an old position gets a snapshot and the live viewer is reset', async () => {
  const { runtime, client } = connectedRuntime();
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const before = runtime.remoteView();
  assert.equal(before.baseSequence, 1);
  const channel = new RecordingChannel();
  const clock = newClock();
  const attachment = new SessionAttachment({ runtime, channel, clock });
  attachment.attach(undefined);
  channel.clear();
  client.emitEvent({ type: 'session_started', session_id: SESSION_A });
  assert.deepEqual(channel.methods(), ['session.reset']);
  clock.advance(LIVE_BATCH_MS);
  const frames = channel.of('session.snapshot');
  const after = runtime.remoteView();
  assert.notEqual(after.epoch, before.epoch);
  assert.equal(after.runtimeId, before.runtimeId);
  assert.equal(frames[0].epoch, after.epoch);
  assert.equal(new SessionAttachment({ runtime, channel: new RecordingChannel(), clock }).attach({ epoch: before.epoch, after_seq: 1 }).mode, 'snapshot');
  attachment.close();
  await runtime.dispose();
});

test('a real runtime: two runtimes for one session never share an epoch', async () => {
  const first = connectedRuntime().runtime;
  const second = connectedRuntime().runtime;
  assert.notEqual(first.remoteView().runtimeId, second.remoteView().runtimeId);
  await first.dispose();
  await second.dispose();
});

test('a real runtime: the prompt journal keeps 500 entries and image bytes for the last 20; a viewer behind the dropped part is sent a snapshot', async () => {
  const { runtime, client } = connectedRuntime();
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const image = { media_type: 'image/png', base64: PNG_BASE64 };
  const sequences: number[] = [];
  for (let i = 0; i < 505; i += 1) {
    (runtime as any).journalPrompt(`p${i}`, i % 5 === 0 ? [image] : [], i + 1, undefined);
    sequences.push(runtime.remoteView().lastSequence);
  }
  const view = runtime.remoteView();
  assert.equal(view.prompts.length, 500);
  assert.equal(view.prompts[0]!.text, 'p5');
  assert.equal(view.prompts.at(-1)!.text, 'p504');
  const withBytes = view.prompts.filter((entry) => entry.images.some((item) => item.base64 !== undefined));
  assert.equal(withBytes.length, 20);
  assert.deepEqual(withBytes.map((entry) => entry.text), Array.from({ length: 20 }, (_, i) => `p${405 + i * 5}`));
  // Older images keep their media type but not their bytes.
  const older = view.prompts.find((entry) => entry.text === 'p10')!;
  assert.deepEqual(older.images, [{ media_type: 'image/png' }]);
  assert.equal(runtime.promptImage(older.sequence, 0), undefined);
  const newest = withBytes.at(-1)!;
  assert.deepEqual(runtime.promptImage(newest.sequence, 0), { media_type: 'image/png', base64: PNG_BASE64 });
  // The first five entries were forgotten, so a position before them cannot be resumed.
  assert.equal(view.droppedThroughSequence, sequences[4]);
  const attach = (afterSeq: number) => new SessionAttachment({ runtime, channel: new RecordingChannel(), clock: newClock() }).attach({ epoch: view.epoch, after_seq: afterSeq }).mode;
  assert.equal(attach(sequences[2]!), 'snapshot');
  assert.equal(attach(sequences[4]!), 'delta');
  await runtime.dispose();
});

test('a real runtime: scheduled prompts are journaled as scheduled, slash commands that echo are journaled with their origin', async () => {
  const { runtime, client } = connectedRuntime();
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/compact keep this', images: [], turn_id: 5 }, { origin: 'device', deviceName: 'Pixel' });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/effort high', images: [], turn_id: 6 }, { origin: 'device', deviceName: 'Pixel' });
  const prompts = runtime.remoteView().prompts;
  assert.deepEqual(prompts.map((entry) => [entry.text, entry.origin, entry.deviceName, entry.turnId]), [['/compact keep this', 'device', 'Pixel', 5]]);
  await runtime.dispose();
});

test('a real runtime: the first answer to a permission request wins, whichever side it is', async () => {
  const { runtime, client } = connectedRuntime();
  (runtime as any).activeTurn = true;
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const channel = new RecordingChannel();
  const clock = newClock();
  const attachment = new SessionAttachment({ runtime, channel, clock });
  attachment.attach(undefined);
  channel.clear();
  const commands = new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => undefined, clock });

  client.emit('permission', permissionRequest(1));
  client.emit('permission', permissionRequest(2));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(runtime.remotePending().permissions.map((request) => request.request_id), [1, 2]);
  assert.equal(channel.of('session.pending').at(-1).pending.permissions.length, 2);
  assert.ok(channel.records().some((item) => item.record.type === 'permission_request'));

  // The viewer answers request 1 first; the desktop's later answer fails.
  commands.permissionRespond(runtime, { session_id: SESSION_A, request_id: 1, decision: { type: 'allow_once' } });
  assert.deepEqual(client.approved, [[1, { type: 'allow_once' }]]);
  assert.throws(() => runtime.approvePermission(1), /not pending/);
  // The desktop answers request 2 first; the viewer's later answer is not_pending.
  runtime.denyPermission(2);
  assert.throws(() => commands.permissionRespond(runtime, { session_id: SESSION_A, request_id: 2, decision: { type: 'allow_once' } }), (error: any) => error.code === 'not_pending');
  assert.deepEqual(client.denied, [2]);
  assert.equal(client.approved.length, 1);
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(channel.of('session.pending').at(-1).pending, { permissions: [], questions: [], computer_access: [] });
  attachment.close();
  await runtime.dispose();
});

test('a real runtime: a bypass-permissions request cannot be approved by a viewer', async () => {
  const { runtime, client } = connectedRuntime();
  (runtime as any).activeTurn = true;
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const commands = new RemoteCommands({ allowAlwaysAllow: () => true, echo: () => undefined, clock: newClock() });
  client.emit('permission', permissionRequest(3, { type: 'bypass_permissions_mode' }));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.throws(() => commands.permissionRespond(runtime, { session_id: SESSION_A, request_id: 3, decision: { type: 'allow_once' } }), (error: any) => error.code === 'forbidden');
  assert.equal(client.approved.length, 0);
  assert.equal(runtime.remotePending().permissions.length, 1);
  await runtime.dispose();
});

test('a real runtime: questions and computer access show as pending; computer access is display only', async () => {
  const { runtime, client } = connectedRuntime();
  (runtime as any).activeTurn = true;
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  const seen: RemoteObservation['type'][] = [];
  runtime.observeRemote((observation) => { seen.push(observation.type); });
  client.emitEvent({ type: 'ask_user_question', request: { request_id: 4, questions: [] } });
  client.emit('computerAccess', { request_id: 5, reason: 'automate', apps: [{ label: 'Slack' }], tier: 'full', clipboard_read: false, clipboard_write: false, system_key_combos: false });
  await new Promise<void>((resolve) => setImmediate(resolve));
  const pending = runtime.remotePending();
  assert.deepEqual(pending.questions.map((question) => question.request_id), [4]);
  assert.deepEqual(pending.computer_access.map((request) => request.request_id), [5]);
  assert.equal(runtime.pendingQuestion(4)?.request_id, 4);
  assert.ok(seen.includes('pending'));
  assert.ok(seen.includes('interaction'), 'the computer access request is a record too');
  await runtime.dispose();
});

test('a real runtime: observers are told about the disposal and then forgotten', async () => {
  const { runtime } = connectedRuntime();
  const seen: string[] = [];
  const stop = runtime.observeRemote((observation) => seen.push(observation.type));
  assert.equal(runtime.remoteObserved, true);
  await runtime.dispose();
  assert.ok(seen.includes('disposed'));
  assert.equal(runtime.remoteObserved, false);
  stop();
});

test('a real runtime: an observer that throws does not disturb the others or the runtime', async () => {
  const { runtime, client } = connectedRuntime();
  const seen: string[] = [];
  runtime.observeRemote(() => { throw new Error('bad observer'); });
  runtime.observeRemote((observation) => seen.push(observation.type));
  client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, messages: [] });
  client.emitEvent({ type: 'text_delta', text: 'x' });
  assert.deepEqual(seen, ['event', 'event']);
  await runtime.dispose();
});

// ─────────────────────────────────────────────────────────────────────────────
// The manager
// ─────────────────────────────────────────────────────────────────────────────

function stubbedManager() {
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    await (this as any).opts.launchConfig();
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({
    launchConfig: (candidate) => ({ workspace: '/workspace', sessionId: candidate?.sessionId, trusted: true }),
  } as never);
  const refs = [SESSION_A, SESSION_B, SESSION_C, SESSION_D].map((sessionId) => ({ projectPath: '/workspace', sessionId }));
  return {
    manager,
    refs,
    restore() {
      SessionRuntime.prototype.start = originalStart;
      SessionRuntime.prototype.resumeOwnedSession = originalResume;
    },
  };
}

test('the manager opens dormant sessions for remote without making them the desktop\'s active session', async () => {
  const { manager, refs, restore } = stubbedManager();
  try {
    const created: string[] = [];
    manager.onRuntimeCreated((runtime) => created.push(runtime.sessionId));
    const lease = await manager.openForRemote(refs[0]!);
    assert.equal(lease.runtime.sessionId, SESSION_A);
    assert.deepEqual(created, [SESSION_A]);
    assert.equal((manager as any).activeSessionId, null, 'no desktop activation');
    assert.equal(manager.get(SESSION_A), lease.runtime);
    // A listener added later hears of the runtimes that already exist.
    const late: string[] = [];
    manager.onRuntimeCreated((runtime) => late.push(runtime.sessionId));
    assert.deepEqual(late, [SESSION_A]);
    lease.release();
  } finally {
    restore();
    await manager.dispose();
  }
});

test('at most three runtimes are opened for remote; a lease released makes room, and the release is idempotent', async () => {
  const { manager, refs, restore } = stubbedManager();
  try {
    const leases = [];
    for (const ref of refs.slice(0, 3)) leases.push(await manager.openForRemote(ref));
    await assert.rejects(manager.openForRemote(refs[3]!), (error) => error instanceof RemoteCapacityError);
    leases[0]!.release();
    leases[0]!.release();
    const fourth = await manager.openForRemote(refs[3]!);
    await assert.rejects(manager.openForRemote({ projectPath: '/workspace', sessionId: '12121212-3434-4567-8899-eeeeeeeeeeee' }), RemoteCapacityError);
    // A session that is already open (the desktop's own) is only kept, not counted.
    const again = await manager.openForRemote(refs[1]!);
    assert.equal(again.runtime, leases[1]!.runtime);
    fourth.release();
  } finally {
    restore();
    await manager.dispose();
  }
});

test('a failed open gives its place back', async () => {
  const { manager, refs, restore } = stubbedManager();
  try {
    const start = SessionRuntime.prototype.start;
    SessionRuntime.prototype.start = async function () { throw new Error('cannot start'); };
    for (let i = 0; i < 5; i += 1) await assert.rejects(manager.openForRemote(refs[0]!), /cannot start/);
    SessionRuntime.prototype.start = start;
    const lease = await manager.openForRemote(refs[0]!);
    lease.release();
  } finally {
    restore();
    await manager.dispose();
  }
});

test('a runtime the desktop opened first does not count against the remote limit', async () => {
  const { manager, refs, restore } = stubbedManager();
  try {
    await manager.openSession(refs[0]!, false, undefined, false);
    const leases = [];
    for (const ref of refs.slice(1)) leases.push(await manager.openForRemote(ref));
    const one = await manager.openForRemote(refs[0]!);
    one.release();
    for (const lease of leases) lease.release();
  } finally {
    restore();
    await manager.dispose();
  }
});
