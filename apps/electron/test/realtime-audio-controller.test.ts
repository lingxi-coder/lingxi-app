import assert from 'node:assert/strict';
import { test } from 'node:test';
import { RealtimeAudioController, type RealtimeAudioRuntime } from '../src/main/audio/realtimeAudioController';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import type { NativeAudioEvent } from '../src/shared/nativeAudio';
import type { NativeRealtimeAudioState } from '../src/shared/realtimeAudio';
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
function setup() {
  let event: ((eventJson: string) => void) | undefined;
  let device: ((event: NativeAudioEvent) => void) | undefined;
  const inputs: any[] = []; const starts: any[] = []; const states: NativeRealtimeAudioState[] = [];
  const playback = deferred<{ type: 'playback_completed'; duration_ms: number }>();
  const runtime: RealtimeAudioRuntime = { sessionId: 's', realtimeAudioSupported: true,
    onRealtimeAudioEvent: (callback) => { event = callback; return () => { event = undefined; }; },
    startRealtimeAudio: (json) => starts.push(JSON.parse(json)), realtimeAudioInput: (json) => inputs.push(JSON.parse(json)), stopRealtimeAudio: () => undefined };
  let current: RealtimeAudioRuntime | undefined = runtime;
  let captureCount = 0; const played: string[] = [];
  const controller = new RealtimeAudioController({ currentRuntime: () => current, publish: (state) => states.push(state), audio: {
    onEvent: (callback) => { device = callback; return () => { device = undefined; }; },
    startRealtimeCapture: async () => { captureCount++; return { type: 'recording_started', handle: 'h' }; },
    stopRealtimeCapture: async () => undefined,
    playRealtimeAudio: async (_session, pcm) => { played.push(pcm); return playback.promise; },
  } });
  return { controller, runtime, inputs, starts, states, played, playback, send: (item: unknown) => event?.(JSON.stringify({ operationId: starts.at(-1)?.operationId, ...(item as object) })), capture: (item: NativeAudioEvent) => device?.(item), setCurrent: (value?: RealtimeAudioRuntime) => { current = value; }, captures: () => captureCount };
}
const ready = { type: 'session_ready', model: 'realtime', inputFormat: { encoding: 'pcm16', sampleRateHz: 24_000, channels: 1 }, outputFormat: { encoding: 'pcm16', sampleRateHz: 24_000, channels: 1 }, capabilities: {} };
const delta = { type: 'audio_delta', audioBase64: 'AAA=', sampleRateHz: 24_000, channels: 1, encoding: 'pcm16', itemId: 'item' };

test('realtime playback completion is acknowledged only after device playback, then capture resumes', async () => {
  const f = setup(); const config = audioConfigurationDefaults(); config.conversation.mode = 'realtime';
  await f.controller.start(config); f.send(ready); await tick();
  assert.equal(f.captures(), 1); await f.controller.commit();
  assert.deepEqual(f.inputs, [{ type: 'commit', operationId: f.starts[0].operationId }]);
  f.send(delta); f.send({ type: 'turn_completed' }); await tick();
  assert.equal(f.captures(), 1); assert.deepEqual(f.played, ['AAA=']);
  f.playback.resolve({ type: 'playback_completed', duration_ms: 20 }); await tick(); await tick();
  assert.deepEqual(f.inputs[1], { type: 'playback_completed', operationId: f.starts[0].operationId, itemId: 'item' }); assert.equal(f.captures(), 2);
  assert.equal(f.starts[0].cloud.binding, 'follow_session'); assert.equal(f.starts[0].session, undefined);
  await f.controller.stop();
});

test('stale session events cannot play provider audio in the new active session', async () => {
  const f = setup(); await f.controller.start(audioConfigurationDefaults()); f.setCurrent(undefined); f.send(delta); await tick();
  assert.deepEqual(f.played, []); assert.equal(f.states.at(-1)?.phase, 'paused');
});

test('interruptible mode requires actual native echo cancellation and fails before provider start', async () => {
  const f = setup(); const config = audioConfigurationDefaults(); config.conversation.interaction = 'interruptible';
  await assert.rejects(() => f.controller.start(config), /回声消除/); assert.deepEqual(f.starts, []);
});

test('ordered device input stays on the authenticated runtime lane and sequence gaps stop the session', async () => {
  const f = setup(); await f.controller.start(audioConfigurationDefaults()); f.send(ready); await tick();
  const chunk = { type: 'capture_chunk' as const, owner: { kind: 'session' as const, id: 's' }, identity: { id: '00000000-0000-4000-8000-000000000099', generation: 1, service_epoch: 2 }, sequence: 0, pcmBase64: 'AAA=', sampleRateHz: 24_000 };
  f.capture(chunk); assert.deepEqual(f.inputs[0], { type: 'audio', operationId: f.starts[0].operationId, audioBase64: 'AAA=' });
  f.capture({ ...chunk, sequence: 2 }); await tick(); assert.equal(f.states.at(-1)?.phase, 'failed'); assert.equal(f.inputs.length, 1);
});

test('same-session output carries an operation fence so old queued audio cannot enter replacement playback', async () => {
  const f = setup(); await f.controller.start(audioConfigurationDefaults());
  const oldOperation = f.starts[0].operationId;
  await f.controller.start(audioConfigurationDefaults());
  f.send({ ...delta, operationId: oldOperation }); await tick();
  assert.deepEqual(f.played, []); await f.controller.stop();
});


test('transport backpressure ends input and releases capture even when the socket is disconnected', async () => {
  const f = setup(); await f.controller.start(audioConfigurationDefaults()); f.send(ready); await tick();
  f.runtime.realtimeAudioInput = () => { throw new Error('Realtime audio input queue is full'); };
  f.runtime.stopRealtimeAudio = () => { throw new Error('bridge client not connected'); };
  f.capture({ type: 'capture_chunk', owner: { kind: 'session', id: 's' }, identity: { id: '00000000-0000-4000-8000-000000000099', generation: 1, service_epoch: 2 }, sequence: 0, pcmBase64: 'AAA=', sampleRateHz: 24_000 });
  await tick(); assert.equal(f.states.at(-1)?.phase, 'failed'); assert.match(f.states.at(-1)?.detail ?? '', /queue is full/);
  assert.deepEqual(f.inputs, []);
});
