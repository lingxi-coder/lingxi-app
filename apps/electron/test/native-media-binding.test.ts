import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient, validateClientEvent } from '@lingxi/bridge-client';
import type { MediaDeviceRequestEnvelopeDto } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const attachment = (generation: number) => ({ type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc',
  attachment_generation: generation, logical_kind: 'session', logical_owner_id: sessionId });
const open = (id: string, generation: number) => ({ request_id: id, request: { type: 'open', request: {
  identity: { id: `op-${id}`, generation: 1, service_epoch: 1 }, owner: attachment(generation), max_payload_bytes: 1024,
  request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'media-runtime:microphone' } },
} } });
async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 1_000;
  while (!check() && Date.now() < deadline) await new Promise((resolve) => setImmediate(resolve));
  assert.ok(check(), 'timed out waiting for media binding');
}

test('audio context requires correlation and a valid optional media owner', () => {
  assert.deepEqual(validateClientEvent({ type: 'audio_session_context', request_id: 'query', session_id: null,
    profile_id: null, account_scope: null, media_owner: null }), { type: 'audio_session_context', request_id: 'query' });
  assert.throws(() => validateClientEvent({ type: 'audio_session_context', session_id: sessionId, media_owner: attachment(1) }));
  assert.throws(() => validateClientEvent({ type: 'audio_session_context', request_id: 'query', session_id: sessionId,
    media_owner: { ...attachment(1), attachment_generation: true } }));
});

test('a service without confirmed native open-flight cleanup cannot install a binding', async () => {
  const commands: any[] = []; let dispatched = 0;
  const audio = { getCapabilities: () => undefined, onEvent: () => () => undefined, hasMediaRequestId: () => false,
    executeMediaDevice: async () => { dispatched++; throw new Error('must remain closed'); } };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const internal = runtime as any;
  const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  internal.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  internal.requestAudioSessionContext(client, 0);
  deliver({ type: 'audio_session_context', request_id: commands[0].request_id, session_id: sessionId, media_owner: attachment(1) });
  deliver({ type: 'media_device_request', request: open('without-flight-receipt', 1) });
  await until(() => commands.some((command) => command.type === 'media_device_response'));
  assert.equal(commands.find((command) => command.type === 'media_device_response').response.result.type, 'open_failed_without_resource');
  assert.equal(dispatched, 0);
  internal.nativeMediaClient = undefined;
  internal.invalidateNativeMediaBinding();
});

test('BridgeClient frame decoding fences old contexts and waits for native cleanup', async () => {
  const commands: any[] = []; const calls: MediaDeviceRequestEnvelopeDto[] = []; const broadcasts: any[] = [];
  let releaseStop!: () => void;
  const stopGate = new Promise<void>((resolve) => { releaseStop = resolve; });
  let releaseFlight!: () => void;
  const flightGate = new Promise<void>((resolve) => { releaseFlight = resolve; });
  let drainCalls = 0;
  let stopHeld = false;
  const audio = {
    getCapabilities: () => undefined, getMediaSessionModes: () => ['audio'], onEvent: () => () => undefined,
    endAudioOwner: async () => {}, hasMediaRequestId: () => false,
    drainMediaOpenFlights: async () => { if (++drainCalls > 1) await flightGate; },
    executeMediaDevice: async (request: MediaDeviceRequestEnvelopeDto) => {
      calls.push(request);
      if (request.request.type === 'open') return { request_id: request.request_id,
        result: { type: 'opened', lease: { lease_id: `lease-${calls.length}` } } };
      if (!stopHeld) { stopHeld = true; await stopGate; }
      return { request_id: request.request_id, result: { type: 'stopped' } };
    },
  };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const internal = runtime as any;
  internal.broadcastClientEvent = (event: unknown) => broadcasts.push(event);
  const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  internal.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  const contexts = () => commands.filter((command) => command.type === 'get_audio_session_context');
  const responses = () => commands.filter((command) => command.type === 'media_device_response');
  try {
    internal.requestAudioSessionContext(client, 0);
    const first = contexts()[0].request_id;
    deliver({ type: 'media_device_request', request: open('first', 1) });
    deliver({ type: 'audio_session_context', request_id: 'wrong', session_id: sessionId, media_owner: attachment(1) });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls.length, 0);
    deliver({ type: 'audio_session_context', request_id: first, session_id: sessionId, media_owner: attachment(1) });
    await until(() => responses().length === 1);
    assert.equal(responses()[0].response.result.type, 'opened');

    deliver({ type: 'session_started', session_id: sessionId, mode: 'chat' });
    await until(() => calls.length === 2);
    deliver({ type: 'model_changed', model: 'test-model' });
    const stale = contexts()[1].request_id; const latest = contexts()[2].request_id;
    deliver({ type: 'audio_session_context', request_id: stale, session_id: sessionId, media_owner: attachment(1) });
    deliver({ type: 'audio_session_context', request_id: latest, session_id: sessionId, media_owner: attachment(2) });
    deliver({ type: 'media_device_request', request: open('second', 2) });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls.length, 2);
    releaseStop();
    await until(() => drainCalls === 2);
    assert.equal(calls.length, 2, 'a terminal open-flight receipt is also required after lease stop');
    releaseFlight();
    await until(() => responses().length === 2);
    assert.equal(responses()[1].response.result.type, 'opened');
    assert.deepEqual((calls[2].request as any).request.owner, attachment(2));
    deliver({ type: 'media_device_request', request: open('replay', 1) });
    await until(() => responses().length === 3);
    assert.equal(responses()[2].response.result.type, 'open_failed_without_resource');
    assert.equal(calls.length, 3);
    assert.equal(broadcasts.some((event) => event.type === 'audio_session_context'), false);
  } finally {
    releaseStop();
    releaseFlight();
    internal.nativeMediaClient = undefined;
    internal.invalidateNativeMediaBinding();
    await internal.nativeMediaDeviceUpdate;
  }
});

test('unacknowledged native cleanup prevents replacement and a later request retries', async () => {
  const commands: any[] = []; const calls: MediaDeviceRequestEnvelopeDto[] = [];
  let stopAttempts = 0;
  const audio = {
    getCapabilities: () => undefined, getMediaSessionModes: () => ['audio'], onEvent: () => () => undefined,
    endAudioOwner: async () => {}, hasMediaRequestId: () => false,
    drainMediaOpenFlights: async () => undefined,
    executeMediaDevice: async (request: MediaDeviceRequestEnvelopeDto) => {
      calls.push(request);
      if (request.request.type === 'open') return { request_id: request.request_id,
        result: { type: 'opened', lease: { lease_id: `lease-${calls.length}` } } };
      if (++stopAttempts <= 2) return { request_id: request.request_id,
        result: { type: 'error', error: { kind: 'native_failure', message: 'not stopped' } } };
      return { request_id: request.request_id, result: { type: 'stopped' } };
    },
  };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const internal = runtime as any;
  const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  internal.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  const contexts = () => commands.filter((command) => command.type === 'get_audio_session_context');
  const responses = () => commands.filter((command) => command.type === 'media_device_response');
  const respond = (index: number, generation: number) => deliver({ type: 'audio_session_context',
    request_id: contexts()[index].request_id, session_id: sessionId, media_owner: attachment(generation) });
  try {
    internal.requestAudioSessionContext(client, 0);
    respond(0, 1);
    deliver({ type: 'media_device_request', request: open('initial', 1) });
    await until(() => responses().length === 1);
    deliver({ type: 'model_changed', model: 'new' });
    respond(1, 2);
    deliver({ type: 'media_device_request', request: open('unacknowledged', 2) });
    await until(() => responses().length === 2);
    assert.equal(responses()[1].response.result.type, 'open_failed_without_resource');
    assert.equal(calls.filter((request) => request.request.type === 'open').length, 1);
    assert.equal(stopAttempts, 1);
    deliver({ type: 'media_device_request', request: open('retry-fails', 2) });
    assert.equal(contexts().length, 3, 'a temporary failure starts a fresh authenticated lookup');
    respond(2, 2);
    await until(() => responses().length === 3);
    assert.equal(responses()[2].response.result.type, 'open_failed_without_resource');
    assert.equal(stopAttempts, 2);
    deliver({ type: 'media_device_request', request: open('retry', 2) });
    assert.equal(contexts().length, 4);
    respond(3, 2);
    await until(() => responses().length === 4);
    assert.equal(responses()[3].response.result.type, 'opened');
    assert.equal(stopAttempts, 3);
  } finally {
    internal.nativeMediaClient = undefined;
    internal.invalidateNativeMediaBinding();
    await internal.nativeMediaDeviceUpdate;
  }
});
