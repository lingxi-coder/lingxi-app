import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';
import { NativeMediaDevices } from '../src/main/media/nativeMediaDevices.js';

const owner = { type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc', attachment_generation: 1, logical_kind: 'session', logical_owner_id: 'session-a' } as const;
const open = (id: string) => ({ request_id: id, request: { type: 'open', request: { identity: { id: `op-${id}`, generation: 1, service_epoch: 1 }, owner, max_payload_bytes: 1024, request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'media-runtime:microphone' } } } } } as const);
function runtimeFor(audio: any) {
  audio.getCapabilities ??= () => undefined;
  audio.onEvent ??= () => () => undefined;
  const sent: any[] = [];
  const runtime = new SessionRuntime({ sessionId: 'session-a', projectPath: '/workspace', audioService: audio,
    launchConfig: () => ({ workspace: '/workspace', sessionId: 'session-a', trusted: true }) });
  const state = runtime as any;
  const client = { sendCommand: (command: any) => sent.push(command) };
  state.client = client; state.nativeMediaClient = client; state.generation = 0;
  state.requestAudioSessionContext = () => {};
  return { state, client, sent, dispatch: (id: string) => state.dispatchMediaDeviceRequest(client, 0, open(id)) as Promise<void> };
}

test('a fresh no-wrapper open proves non-dispatch while duplicates remain silent', async () => {
  const service = { hasMediaRequestId: () => false, executeMediaDevice: async () => { throw new Error('must not dispatch'); } };
  const f = runtimeFor(service);
  await f.dispatch('fresh'); await f.dispatch('fresh');
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0].response.request_id, 'fresh');
  assert.equal(f.sent[0].response.result.type, 'open_failed_without_resource');
  const uninspectable = runtimeFor({ executeMediaDevice: service.executeMediaDevice });
  await uninspectable.dispatch('unknown-service');
  assert.equal(uninspectable.sent[0].response.result.type, 'error');
});

test('two runtimes reserve the same open before either awaits context', async () => {
  const service = { hasMediaRequestId: () => false, executeMediaDevice: async () => { throw new Error('must not dispatch'); } };
  const first = runtimeFor(service); const second = runtimeFor(service);
  let resolve!: () => void;
  const done = new Promise<void>(r => { resolve = r; });
  first.state.pendingAudioSessionContext = { done };
  const waiting = first.dispatch('concurrent');
  await second.dispatch('concurrent');
  assert.equal(second.sent.length, 0);
  first.state.pendingAudioSessionContext = undefined; resolve(); await waiting;
  assert.equal(first.sent[0].response.result.type, 'open_failed_without_resource');
});

test('unknown opens survive completed-history rollover and stale release tokens', async () => {
  let calls = 0;
  const service = { hasMediaRequestId: () => false, executeMediaDevice: async (request: any) => {
    calls++; return { request_id: request.request_id, result: { type: 'error', error: { kind: 'native_failure', message: 'unknown' } } };
  } };
  const f = runtimeFor(service);
  f.state.nativeMediaDevices = new NativeMediaDevices({ owner, audio: service });
  await f.dispatch('unknown');
  assert.equal(f.sent[0].response.result.type, 'error'); assert.equal(calls, 1);
  f.state.nativeMediaDevices = undefined;
  for (let i = 0; i < 4097; i++) await f.dispatch(`fresh-${i}`);
  const before = f.sent.length;
  await f.dispatch('unknown');
  assert.equal(f.sent.length, before);
  const old = {}; const current = {};
  f.state.unsettledMediaOpenIds.set('token-test', current);
  f.state.finishMediaOpenRequest('token-test', old);
  assert.equal(f.state.unsettledMediaOpenIds.get('token-test'), current);
});
