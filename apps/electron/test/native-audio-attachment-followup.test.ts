import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient, validateClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const owner = (generation: number) => ({ type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: generation, logical_kind: 'session', logical_owner_id: sessionId });
const request = (id: string, generation: number) => ({ identity: { id, generation: 1, service_epoch: 1 }, owner: owner(generation), max_payload_bytes: 1024, operation: { type: 'status' } });
async function until(check: () => boolean): Promise<void> { const end = Date.now() + 2000; while (!check() && Date.now() < end) await new Promise(r => setImmediate(r)); assert.ok(check()); }

test('only authenticated attachment enters audio; same-session rotation joins its exact old owner', async () => {
  const commands: any[] = []; const calls: any[] = []; const ended: any[] = []; let release!: () => void;
  const closed = new Promise<void>(resolve => { release = resolve; });
  const audio = { getCapabilities: () => undefined, onEvent: () => () => {}, drainMediaOpenFlights: async () => {},
    hasMediaRequestId: () => false, executeMediaDevice: async () => { throw new Error('unused'); },
    endAudioOwner: async (value: any) => { ended.push(value); await closed; },
    reserveAudioAdmission: () => ({}), cancelReservedAudioAdmission: () => true,
    rejectAudioRequestBeforeDispatch: () => true, cancelAudioRequest: async () => 'verified',
    executeAudioRequestWithCloseReceipt: async (value: any) => { calls.push(value); return { type: 'status', status: { recording: false, playing: false } }; },
  };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const state = runtime as any; const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  state.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  const contexts = () => commands.filter(x => x.type === 'get_audio_session_context');
  const replies = () => commands.filter(x => x.type === 'audio_response');
  try {
    assert.deepEqual((validateClientEvent({ type: 'audio_request', request: request('wire', 1) }) as any).request.owner, owner(1));
    state.requestAudioSessionContext(client, 0);
    deliver({ type: 'audio_request', request: request('a', 1) });
    assert.equal(calls.length, 0);
    deliver({ type: 'audio_session_context', request_id: contexts()[0].request_id, session_id: sessionId, media_owner: owner(1) });
    await until(() => calls.length === 1); assert.deepEqual(calls[0].owner, owner(1));
    deliver({ type: 'model_changed', model: 'same' });
    deliver({ type: 'audio_session_context', request_id: contexts()[1].request_id, session_id: sessionId, media_owner: owner(1) });
    await until(() => !!state.attachedAudioBinding); assert.equal(ended.length, 0);
    deliver({ type: 'model_changed', model: 'rotate' });
    deliver({ type: 'audio_session_context', request_id: contexts()[2].request_id, session_id: sessionId, media_owner: owner(2) });
    deliver({ type: 'audio_request', request: request('b', 2) });
    await until(() => ended.length === 1); assert.deepEqual(ended[0], owner(1)); assert.equal(calls.length, 1);
    release(); await until(() => calls.length === 2); assert.deepEqual(calls[1].owner, owner(2));
    deliver({ type: 'audio_request', request: request('stale', 1) });
    await until(() => replies().some(x => x.identity.id === 'stale'));
    assert.equal(replies().find(x => x.identity.id === 'stale').result.type, 'failed_without_resource');
    assert.equal(calls.length, 2);
  } finally { release(); state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding(); }
});

test('cancellation while attachment context is pending cannot dispatch the old request', async () => {
  const commands: any[] = []; const calls: any[] = []; const seen = new Set<string>();
  const audio = { getCapabilities: () => undefined, onEvent: () => () => {}, drainMediaOpenFlights: async () => {},
    executeMediaDevice: async () => { throw new Error('unused'); }, endAudioOwner: async () => {}, hasMediaRequestId: () => false,
    reserveAudioAdmission: () => ({}), cancelReservedAudioAdmission: (identity: any) => { seen.add(identity.id); return true; },
    cancelAudioRequest: async (identity: any) => { seen.add(identity.id); return 'verified'; },
    rejectAudioRequestBeforeDispatch: (identity: any) => { if (seen.has(identity.id)) return false; seen.add(identity.id); return true; },
    executeAudioRequestWithCloseReceipt: async (value: any) => { calls.push(value); return { type: 'status', status: { recording: false, playing: false } }; },
  };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const state = runtime as any; const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  state.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  try {
    state.requestAudioSessionContext(client, 0);
    const target = request('cancel-me', 1);
    deliver({ type: 'audio_request', request: target });
    deliver({ type: 'audio_cancel', identity: target.identity });
    const context = commands.find(x => x.type === 'get_audio_session_context');
    deliver({ type: 'audio_session_context', request_id: context.request_id, session_id: sessionId, media_owner: owner(1) });
    await until(() => commands.some(x => x.type === 'audio_response' && x.identity.id === 'cancel-me'));
    assert.equal(calls.length, 0);
  } finally { state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding(); }
});

test('an untracked audio cancellation cannot reach the shared manager', async () => {
  const cancelled: any[] = [];
  const audio = { getCapabilities: () => undefined, onEvent: () => () => {},
    cancelAudioRequest: async (id: any) => { cancelled.push(id); return 'verified'; } };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const state = runtime as any; const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: () => {} };
  state.wireClient(client, 0);
  client.emit('event', { type: 'audio_cancel', identity: request('another-runtime', 1).identity });
  await new Promise(r => setImmediate(r));
  assert.deepEqual(cancelled, []);
  state.nativeMediaClient = undefined; state.invalidateNativeMediaBinding();
});
