import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { MediaDeviceRequestEnvelopeDto, MediaDeviceResponseDto } from '@lingxi/bridge-client';
import { NativeMediaDevices } from '../src/main/media/nativeMediaDevices.js';
import { validateMediaDeviceRequest, validateMediaDeviceResponse } from '../src/shared/nativeMedia.js';

const owner = { type: 'session' as const, session_id: 'session-a' };
const open = (requestId = 'open-a'): MediaDeviceRequestEnvelopeDto => ({
  request_id: requestId,
  request: { type: 'open', request: {
    identity: { id: 'operation-a', generation: 1, service_epoch: 10 }, owner,
    max_payload_bytes: 1024,
    request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'microphone' } },
  } },
});
const opened = (requestId = 'open-a', lease = 'lease-a'): MediaDeviceResponseDto => ({
  request_id: requestId, result: { type: 'opened', lease: { lease_id: lease } },
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

test('native media rejects partial multichannel PCM, oversized layouts and nonfinite gain before dispatch', () => {
  assert.throws(() => validateMediaDeviceRequest({ request_id: 'write', request: {
    type: 'write_audio', request: { lease_id: 'lease', frame: { channels: 2, sample_rate_hz: 48_000, pcm16le: [0, 0] } },
  } }), /partial sample/);
  assert.throws(() => validateMediaDeviceRequest({ request_id: 'write', request: {
    type: 'write_video', request: { lease_id: 'lease', frame: { type: 'rgba8', width_px: 100, height_px: 100, stride_bytes: 400, bytes: [0] } },
  } }), /RGBA/);
  assert.throws(() => validateMediaDeviceRequest({ request_id: 'change', request: {
    type: 'update', request: { lease_id: 'lease', update: { type: 'set_playback_gain', gain: Number.NaN } },
  } }), /gain/);
  assert.throws(() => validateMediaDeviceResponse({ request_id: 'response', result: {
    type: 'input', frame: { source: { id: 'mic' }, sequence: -1, captured_at_unix_ms: 1, payload: { type: 'audio', frame: { channels: 1, sample_rate_hz: 48_000, pcm16le: [0, 0] } } },
  } }), /integer/);
  assert.throws(() => validateMediaDeviceResponse({ request_id: 'response', result: {
    type: 'audio_flushed', receipt: { accepted_sample_frames: 1, played_sample_frames: 1 },
  } }), /barrier/);
  assert.doesNotThrow(() => validateMediaDeviceResponse({ request_id: 'response', result: {
    type: 'audio_interrupted', receipt: { accepted_sample_frames: 0, played_sample_frames: 1 },
  } }));
});

test('a native bridge cannot open another owner or use another connection lease', async () => {
  const calls: MediaDeviceRequestEnvelopeDto[] = [];
  const audio = { executeMediaDevice: async (request: MediaDeviceRequestEnvelopeDto) => {
    calls.push(request); return opened(request.request_id);
  } };
  const first = new NativeMediaDevices({ audio, owner });
  const second = new NativeMediaDevices({ audio, owner: { type: 'session', session_id: 'session-b' } });
  const foreign = await second.request(open());
  assert.equal(foreign.result.type, 'open_failed_without_resource');
  assert.equal(calls.length, 0);
  assert.equal((await first.request(open())).result.type, 'opened');
  const guessed = await second.request({ request_id: 'guess', request: { type: 'stop', lease_id: 'lease-a' } });
  assert.deepEqual(guessed.result, { type: 'error', error: { kind: 'not_found', message: 'The native media lease was not found' } });
  assert.equal(calls.length, 1);
});

test('duplicate in-flight request shares one native operation and conflicting reuse never dispatches', async () => {
  const gate = deferred<MediaDeviceResponseDto>(); let calls = 0;
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async () => { calls++; return gate.promise; } } });
  const first = bridge.request(open());
  const duplicate = bridge.request(open());
  assert.equal(first, duplicate);
  const changed = open();
  if (changed.request.type === 'open') changed.request.request.identity.generation = 2;
  assert.equal((await bridge.request(changed)).result.type, 'error');
  assert.equal(calls, 1);
  gate.resolve(opened()); await first;
  assert.equal((await bridge.request(open())).result.type, 'error');
  assert.equal(calls, 1);
});

test('closing a connection stops a native lease that finishes opening later', async () => {
  const gate = deferred<MediaDeviceResponseDto>(); const stops: string[] = [];
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
    if (request.request.type === 'open') return gate.promise;
    if (request.request.type === 'stop') stops.push(request.request.lease_id);
    return { request_id: request.request_id, result: { type: 'stopped' } };
  } } });
  const pending = bridge.request(open());
  const closing = bridge.dispose(); gate.resolve(opened());
  await closing;
  const result = await pending;
  assert.deepEqual(stops, ['lease-a']);
  assert.equal(result.request_id, 'open-a');
  assert.equal(result.result.type, 'open_failed_without_resource');
  if (result.result.type === 'open_failed_without_resource') assert.equal(result.result.error.kind, 'cancelled');
});

test('capture permits one reader and rejects replayed frames with confirmed native cleanup', async () => {
  const gate = deferred<MediaDeviceResponseDto>(); let reads = 0; let stopped = 0;
  const frame = { source: { id: 'microphone' }, sequence: 1, captured_at_unix_ms: 123,
    payload: { type: 'audio' as const, frame: { channels: 1, sample_rate_hz: 48_000, pcm16le: [0, 0] } } };
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
    if (request.request.type === 'open') return opened(request.request_id);
    if (request.request.type === 'stop') { stopped++; return { request_id: request.request_id, result: { type: 'stopped' } }; }
    reads++;
    if (reads === 1) return gate.promise;
    return { request_id: request.request_id, result: { type: 'input', frame } };
  } } });
  await bridge.request(open());
  const first = bridge.request({ request_id: 'read-a', request: { type: 'next_input', lease_id: 'lease-a' } });
  const second = await bridge.request({ request_id: 'read-b', request: { type: 'next_input', lease_id: 'lease-a' } });
  assert.equal(second.result.type, 'error'); assert.equal(reads, 1);
  gate.resolve({ request_id: 'read-a', result: { type: 'input', frame } }); await first;
  const replay = await bridge.request({ request_id: 'read-c', request: { type: 'next_input', lease_id: 'lease-a' } });
  assert.equal(replay.result.type, 'error'); assert.equal(stopped, 1);
});

test('playback receipts cannot claim more samples than were submitted', async () => {
  let stopped = false;
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
    if (request.request.type === 'open') return opened(request.request_id);
    if (request.request.type === 'stop') { stopped = true; return { request_id: request.request_id, result: { type: 'stopped' } }; }
    return { request_id: request.request_id, result: { type: 'audio_written', receipt: {
      accepted_sample_frames: 2, played_sample_frames: 2, rendered_at_unix_ms: 123,
    } } };
  } } });
  await bridge.request(open());
  const result = await bridge.request({ request_id: 'write-a', request: { type: 'write_audio', request: {
    lease_id: 'lease-a', frame: { channels: 1, sample_rate_hz: 48_000, pcm16le: [0, 0] },
  } } });
  assert.equal(result.result.type, 'error'); assert.equal(stopped, true);
});

test('native stop failure remains an observable cleanup failure', async () => {
  let attempts = 0;
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => request.request.type === 'open'
    ? opened(request.request_id) : ++attempts === 1
      ? { request_id: request.request_id, result: { type: 'error', error: { kind: 'native_failure', message: 'device did not stop' } } }
      : { request_id: request.request_id, result: { type: 'stopped' } },
  } });
  await bridge.request(open());
  await assert.rejects(bridge.dispose(), /not acknowledged/);
  await bridge.dispose();
  assert.equal(attempts, 2);
});

test('failed cleanup after invalid capture quarantines the lease and remains retryable', async () => {
  let stopAttempts = 0; let reads = 0;
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
    if (request.request.type === 'open') return opened(request.request_id);
    if (request.request.type === 'stop') {
      if (++stopAttempts === 1) throw new Error('native cleanup failed');
      return { request_id: request.request_id, result: { type: 'stopped' } };
    }
    reads++;
    return { request_id: 'incorrect-correlation', result: { type: 'input', frame: null } };
  } } });
  await bridge.request(open());
  const invalid = await bridge.request({ request_id: 'read-a', request: { type: 'next_input', lease_id: 'lease-a' } });
  assert.equal(invalid.result.type, 'error');
  const quarantined = await bridge.request({ request_id: 'read-b', request: { type: 'next_input', lease_id: 'lease-a' } });
  assert.equal(quarantined.result.type, 'error');
  assert.equal(reads, 1);
  await bridge.dispose();
  assert.equal(stopAttempts, 2);
});

test('a late open whose stop fails remains tracked for disposal retry', async () => {
  const gate = deferred<MediaDeviceResponseDto>(); let stopAttempts = 0;
  const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
    if (request.request.type === 'open') return gate.promise;
    if (++stopAttempts === 1) throw new Error('native cleanup failed');
    return { request_id: request.request_id, result: { type: 'stopped' } };
  } } });
  const pending = bridge.request(open());
  const closing = bridge.dispose();
  const rejected = assert.rejects(closing, /not acknowledged/);
  gate.resolve(opened());
  assert.equal((await pending).result.type, 'error');
  await rejected;
  await bridge.dispose();
  assert.equal(stopAttempts, 2);
});


test('a live or unknown open identity stays reserved after response history rollover', async () => {
  for (const outcome of ['opened', 'unknown'] as const) {
    let dispatches = 0;
    const bridge = new NativeMediaDevices({ owner, audio: { executeMediaDevice: async (request) => {
      dispatches++;
      if (request.request.type === 'open') return outcome === 'opened' ? opened(request.request_id)
        : { request_id: request.request_id, result: { type: 'error', error: { kind: 'native_failure', message: 'unknown' } } };
      if (request.request.type === 'stop') return { request_id: request.request_id, result: { type: 'stopped' } };
      return { request_id: request.request_id, result: { type: 'input', frame: null } };
    } } });
    await bridge.request(open());
    // For an unknown open, use separately rejected owner requests to roll history.
    if (outcome === 'opened') {
      for (let i = 0; i < 257; i++) await bridge.request({ request_id: `read-${i}`, request: { type: 'next_input', lease_id: 'lease-a' } });
    } else {
      // Exercise completed history through successful independent opens/stops.
      for (let i = 0; i < 257; i++) {
        const distinct = open(`other-${i}`);
        await bridge.request(distinct);
      }
    }
    const count = dispatches;
    const stale = open();
    if (stale.request.type === 'open') stale.request.request.owner = { type: 'session', session_id: 'foreign' };
    assert.equal((await bridge.request(stale)).result.type, 'error');
    assert.equal(dispatches, count);
  }
});
