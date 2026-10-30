import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { test } from 'node:test';
import { BridgeClient, validateClientEvent } from '@lingxi/bridge-client';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager';
import { SessionRuntime } from '../src/main/bridge';
import { RealtimeAudioController, type RealtimeAudioRuntime } from '../src/main/audio/realtimeAudioController';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';

class Helper extends EventEmitter {
  readonly stdout = new PassThrough(); readonly stderr = new PassThrough(); readonly stdin = new PassThrough();
  readonly pid = 4242; readonly writes: any[] = []; readonly ackWrites: any[] = []; killCount = 0; autoAck = true;
  readonly captures = new Map<string, { identity: any; owner: any; context: any }>();
  constructor() { super(); this.stdin.write = ((chunk: string | Uint8Array) => {
    const value = JSON.parse(Buffer.from(chunk).toString());
    if (value.command?.type === 'ack_close_receipt') {
      this.ackWrites.push(value);
      if (this.autoAck) queueMicrotask(() => this.stdout.write(`${JSON.stringify({ id: value.id, type: 'response', result: { type: 'close_ack', snapshot: snapshot() } })}\n`));
    } else this.writes.push(value);
    return true;
  }) as typeof this.stdin.write; }
  kill() { this.killCount += 1; this.emit('exit', 0, null); return true; }
  respond(request: any, result: any, extra: Record<string, unknown> = {}) {
    if (result.type === 'recording_started') this.captures.set(result.handle, { identity: request.request.identity, owner: request.request.owner, context: request.closeContext });
    const origin = request.request?.operation?.type === 'stop_recording' ? this.captures.get(request.request.operation.handle) : undefined;
    const closeReceipt = origin && result.type === 'status' && result.status.recording === false
      ? { disposition: 'closed', ...origin, handle: request.request.operation.handle } : undefined;
    this.stdout.write(`${JSON.stringify({ id: request.id, type: 'response', result: { type: 'engine_result', snapshot: snapshot(), result, ...(closeReceipt ? { closeReceipt } : {}), ...extra } })}\n`);
  }
}
const capabilities = { service_epoch: 9, support_revision: 1, supported_operations: ['record', 'listen', 'synthesize', 'speak'], readiness: [], max_payload_bytes: 8_000_000 };
function snapshot() { return { helper: { state: 'running', instanceId: '7a269634-4dc7-4270-a2df-23bd7df64582' }, permissions: { microphone: 'granted', speech: 'authorized' }, owner: null, activity: 'idle', audioOperations: [], activeOperationCount: 0, pendingOperationCount: 0, activeRecordingCount: 0, activePlaybackCount: 0, activeModelReferenceCount: 0, voices: [], models: [], mediaSessionModes: [], capabilities }; }
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
async function envelope(helper: Helper, index: number) { for (let n = 0; n < 50; n++) { if (helper.writes[index]) return helper.writes[index]; await tick(); } throw new Error(`missing helper envelope ${index}`); }
async function ackEnvelope(helper: Helper, index: number) { for (let n = 0; n < 50; n++) { if (helper.ackWrites[index]) return helper.ackWrites[index]; await tick(); } throw new Error(`missing helper ack ${index}`); }
async function fixture() {
  const helper = new Helper();
  const manager = new NativeAudioManager({ isPackaged: false, resourcesPath: '/resources', userDataPath: '/tmp/lingxi-audio-tests', helperPath: '/tmp/LingXiAudioHelper', diagnostics: new DiagnosticBuffer(), suspendTeardownTimeoutMs: 25, closeAckRetryMs: 10, spawnHelper: () => helper as any });
  const priming = manager.request({ type: 'get_snapshot' }); const request = await envelope(helper, 0);
  helper.stdout.write(`${JSON.stringify({ id: request.id, type: 'response', result: { type: 'snapshot', snapshot: snapshot() } })}\n`); await priming;
  return { manager, helper, async close() { await manager.dispose(); helper.stdin.end(); helper.stdout.end(); helper.stderr.end(); } };
}
const owner = { type: 'session', session_id: 's' } as const;
const started = (handle: string) => ({ type: 'recording_started', handle });
const stopped = { type: 'status', status: { recording: false, playing: false } };

test('controller and JSONL manager stop forwarding immediately and report helper failure', async () => {
  const f = await fixture();
  try {
    let receive: ((json: string) => void) | undefined;
    const inputs: any[] = []; const starts: any[] = []; const states: any[] = [];
    const runtime: RealtimeAudioRuntime = { sessionId: 's', realtimeAudioSupported: true,
      onRealtimeAudioEvent: (callback) => { receive = callback; return () => { receive = undefined; }; },
      startRealtimeAudio: (json) => starts.push(JSON.parse(json)), realtimeAudioInput: (json) => inputs.push(JSON.parse(json)), stopRealtimeAudio: () => undefined };
    const controller = new RealtimeAudioController({ audio: f.manager, currentRuntime: () => runtime, publish: (state) => states.push(state) });
    await controller.start(audioConfigurationDefaults());
    receive!(JSON.stringify({ type: 'session_ready', operationId: starts[0].operationId, inputFormat: { encoding: 'pcm16', channels: 1, sampleRateHz: 24_000 } }));
    const start = await envelope(f.helper, 1); assert.equal(start.streamPcm, true);
    f.helper.respond(start, started('capture-1')); await tick();
    const event = { type: 'capture_chunk', owner: { kind: 'session', id: 's' }, identity: start.request.identity, sequence: 0, pcmBase64: 'AAA=', sampleRateHz: 24_000 };
    f.helper.stdout.write(`${JSON.stringify({ type: 'event', event })}\n`); assert.equal(inputs.length, 1);
    const committing = controller.commit();
    f.helper.stdout.write(`${JSON.stringify({ type: 'event', event: { ...event, sequence: 1 } })}\n`);
    const stop = await envelope(f.helper, 2); assert.deepEqual(stop.request.operation, { type: 'stop_recording', handle: 'capture-1' });
    f.helper.respond(stop, { type: 'failed', error: { kind: 'native_failure', message: 'stop failed' } });
    await assert.rejects(committing, /stop failed/); assert.equal(inputs.length, 1);
    assert.equal(states.at(-1)?.phase, 'failed');
    await assert.rejects(controller.start(audioConfigurationDefaults()), /stop failed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
    const recovery = f.helper.writes.slice(3);
    assert.equal(recovery.length, 2);
    assert.ok(recovery.every((item) => item.command?.type === 'cancel_operation'));
  } finally { await f.close(); }
});

test('stream stop accepts the Swift status result and an old handle cannot stop a later stream', async () => {
  const f = await fixture();
  try {
    const start1 = f.manager.startMediaCapture(owner, 24_000); const r1 = await envelope(f.helper, 1); f.helper.respond(r1, started('h1')); await start1;
    const stop1 = f.manager.stopMediaCapture(owner, 'h1'); const duplicate = f.manager.stopMediaCapture(owner, 'h1');
    const s1 = await envelope(f.helper, 2); f.helper.respond(s1, stopped); await Promise.all([stop1, duplicate]);
    const start2 = f.manager.startMediaCapture(owner, 24_000); const r2 = await envelope(f.helper, 3); f.helper.respond(r2, started('h2')); await start2;
    await assert.rejects(f.manager.stopMediaCapture(owner, 'h1'), /no longer current/);
    await assert.rejects(f.manager.stopMediaCapture(owner), /exact audio capture handle/);
    assert.equal(f.helper.writes.length, 4);
    const stop2 = f.manager.stopMediaCapture(owner, 'h2'); const s2 = await envelope(f.helper, 4); f.helper.respond(s2, stopped); await stop2;
  } finally { await f.close(); }
});

test('a helper replacement does not receive an old capture stop or permit restart', async () => {
  const f = await fixture();
  try {
    const starting = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await starting;
    f.helper.kill(); await assert.rejects(f.manager.stopMediaCapture(owner, 'h1'), /helper changed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed'); assert.equal(f.helper.writes.length, 2);
  } finally { await f.close(); }
});

test('unexpected recording status does not complete stop or free the owner slot', async () => {
  const f = await fixture();
  try {
    const starting = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await starting;
    const stopping = f.manager.stopMediaCapture(owner, 'h1'); const stop = await envelope(f.helper, 2);
    f.helper.respond(stop, { type: 'status', status: { recording: true, playing: false } });
    await assert.rejects(stopping, /unconfirmed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
  } finally { await f.close(); }
});

test('playback owner teardown reserves the local owner until its response', async () => {
  const f = await fixture();
  try {
    const stopping = f.manager.stopMediaCapture(owner); const duplicate = f.manager.stopMediaCapture(owner); const end = await envelope(f.helper, 1);
    assert.equal(end.request.operation.type, 'end_owner');
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
    f.helper.respond(end, { type: 'owner_ended' }); await Promise.all([stopping, duplicate]);
    const starting = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 2); f.helper.respond(request, started('h1')); await starting;
    const stop = f.manager.stopMediaCapture(owner, 'h1'); const stopRequest = await envelope(f.helper, 3); f.helper.respond(stopRequest, stopped); await stop;
  } finally { await f.close(); }
});


test('controller stop during a delayed real-manager start cleans that handle before settling', async () => {
  const f = await fixture();
  try {
    let receive: ((json: string) => void) | undefined; const starts: any[] = []; const inputs: any[] = []; const states: any[] = [];
    const runtime: RealtimeAudioRuntime = { sessionId: 's', realtimeAudioSupported: true,
      onRealtimeAudioEvent: (callback) => { receive = callback; return () => { receive = undefined; }; },
      startRealtimeAudio: (json) => starts.push(JSON.parse(json)), realtimeAudioInput: (json) => inputs.push(JSON.parse(json)), stopRealtimeAudio: () => undefined };
    const controller = new RealtimeAudioController({ audio: f.manager, currentRuntime: () => runtime, publish: (state) => states.push(state) });
    await controller.start(audioConfigurationDefaults());
    receive!(JSON.stringify({ type: 'session_ready', operationId: starts[0].operationId, inputFormat: { encoding: 'pcm16', channels: 1, sampleRateHz: 24_000 } }));
    const start = await envelope(f.helper, 1); const stopping = controller.stop(); await tick();
    assert.equal(f.helper.writes.length, 2);
    f.helper.respond(start, started('late-handle'));
    const stop = await envelope(f.helper, 2); assert.equal(stop.request.operation.handle, 'late-handle');
    f.helper.respond(stop, stopped); await stopping;
    assert.deepEqual(inputs, []); assert.equal(states.at(-1)?.phase, 'paused');
    assert.equal(states.some((state) => state.phase === 'listening'), false);
  } finally { await f.close(); }
});

test('suspend while a helper stop is pending rejects it and keeps the capture fenced', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await start;
    const stopping = f.manager.stopMediaCapture(owner, 'h1'); await envelope(f.helper, 2);
    const rejected = assert.rejects(stopping, /unconfirmed|helper changed/);
    const suspend = f.manager.suspend('test'); await rejected; await assert.rejects(suspend, /cleanup remains unconfirmed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
  } finally { await f.close(); }
});

test('a timed-out controller remains fenced after a late real-manager stop result', async () => {
  const f = await fixture();
  try {
    let receive: ((json: string) => void) | undefined; const starts: any[] = []; const inputs: any[] = [];
    const runtime: RealtimeAudioRuntime = { sessionId: 's', realtimeAudioSupported: true,
      onRealtimeAudioEvent: (callback) => { receive = callback; return () => { receive = undefined; }; },
      startRealtimeAudio: (json) => starts.push(JSON.parse(json)), realtimeAudioInput: (json) => inputs.push(JSON.parse(json)), stopRealtimeAudio: () => undefined };
    const controller = new RealtimeAudioController({ audio: f.manager, currentRuntime: () => runtime, publish: () => undefined, inputCloseObserverMs: 15 });
    await controller.start(audioConfigurationDefaults());
    receive!(JSON.stringify({ type: 'session_ready', operationId: starts[0].operationId, inputFormat: { encoding: 'pcm16', channels: 1, sampleRateHz: 24_000 } }));
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('h1')); await tick();
    const committing = controller.commit(); const stop = await envelope(f.helper, 2);
    await assert.rejects(committing, /remains unconfirmed/);
    f.helper.respond(stop, stopped); await tick();
    await controller.start(audioConfigurationDefaults());
    assert.deepEqual(inputs, []); assert.equal(f.helper.writes.length, 3);
  } finally { await f.close(); }
});

test('held or rejected helper cancellation returns an explicit unconfirmed result and retains the owner', async () => {
  const f = await fixture();
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000061', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording' as const, sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    const original = f.manager.executeAudioRequest(request); const start = await envelope(f.helper, 1);
    const cancelling = f.manager.cancelAudioRequest(identity); const cancellation = await envelope(f.helper, 2);
    let settled = false; void original.then(() => { settled = true; }); await tick();
    assert.equal(settled, false, 'held cancellation is not sent as a terminal receipt');
    const next = await f.manager.executeAudioRequest({ ...request, identity: { ...identity, id: '00000000-0000-4000-8000-000000000062', generation: 2 } });
    assert.equal(next.type, 'failed_without_resource'); if (next.type === 'failed_without_resource') assert.match(next.error.message, /remains unconfirmed/);
    f.helper.stdout.write(`${JSON.stringify({ id: cancellation.id, type: 'response', result: { type: 'error', error: { code: 'native-error', message: 'refused' }, snapshot: snapshot() } })}\n`);
    await assert.rejects(cancelling, /cleanup remains unconfirmed/);
    assert.deepEqual(await original, { type: 'failed', error: { kind: 'unavailable', message: 'native audio cancellation was requested; cleanup remains unconfirmed' } });
    const retry = f.manager.cancelAudioRequest(identity); const retryCommand = await envelope(f.helper, 3);
    f.helper.stdout.write(`${JSON.stringify({ id: retryCommand.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: { disposition: 'unknown', context: retryCommand.command.closeContext, identity: retryCommand.command.identity, owner: retryCommand.command.owner } } })}\n`);
    await assert.rejects(retry, /remains unconfirmed/);
    f.helper.respond(start, started('late')); await tick();
    assert.equal(f.helper.writes.length, 4);
  } finally { await f.close(); }
});

test('helper-originated cancelled response cannot be mistaken for a cleanup receipt', async () => {
  const f = await fixture();
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000071', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording' as const, sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    const original = f.manager.executeAudioRequest(request); const start = await envelope(f.helper, 1);
    f.helper.respond(start, { type: 'failed', error: { kind: 'cancelled', message: 'native task cancelled' } });
    const result = await original;
    assert.equal(result.type, 'failed'); if (result.type === 'failed') { assert.equal(result.error.kind, 'unavailable'); assert.match(result.error.message, /unconfirmed/); }
    const next = await f.manager.executeAudioRequest({ ...request, identity: { ...identity, id: '00000000-0000-4000-8000-000000000072', generation: 2 } });
    assert.equal(next.type, 'failed_without_resource'); if (next.type === 'failed_without_resource') assert.match(next.error.message, /unconfirmed/);
  } finally { await f.close(); }
});

test('a request waiting for admission rechecks a same-owner cancellation before helper dispatch', async () => {
  const f = await fixture();
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000081', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording' as const, sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    const old = f.manager.executeAudioRequest(request); await envelope(f.helper, 1);
    const next = f.manager.executeAudioRequest({ ...request, identity: { ...identity, id: '00000000-0000-4000-8000-000000000082', generation: 2 } });
    const cancel = f.manager.cancelAudioRequest(identity); const command = await envelope(f.helper, 2);
    const result = await next; assert.equal(result.type, 'failed_without_resource'); if (result.type === 'failed_without_resource') assert.match(result.error.message, /remains unconfirmed/);
    f.helper.stdout.write(`${JSON.stringify({ id: command.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: { disposition: 'unknown', context: command.command.closeContext, identity: command.command.identity, owner: command.command.owner } } })}\n`);
    await assert.rejects(cancel, /remains unconfirmed/); assert.equal((await old).type, 'failed');
    assert.equal(f.helper.writes.length, 3);
  } finally { await f.close(); }
});


test('a late failed owner teardown does not kill a replacement helper', async () => {
  const f = await fixture(); const replacement = new Helper();
  try {
    const stopping = f.manager.stopMediaCapture(owner); await envelope(f.helper, 1);
    const rejected = assert.rejects(stopping, /could not be ended/);
    f.helper.kill();
    // Emulate a replacement between the old helper's exit and the waiting
    // teardown continuation. The teardown must remain bound to its producer.
    (f.manager as any).helper = replacement;
    await rejected; assert.equal(replacement.killCount, 0);
  } finally { await f.close(); replacement.stdin.end(); replacement.stdout.end(); replacement.stderr.end(); }
});


test('not-admitted requires a matching joined-task control receipt', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    f.helper.respond(request, { type: 'failed', error: { kind: 'permission_denied', message: 'denied' } });
    const initial = await start; assert.equal(initial.type, 'failed'); assert.equal(initial.captureAdmission, 'unknown');
    const cancel = await envelope(f.helper, 2);
    const wrong = { disposition: 'not_admitted', context: { ...cancel.command.closeContext, flight_id: 'ffffffff-ffff-4fff-8fff-ffffffffffff' }, identity: cancel.command.identity, owner: cancel.command.owner };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: wrong } })}\n`);
    await tick(); assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
    const retry = f.manager.cancelAudioRequest(request.request.identity); const control = await envelope(f.helper, 3);
    const exact = { ...wrong, context: control.command.closeContext };
    f.helper.stdout.write(`${JSON.stringify({ id: control.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: exact } })}\n`);
    await retry; await initial.captureClosed;
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 4);
    f.helper.respond(nextRequest, started('after-proof')); assert.equal((await next).type, 'recording_started');
    const stop = f.manager.stopMediaCapture(owner, 'after-proof'); const stopRequest = await envelope(f.helper, 5); f.helper.respond(stopRequest, stopped); await stop;
  } finally { await f.close(); }
});

test('a status without the exact close receipt cannot free the recording', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await start;
    const stop = f.manager.stopMediaCapture(owner, 'h1'); const control = await envelope(f.helper, 2);
    f.helper.respond(control, stopped, { closeReceipt: undefined });
    await assert.rejects(stop, /remains unconfirmed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
  } finally { await f.close(); }
});


test('suspension never projects an attached media-open owner to a logical audio owner', async () => {
  const f = await fixture();
  const timer = setTimeout(() => undefined, 10_000);
  try {
    const pending = (f.manager as any).pending as Map<string, unknown>;
    pending.set('attached-open', { kind: 'media', timer, resolve: () => undefined, reject: () => undefined,
      request: { request_id: 'open', request: { type: 'open', request: { owner: { type: 'attached', attachment_host_id: 'host', attachment_generation: 1, logical_kind: 'session', logical_owner_id: 's' } } } } });
    await assert.rejects(f.manager.suspend('attached-owner'), /cleanup remains unconfirmed/);
    assert.equal(f.helper.killCount, 1);
    assert.equal(f.helper.writes.some((value) => value.request?.operation?.type === 'end_owner'), false);
  } finally { clearTimeout(timer); await f.close(); }
});

test('verified cancellation retires the exact capture flight and allows a new recording', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    f.helper.respond(request, started('h1')); await start;
    const cancel = f.manager.cancelAudioRequest(request.request.identity); const command = await envelope(f.helper, 2);
    const receipt = { disposition: 'closed', context: command.command.closeContext, identity: command.command.identity, owner: command.command.owner, handle: 'h1' };
    f.helper.stdout.write(`${JSON.stringify({ id: command.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await cancel;
    const ack = await ackEnvelope(f.helper, 0); assert.equal(ack.command.type, 'ack_close_receipt');
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 3);
    f.helper.respond(nextRequest, started('h2')); assert.equal((await next).type, 'recording_started');
    await f.manager.cancelAudioRequest(request.request.identity);
    assert.equal(f.helper.writes.length, 4, 'old completed flight does not target the new capture');
    const stop = f.manager.stopMediaCapture(owner, 'h2'); const stopRequest = await envelope(f.helper, 4); f.helper.respond(stopRequest, stopped); await stop;
  } finally { await f.close(); }
});

test('a mismatched cancellation flight cannot release the owner and a retry uses the original flight', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await start;
    const cancel = f.manager.cancelAudioRequest(request.request.identity); const command = await envelope(f.helper, 2);
    const wrong = { disposition: 'closed', context: { ...command.command.closeContext, flight_id: 'fefefefe-fefe-4efe-8efe-fefefefefefe' }, identity: command.command.identity, owner: command.command.owner, handle: 'h1' };
    f.helper.stdout.write(`${JSON.stringify({ id: command.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: wrong } })}\n`);
    await assert.rejects(cancel, /remains unconfirmed/);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
    const retry = f.manager.cancelAudioRequest(request.request.identity); const next = await envelope(f.helper, 3);
    assert.deepEqual(next.command.closeContext, command.command.closeContext);
    const valid = { ...wrong, context: command.command.closeContext };
    f.helper.stdout.write(`${JSON.stringify({ id: next.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: valid } })}\n`);
    await retry;
    const ack = await ackEnvelope(f.helper, 0); assert.equal(ack.command.type, 'ack_close_receipt');
  } finally { await f.close(); }
});


test('a lost close acknowledgement is retried without creating another close flight', async () => {
  const f = await fixture();
  try {
    f.helper.autoAck = false;
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1); f.helper.respond(request, started('h1')); await start;
    const cancel = f.manager.cancelAudioRequest(request.request.identity); const command = await envelope(f.helper, 2);
    const receipt = { disposition: 'closed', context: command.command.closeContext, identity: command.command.identity, owner: command.command.owner, handle: 'h1' };
    f.helper.stdout.write(`${JSON.stringify({ id: command.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`); await cancel;
    const ack = await ackEnvelope(f.helper, 0);
    f.helper.stdout.write(`${JSON.stringify({ id: ack.id, type: 'response', result: { type: 'error', snapshot: snapshot(), error: { code: 'native-error', message: 'lost' } } })}\n`);
    await new Promise((resolve) => setTimeout(resolve, 20));
    const retry = await ackEnvelope(f.helper, 1); assert.equal(retry.command.type, 'ack_close_receipt');
    assert.deepEqual(retry.command.closeContext, ack.command.closeContext);
    f.helper.stdout.write(`${JSON.stringify({ id: retry.id, type: 'response', result: { type: 'close_ack', snapshot: snapshot() } })}\n`);
    await tick(); assert.equal((f.manager as any).pendingCloseAcks.size, 0);
    assert.equal(f.helper.writes.filter((item) => item.command?.type === 'cancel_operation').length, 1);
  } finally { await f.close(); }
});

test('a cancelled pending capture waits for its joined not-admitted receipt', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    const cancel = f.manager.cancelAudioRequest(request.request.identity); const command = await envelope(f.helper, 2);
    f.helper.respond(request, { type: 'failed', error: { kind: 'cancelled', message: 'before native capture' } });
    await tick();
    const receipt = { disposition: 'not_admitted', context: command.command.closeContext, identity: command.command.identity, owner: command.command.owner };
    f.helper.stdout.write(`${JSON.stringify({ id: command.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await cancel; const startedResult = await start;
    assert.equal(startedResult.type, 'failed_without_resource'); assert.equal(startedResult.captureAdmission, 'never_admitted');
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 3); f.helper.respond(nextRequest, started('h2')); await next;
    const stop = f.manager.stopMediaCapture(owner, 'h2'); const stopRequest = await envelope(f.helper, 4); f.helper.respond(stopRequest, stopped); await stop;
  } finally { await f.close(); }
});

test('a failed admitted capture can recover after the original helper supplies a matching close receipt', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    f.helper.respond(request, { type: 'failed', error: { kind: 'native_failure', message: 'capture failed after start' } });
    const result = await start; assert.equal(result.captureAdmission, 'unknown');
    const cancel = await envelope(f.helper, 2);
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity: cancel.command.identity, owner: cancel.command.owner, handle: 'closed-handle' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await result.captureClosed;
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 3);
    f.helper.respond(nextRequest, started('new-handle')); assert.equal((await next).type, 'recording_started');
    const stop = f.manager.stopMediaCapture(owner, 'new-handle'); const stopRequest = await envelope(f.helper, 4); f.helper.respond(stopRequest, stopped); await stop;
  } finally { await f.close(); }
});

test('failed stop retains both flights until exact stop and original capture receipts arrive', async () => {
  const f = await fixture();
  try {
    const starting = f.manager.startMediaCapture(owner, 24_000); const start = await envelope(f.helper, 1); f.helper.respond(start, started('h1')); await starting;
    const stopping = f.manager.stopMediaCapture(owner, 'h1'); const stop = await envelope(f.helper, 2);
    f.helper.respond(stop, { type: 'failed', error: { kind: 'native_failure', message: 'interrupted' } });
    await assert.rejects(stopping, /interrupted/);
    const a = await envelope(f.helper, 3), b = await envelope(f.helper, 4);
    assert.equal(a.command.type, 'cancel_operation'); assert.equal(b.command.type, 'cancel_operation');
    for (const cmd of [a, b]) {
      const receipt = { disposition: 'closed', context: cmd.command.closeContext, identity: cmd.command.identity, owner: cmd.command.owner, handle: 'h1' };
      f.helper.stdout.write(`${JSON.stringify({ id: cmd.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    }
    await tick(); await tick();
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 5);
    f.helper.respond(nextRequest, started('h2')); assert.equal((await next).type, 'recording_started');
    const finalStop = f.manager.stopMediaCapture(owner, 'h2'); const finalRequest = await envelope(f.helper, 6); f.helper.respond(finalRequest, stopped); await finalStop;
  } finally { await f.close(); }
});


test('matching native close receipt releases the original cancelled request only after confirmation', async () => {
  const f = await fixture();
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000091', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording' as const, sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    const original = f.manager.executeAudioRequest(request); const start = await envelope(f.helper, 1);
    const cancelling = f.manager.cancelAudioRequest(identity); const cancel = await envelope(f.helper, 2);
    let settled = false; void original.then(() => { settled = true; });
    f.helper.respond(start, { type: 'failed', error: { kind: 'cancelled', message: 'cancelled' } }); await tick();
    assert.equal(settled, false, 'original helper completion cannot overtake the close receipt');
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity, owner };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await cancelling; assert.deepEqual(await original, { type: 'failed_without_resource', error: { kind: 'cancelled', message: 'the audio operation was cancelled' } });
  } finally { await f.close(); }
});

test('duplicate identity or an unverified helper result cannot claim no resource', async () => {
  const f = await fixture();
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000092', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording' as const, sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    const first = f.manager.executeAudioRequest(request); const start = await envelope(f.helper, 1);
    assert.equal((await f.manager.executeAudioRequest({ ...request, timeout_budget_ms: 0 })).type, 'failed');
    assert.equal(f.manager.rejectAudioRequestBeforeDispatch(identity), false);
    f.helper.respond(start, { type: 'failed_without_resource', error: { kind: 'native_failure', message: 'unverified' } });
    assert.equal((await first).type, 'failed');
    const fresh = { ...identity, id: '00000000-0000-4000-8000-000000000093' };
    assert.equal(f.manager.rejectAudioRequestBeforeDispatch(fresh), true);
    assert.equal(f.manager.rejectAudioRequestBeforeDispatch(fresh), false);
    assert.equal((await f.manager.executeAudioRequest({ ...request, identity: fresh })).type, 'failed');
  } finally { await f.close(); }
});


test('owner teardown awaits an exact capture receipt before sending the broad end-owner request', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    f.helper.respond(request, started('owner-capture')); await start;
    const ending = f.manager.endAudioOwner(owner); const cancel = await envelope(f.helper, 2);
    assert.equal(cancel.command.type, 'cancel_operation');
    assert.equal(f.helper.writes.some((item) => item.request?.operation?.type === 'end_owner'), false);
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity: cancel.command.identity, owner: cancel.command.owner, handle: 'owner-capture' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    const end = await envelope(f.helper, 3); assert.equal(end.request.operation.type, 'end_owner');
    const nextIdentity = { id: '00000000-0000-4000-8000-000000000100', generation: 1, service_epoch: 9 };
    const whileClosing = await f.manager.executeAudioRequest({ identity: nextIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } });
    assert.equal(whileClosing.type, 'failed_without_resource');
    assert.equal(f.helper.writes.length, 4, 'a new session cannot be admitted before end-owner finishes');
    f.helper.respond(end, { type: 'owner_ended' }); await ending;
    const next = f.manager.startMediaCapture(owner, 24_000); const nextRequest = await envelope(f.helper, 4);
    f.helper.respond(nextRequest, started('next')); assert.equal((await next).type, 'recording_started');
    const stop = f.manager.stopMediaCapture(owner, 'next'); const last = await envelope(f.helper, 5); f.helper.respond(last, stopped); await stop;
  } finally { await f.close(); }
});

test('owner teardown with an unknown capture receipt fails without sending end-owner', async () => {
  const f = await fixture();
  try {
    const start = f.manager.startMediaCapture(owner, 24_000); const request = await envelope(f.helper, 1);
    f.helper.respond(request, started('owner-capture')); await start;
    const ending = f.manager.endAudioOwner(owner); const cancel = await envelope(f.helper, 2);
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: { disposition: 'unknown', context: cancel.command.closeContext, identity: cancel.command.identity, owner: cancel.command.owner } } })}\n`);
    await assert.rejects(ending, /could not be ended/);
    assert.equal(f.helper.writes.some((item) => item.request?.operation?.type === 'end_owner'), false);
    assert.equal((await f.manager.startMediaCapture(owner, 24_000)).type, 'failed');
  } finally { await f.close(); }
});


test('Bridge sends a late typed cleanup frame for a started capture only after the exact helper receipt', async () => {
  const f = await fixture(); const commands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
  (runtime as any).client = client; (runtime as any).generation = 4; (runtime as any).wireClient(client, 4);
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000094', generation: 1, service_epoch: 9 };
    const request = { identity, owner, operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
    client.emit('event', { type: 'audio_request', request }); const start = await envelope(f.helper, 1);
    f.helper.respond(start, started('bridge-capture')); await tick();
    assert.equal(commands.length, 1); assert.equal(commands[0].result.type, 'recording_started');
    client.emit('event', { type: 'audio_cancel', identity }); const cancel = await envelope(f.helper, 2);
    await tick(); assert.equal(commands.length, 1);
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity, owner, handle: 'bridge-capture' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    for (let n = 0; n < 10 && commands.length < 2; n++) await tick();
    assert.equal(commands.length, 2); assert.deepEqual(commands[1].identity, identity);
    assert.equal(commands[1].result.type, 'failed_without_resource');
    assert.equal(f.helper.writes.some((item) => item.request?.operation?.type === 'end_owner'), false);
  } finally { (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close(); }
});

test('Bridge emits one typed result for a pending cancelled request and no false proof on rejection', async () => {
  for (const accepted of [true, false]) {
    const f = await fixture(); const commands: any[] = [];
    const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
      sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
    const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
    (runtime as any).client = client; (runtime as any).generation = 5; (runtime as any).wireClient(client, 5);
    try {
      const identity = { id: accepted ? '00000000-0000-4000-8000-000000000095' : '00000000-0000-4000-8000-000000000096', generation: 1, service_epoch: 9 };
      const request = { identity, owner, operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' }, max_payload_bytes: 8_000_000 };
      client.emit('event', { type: 'audio_request', request }); const start = await envelope(f.helper, 1);
      client.emit('event', { type: 'audio_cancel', identity }); const cancel = await envelope(f.helper, 2);
      f.helper.respond(start, { type: 'failed', error: { kind: 'cancelled', message: 'cancelled' } }); await tick();
      assert.equal(commands.length, 0);
      const receipt = { disposition: accepted ? 'closed' : 'unknown', context: cancel.command.closeContext, identity, owner };
      f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
      for (let n = 0; n < 10 && commands.length < 1; n++) await tick();
      assert.equal(commands.length, 1); assert.equal(commands[0].result.type, accepted ? 'failed_without_resource' : 'failed');
      await tick(); assert.equal(commands.length, 1);
    } finally { (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close(); }
  }
});


test('Bridge normal stop emits exact origin cleanup proof before accepting its stop result', async () => {
  const f = await fixture(); const commands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
  (runtime as any).client = client; (runtime as any).generation = 6; (runtime as any).wireClient(client, 6);
  try {
    const startIdentity = { id: '00000000-0000-4000-8000-000000000097', generation: 1, service_epoch: 9 };
    const stopIdentity = { id: '00000000-0000-4000-8000-000000000098', generation: 2, service_epoch: 9 };
    client.emit('event', { type: 'audio_request', request: { identity: startIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } } });
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('normal-stop')); await tick();
    client.emit('event', { type: 'audio_request', request: { identity: stopIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'stop_recording', handle: 'normal-stop' } } });
    const stop = await envelope(f.helper, 2); await tick(); assert.equal(commands.length, 1);
    f.helper.respond(stop, stopped);
    for (let n = 0; n < 10 && commands.length < 3; n++) await tick();
    assert.equal(commands.length, 3);
    assert.deepEqual(commands[1].identity, startIdentity); assert.equal(commands[1].result.type, 'failed_without_resource');
    assert.deepEqual(commands[2].identity, stopIdentity); assert.equal(commands[2].result.type, 'status');
  } finally { (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close(); }
});

test('Bridge sends a late cleanup frame after an admitted timeout only on a matching receipt', async () => {
  const f = await fixture(); const commands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
  (runtime as any).client = client; (runtime as any).generation = 7; (runtime as any).wireClient(client, 7);
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000099', generation: 1, service_epoch: 9 };
    client.emit('event', { type: 'audio_request', request: { identity, owner, timeout_budget_ms: 20, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } } });
    await envelope(f.helper, 1); await new Promise((resolve) => setTimeout(resolve, 30)); const cancel = await envelope(f.helper, 2);
    for (let n = 0; n < 30 && commands.length < 1; n++) await new Promise((resolve) => setTimeout(resolve, 2));
    assert.equal(commands.length, 1); assert.equal(commands[0].result.type, 'failed');
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity, owner };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    for (let n = 0; n < 10 && commands.length < 2; n++) await tick();
    assert.equal(commands.length, 2); assert.deepEqual(commands[1].identity, identity); assert.equal(commands[1].result.type, 'failed_without_resource');
  } finally { (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close(); }
});

test('a verified old-helper origin closure cannot be reported to a replacement Bridge generation', async () => {
  const f = await fixture(); const oldCommands: any[] = []; const newCommands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const oldClient = new EventEmitter() as any; oldClient.sendCommand = (command: unknown) => oldCommands.push(command); oldClient.close = () => undefined;
  const newClient = new EventEmitter() as any; newClient.sendCommand = (command: unknown) => newCommands.push(command); newClient.close = () => undefined;
  (runtime as any).client = oldClient; (runtime as any).generation = 8; (runtime as any).wireClient(oldClient, 8);
  try {
    const identity = { id: '00000000-0000-4000-8000-000000000101', generation: 1, service_epoch: 9 };
    oldClient.emit('event', { type: 'audio_request', request: { identity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } } });
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('old-generation')); await tick();
    oldClient.emit('event', { type: 'audio_cancel', identity }); const cancel = await envelope(f.helper, 2);
    (runtime as any).client = newClient; (runtime as any).generation = 9;
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity, owner, handle: 'old-generation' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await tick(); await tick();
    assert.equal(oldCommands.length, 1); assert.equal(newCommands.length, 0);
  } finally {
    (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioStartsByGeneration.clear();
    (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close();
  }
});

test('Bridge rejects a stop status whose origin receipt has the wrong resource incarnation', async () => {
  const f = await fixture(); const commands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
  (runtime as any).client = client; (runtime as any).generation = 10; (runtime as any).wireClient(client, 10);
  try {
    const startIdentity = { id: '00000000-0000-4000-8000-000000000102', generation: 1, service_epoch: 9 };
    const stopIdentity = { id: '00000000-0000-4000-8000-000000000103', generation: 2, service_epoch: 9 };
    client.emit('event', { type: 'audio_request', request: { identity: startIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } } });
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('live')); await tick();
    client.emit('event', { type: 'audio_request', request: { identity: stopIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'stop_recording', handle: 'live' } } });
    const stop = await envelope(f.helper, 2);
    const wrong = { disposition: 'closed', context: { ...start.closeContext, resource_incarnation: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' }, identity: startIdentity, owner, handle: 'live' };
    f.helper.respond(stop, stopped, { closeReceipt: wrong });
    for (let n = 0; n < 10 && commands.length < 2; n++) await tick();
    assert.equal(commands.length, 2); assert.deepEqual(commands[1].identity, stopIdentity);
    assert.equal(commands[1].result.type, 'failed');
    assert.equal(commands.some((item) => item.identity?.id === startIdentity.id && item.result?.type === 'failed_without_resource'), false);
  } finally {
    (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioStartsByGeneration.clear();
    (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close();
  }
});

test('a failed Bridge stop independently reconciles its stop flight and exact capture origin', async () => {
  const f = await fixture(); const commands: any[] = [];
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 's', trusted: true },
    sessionId: 's', projectPath: '/workspace', audioService: f.manager } as any);
  const client = new EventEmitter() as any; client.sendCommand = (command: unknown) => commands.push(command); client.close = () => undefined;
  (runtime as any).client = client; (runtime as any).generation = 11; (runtime as any).wireClient(client, 11);
  try {
    const startIdentity = { id: '00000000-0000-4000-8000-000000000104', generation: 1, service_epoch: 9 };
    const stopIdentity = { id: '00000000-0000-4000-8000-000000000105', generation: 2, service_epoch: 9 };
    client.emit('event', { type: 'audio_request', request: { identity: startIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } } });
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('recover-origin')); await tick();
    client.emit('event', { type: 'audio_request', request: { identity: stopIdentity, owner, max_payload_bytes: 8_000_000,
      operation: { type: 'stop_recording', handle: 'recover-origin' } } });
    const stop = await envelope(f.helper, 2);
    f.helper.respond(stop, { type: 'failed', error: { kind: 'native_failure', message: 'interrupted' } });
    const a = await envelope(f.helper, 3), b = await envelope(f.helper, 4);
    const stopCancel = [a, b].find((item) => item.command.identity.id === stopIdentity.id)!;
    const originCancel = [a, b].find((item) => item.command.identity.id === startIdentity.id)!;
    assert.ok(stopCancel && originCancel); await tick();
    assert.equal(commands.length, 2); assert.equal(commands[1].result.type, 'failed');
    const stopReceipt = { disposition: 'closed', context: stopCancel.command.closeContext, identity: stopIdentity, owner, handle: 'recover-origin' };
    f.helper.stdout.write(`${JSON.stringify({ id: stopCancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: stopReceipt } })}\n`);
    for (let n = 0; n < 10 && commands.length < 3; n++) await tick();
    assert.deepEqual(commands[2].identity, stopIdentity); assert.equal(commands[2].result.type, 'failed_without_resource');
    assert.equal(commands.some((item) => item.identity?.id === startIdentity.id && item.result?.type === 'failed_without_resource'), false);
    const originReceipt = { disposition: 'closed', context: originCancel.command.closeContext, identity: startIdentity, owner, handle: 'recover-origin' };
    f.helper.stdout.write(`${JSON.stringify({ id: originCancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: originReceipt } })}\n`);
    for (let n = 0; n < 10 && commands.length < 4; n++) await tick();
    assert.deepEqual(commands[3].identity, startIdentity); assert.equal(commands[3].result.type, 'failed_without_resource');
  } finally {
    (runtime as any).audioOwnersByGeneration.clear(); (runtime as any).audioStartsByGeneration.clear();
    (runtime as any).audioUnconfirmedByGeneration.clear(); await runtime.dispose(); await f.close();
  }
});

test('two host-issued attachments for the same session retain independent capture teardown', async () => {
  const f = await fixture();
  const attached = (generation: number) => ({ type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc',
    attachment_generation: generation, logical_kind: 'session' as const, logical_owner_id: 's' });
  const a = attached(1); const b = attached(2);
  try {
    const openA = f.manager.startMediaCapture(a, 24_000); const startA = await envelope(f.helper, 1);
    f.helper.respond(startA, started('capture-a')); await openA;
    const openB = f.manager.startMediaCapture(b, 24_000); const startB = await envelope(f.helper, 2);
    f.helper.respond(startB, started('capture-b')); await openB;
    const endA = f.manager.endAudioOwner(a); const cancelA = await envelope(f.helper, 3);
    assert.deepEqual(cancelA.command.owner, a);
    const receiptA = { disposition: 'closed', context: cancelA.command.closeContext, identity: cancelA.command.identity, owner: a, handle: 'capture-a' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancelA.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receiptA } })}\n`);
    const endRequestA = await envelope(f.helper, 4); assert.deepEqual(endRequestA.request.owner, a);
    f.helper.respond(endRequestA, { type: 'owner_ended' }); await endA;
    assert.ok([...(f.manager as any).activeRecordingOrigins.values()].some((value: any) => value.handle === 'capture-b'));
    const endB = f.manager.endAudioOwner(b); const cancelB = await envelope(f.helper, 5);
    assert.deepEqual(cancelB.command.owner, b);
    const receiptB = { disposition: 'closed', context: cancelB.command.closeContext, identity: cancelB.command.identity, owner: b, handle: 'capture-b' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancelB.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receiptB } })}\n`);
    const endRequestB = await envelope(f.helper, 6); assert.deepEqual(endRequestB.request.owner, b);
    f.helper.respond(endRequestB, { type: 'owner_ended' }); await endB;
  } finally { await f.close(); }
});

test('real manager reserves a waiting attached audio identity and cancellation proves no dispatch', async () => {
  const f = await fixture(); const sent: any[] = [];
  const runtime = new SessionRuntime({ sessionId: 's', projectPath: '/workspace', audioService: f.manager as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId: 's', trusted: true }) });
  const state = runtime as any; const client = new EventEmitter() as any;
  client.sendCommand = (value: any) => sent.push(value); client.close = () => {};
  state.wireClient(client, 0);
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000150', generation: 1, service_epoch: 9 };
  try {
    state.requestAudioSessionContext(client, 0);
    client.emit('event', { type: 'audio_request', request: { identity, owner: attached, max_payload_bytes: 1024, operation: { type: 'status' } } });
    assert.equal((f.manager as any).reservedAudioAdmissions.size, 1);
    client.emit('event', { type: 'audio_cancel', identity });
    for (let i = 0; i < 50 && !sent.some(x => x.type === 'audio_response'); i++) await tick();
    const result = sent.find(x => x.type === 'audio_response');
    assert.equal(result?.result.type, 'failed_without_resource');
    assert.equal(f.helper.writes.length, 1, 'no audio request reached the helper');
    assert.equal((f.manager as any).reservedAudioAdmissions.size, 0);
  } finally { state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding(); await f.close(); }
});

test('Bridge wire handles native-invalid attached requests with an atomic freshness check', async () => {
  const f = await fixture(); const sent: any[] = [];
  const runtime = new SessionRuntime({ sessionId: 's', projectPath: '/workspace', audioService: f.manager as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId: 's', trusted: true }) });
  const state = runtime as any; const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => sent.push(JSON.parse(wire).payload.params) };
  state.wireClient(client, 0);
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = (id: string) => ({ id, generation: 1, service_epoch: 9 });
  const invalid = (id: string) => ({ identity: identity(id), owner: attached, max_payload_bytes: 1024, operation: { type: 'speak', text: 'hi', rate: 100 } });
  const deliver = (request: unknown) => {
    const event = { type: 'audio_request', request };
    assert.doesNotThrow(() => validateClientEvent(event));
    (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload: event })));
  };
  const responseFor = (id: string) => sent.find(x => x.type === 'audio_response' && x.identity.id === id);
  const fresh = '00000000-0000-4000-8000-000000000157';
  const duplicate = '00000000-0000-4000-8000-000000000158';
  const malformed = 'not-a-uuid';
  const reservation = f.manager.reserveAudioAdmission({ identity: identity(duplicate), owner: attached, max_payload_bytes: 1024, operation: { type: 'status' } });
  assert.ok(reservation);
  try {
    deliver(invalid(fresh)); deliver(invalid(duplicate)); deliver(invalid(malformed));
    for (let i = 0; i < 50 && !responseFor(malformed); i++) await tick();
    assert.equal(responseFor(fresh)?.result.type, 'failed_without_resource');
    assert.equal(responseFor(fresh)?.result.error.kind, 'invalid_request');
    assert.equal(responseFor(duplicate)?.result.type, 'failed');
    assert.equal(responseFor(malformed)?.result.type, 'failed');
    assert.equal((state.audioRequestsByGeneration.get(0) as Map<string, unknown>).size, 0);
    assert.equal(f.helper.writes.length, 1, 'invalid requests never reached the helper');
    assert.equal(f.manager.cancelReservedAudioAdmission(identity(duplicate), reservation), true, 'the conflicting reservation remains owned by its original caller');
  } finally { state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding(); await f.close(); }
});

test('ending an attached owner joins a dispatched pending start without killing the helper', async () => {
  const f = await fixture();
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000151', generation: 1, service_epoch: 9 };
  try {
    const opening = f.manager.executeAudioRequest({ identity, owner: attached, max_payload_bytes: 1024, operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } });
    await envelope(f.helper, 1);
    const ending = f.manager.endAudioOwner(attached); const cancel = await envelope(f.helper, 2);
    assert.equal(cancel.command.type, 'cancel_operation'); assert.equal(f.helper.killCount, 0);
    const receipt = { disposition: 'not_admitted', context: cancel.command.closeContext, identity, owner: attached };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    assert.equal((await opening).type, 'failed_without_resource');
    const end = await envelope(f.helper, 3); assert.deepEqual(end.request.owner, attached);
    f.helper.respond(end, { type: 'owner_ended' }); await ending;
    assert.equal(f.helper.killCount, 0);
  } finally { await f.close(); }
});

test('ending a preparing attached owner waits before helper dispatch and does not kill the helper', async () => {
  const helper = new Helper();
  const manager = new NativeAudioManager({ isPackaged: false, resourcesPath: '/resources', userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper', diagnostics: new DiagnosticBuffer(), spawnHelper: () => helper as any });
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  try {
    const opening = manager.executeAudioRequest({ identity: { id: '00000000-0000-4000-8000-000000000152', generation: 1, service_epoch: 9 }, owner: attached,
      max_payload_bytes: 1024, operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } });
    const capability = await envelope(helper, 0);
    const ending = manager.endAudioOwner(attached);
    assert.equal(helper.killCount, 0);
    helper.stdout.write(`${JSON.stringify({ id: capability.id, type: 'response', result: { type: 'snapshot', snapshot: snapshot() } })}\n`);
    assert.equal((await opening).type, 'failed_without_resource'); await ending;
    assert.equal(helper.writes.length, 1); assert.equal(helper.killCount, 0);
  } finally { await manager.dispose(); helper.stdin.end(); helper.stdout.end(); helper.stderr.end(); }
});

test('duplicate attached identity cannot cancel or tear down another runtime flight', async () => {
  const f = await fixture();
  const sentA: any[] = []; const sentB: any[] = [];
  const makeRuntime = (sent: any[]) => {
    const runtime = new SessionRuntime({ sessionId: 's', projectPath: '/workspace', audioService: f.manager as any,
      launchConfig: () => ({ workspace: '/workspace', sessionId: 's', trusted: true }) });
    const state = runtime as any; const client = new EventEmitter() as any;
    client.sendCommand = (value: any) => sent.push(value); client.close = () => {};
    state.wireClient(client, 0);
    return { runtime, state, client };
  };
  const a = makeRuntime(sentA); const b = makeRuntime(sentB);
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000153', generation: 1, service_epoch: 9 };
  const request = { identity, owner: attached, max_payload_bytes: 1024, operation: { type: 'start_recording', sample_rate_hz: 24_000, format: 'wav' } };
  try {
    a.state.requestAudioSessionContext(a.client, 0);
    a.client.emit('event', { type: 'audio_request', request });
    b.client.emit('event', { type: 'audio_request', request: structuredClone(request) });
    for (let i = 0; i < 50 && !sentB.some(x => x.type === 'audio_response'); i++) await tick();
    assert.equal(sentB.find(x => x.type === 'audio_response')?.result.type, 'failed');
    const context = sentA.find(x => x.type === 'get_audio_session_context');
    a.client.emit('event', { type: 'audio_session_context', request_id: context.request_id, session_id: 's', media_owner: attached });
    await envelope(f.helper, 1);
    b.client.emit('event', { type: 'audio_cancel', identity });
    await b.state.teardownAudioGeneration(0); await tick();
    assert.equal(f.helper.writes.length, 2, 'duplicate runtime must not cancel or end the admitted owner');
    assert.equal(f.helper.killCount, 0);
    assert.equal(await f.manager.cancelAudioRequest(identity, {}), undefined, 'a different token cannot cancel the flight');
    a.client.emit('event', { type: 'audio_cancel', identity });
    const cancel = await envelope(f.helper, 2);
    const receipt = { disposition: 'not_admitted', context: cancel.command.closeContext, identity, owner: attached };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    for (let i = 0; i < 50 && !sentA.some(x => x.type === 'audio_response'); i++) await tick();
    assert.equal(sentA.find(x => x.type === 'audio_response')?.result.type, 'failed_without_resource');
  } finally {
    for (const { state } of [a, b]) { state.audioOwnersByGeneration.clear(); state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding(); }
    await f.close();
  }
});

test('ending a hosted capture joins its own task and exact recording without recursive teardown', async () => {
  const f = await fixture();
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000154', generation: 1, service_epoch: 9 };
  try {
    const request = { identity, owner: attached, max_payload_bytes: 1024,
      operation: { type: 'capture' as const, sample_rate_hz: 24_000, format: 'wav' as const } };
    const token = f.manager.reserveAudioAdmission(request); assert.ok(token);
    let closeProofs = 0;
    const capturing = f.manager.executeAudioRequestWithCloseReceipt(request, () => { closeProofs += 1; }, token);
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('hosted-capture')); await tick();
    const ending = f.manager.endAudioOwner(attached);
    const endingAgain = f.manager.endAudioOwner(attached);
    const cancel = await envelope(f.helper, 2);
    assert.equal(cancel.command.type, 'cancel_operation'); assert.equal(f.helper.killCount, 0); assert.equal(closeProofs, 0);
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity: cancel.command.identity, owner: attached, handle: 'hosted-capture' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    const end = await envelope(f.helper, 3); assert.deepEqual(end.request.owner, attached);
    f.helper.respond(end, { type: 'owner_ended' }); await Promise.all([ending, endingAgain]);
    assert.equal((await capturing).type, 'failed'); assert.equal(f.helper.killCount, 0); assert.equal(closeProofs, 1);
    assert.equal((f.manager as any).uncertainAudioRequests.size, 0);
  } finally { await f.close(); }
});

test('hosted capture cancellation reports cleanup only after its exact native receipt and task settle', async () => {
  const f = await fixture();
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000155', generation: 1, service_epoch: 9 };
  const request = { identity, owner: attached, max_payload_bytes: 1024,
    operation: { type: 'capture' as const, sample_rate_hz: 24_000, format: 'wav' as const } };
  try {
    const token = f.manager.reserveAudioAdmission(request); assert.ok(token);
    let closeProofs = 0;
    const capturing = f.manager.executeAudioRequestWithCloseReceipt(request, () => { closeProofs += 1; }, token);
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('cancelled-capture')); await tick();
    await assert.rejects(f.manager.cancelAudioRequest(identity, token), /unconfirmed/);
    const cancel = await envelope(f.helper, 2); assert.equal(closeProofs, 0);
    const receipt = { disposition: 'closed', context: cancel.command.closeContext, identity: cancel.command.identity, owner: attached, handle: 'cancelled-capture' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    assert.equal((await capturing).type, 'failed'); await tick();
    assert.equal(closeProofs, 1); assert.equal((f.manager as any).uncertainAudioRequests.size, 0); assert.equal(f.helper.killCount, 0);
  } finally { await f.close(); }
});

test('hosted capture owner teardown does not report proof for a mismatched native receipt', async () => {
  const f = await fixture();
  const attached = { type: 'attached' as const, attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session' as const, logical_owner_id: 's' };
  const identity = { id: '00000000-0000-4000-8000-000000000156', generation: 1, service_epoch: 9 };
  const request = { identity, owner: attached, max_payload_bytes: 1024,
    operation: { type: 'capture' as const, sample_rate_hz: 24_000, format: 'wav' as const } };
  try {
    const token = f.manager.reserveAudioAdmission(request); assert.ok(token);
    let closeProofs = 0;
    const capturing = f.manager.executeAudioRequestWithCloseReceipt(request, () => { closeProofs += 1; }, token);
    const start = await envelope(f.helper, 1); f.helper.respond(start, started('unknown-capture')); await tick();
    const ending = f.manager.endAudioOwner(attached); const cancel = await envelope(f.helper, 2);
    const receipt = { disposition: 'closed', context: { ...cancel.command.closeContext, flight_id: 'wrong' }, identity: cancel.command.identity, owner: attached, handle: 'unknown-capture' };
    f.helper.stdout.write(`${JSON.stringify({ id: cancel.id, type: 'response', result: { type: 'close_receipt', snapshot: snapshot(), closeReceipt: receipt } })}\n`);
    await assert.rejects(ending, /could not be ended/); await capturing;
    assert.equal(closeProofs, 0); assert.ok((f.manager as any).uncertainAudioRequests.size > 0);
  } finally { await f.close(); }
});
