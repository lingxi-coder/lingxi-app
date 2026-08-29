import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { AudioOpDto, AudioResultDto, ClientEvent } from '@lingxi/bridge-client';

import {
  DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE,
  handleAudioRequestEvent,
  serviceAudioOp,
  type AudioRequestDeps,
  type AudioResponseCommand,
} from '../src/renderer/audio/requests';
import { MicrophoneCaptureError, type CapturedRecording, type MicrophoneCaptureOptions } from '../src/renderer/audio/capture';
import { validateClientCommand } from '../src/main/validation';

/**
 * A recorder stand-in with the same surface `MicrophoneCapture` exposes, so
 * a test can drive the branches a real microphone cannot be made to take on
 * demand (permission refused, no device, a mid-capture recorder error).
 */
class FakeRecorder {
  recording = false;
  startError: unknown = null;
  stopError: unknown = null;
  clip: CapturedRecording = { audioBase64: 'AAEC', mimeType: 'audio/webm;codecs=opus' };
  readonly startCalls: MicrophoneCaptureOptions[] = [];
  stopCalls = 0;

  isRecording(): boolean { return this.recording; }

  async start(opts: MicrophoneCaptureOptions): Promise<void> {
    this.startCalls.push(opts);
    if (this.startError) throw this.startError;
    this.recording = true;
  }

  async stop(): Promise<CapturedRecording> {
    this.stopCalls += 1;
    if (this.stopError) { this.recording = false; throw this.stopError; }
    this.recording = false;
    return this.clip;
  }
}

interface Harness {
  deps: AudioRequestDeps;
  recorder: FakeRecorder;
  spoken: { text: string; voiceId: string; rate: number }[];
  synthesisError: { value: unknown };
}

function harness(overrides: Partial<{ voiceSelection: string; rate: number }> = {}): Harness {
  const recorder = new FakeRecorder();
  const spoken: { text: string; voiceId: string; rate: number }[] = [];
  const synthesisError = { value: null as unknown };
  return {
    recorder,
    spoken,
    synthesisError,
    deps: {
      recorder,
      playback: () => ({
        voiceSelection: overrides.voiceSelection ?? 'system:Samantha',
        rate: overrides.rate ?? 1.25,
      }),
      synthesize: async (text, voiceId, rate) => {
        spoken.push({ text, voiceId, rate });
        if (synthesisError.value) throw synthesisError.value;
        return { pcmBase64: '', sampleRateHz: 0 };
      },
    },
  };
}

/** Every op the wire declares, in the shape the engine actually sends it. */
const EVERY_OP: AudioOpDto[] = [
  { type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' },
  { type: 'stop_recording' },
  { type: 'is_recording' },
  { type: 'transcribe', language: 'en-US' },
  { type: 'synthesize', text: 'hello', voice: 'system:Alex' },
];

function audioRequest(requestId: number, op: AudioOpDto): ClientEvent {
  return { type: 'audio_request', request_id: requestId, op };
}

// ---------------------------------------------------------------------------
// Exactly one response, always.
//
// `audio_bridge.rs` parks the engine call on a deadline — 5s
// (`STATE_QUERY_DEADLINE`), 30s (`DEVICE_CONTROL_DEADLINE`), 180s
// (`CAPTURE_DEADLINE`). An unanswered request is not "nothing happened": it
// is the engine blocked for that long and then failing. So the count of
// responses, not just their content, is the property under test.
// ---------------------------------------------------------------------------

test('every op the engine can send produces exactly one response', async () => {
  for (const op of EVERY_OP) {
    const sent: AudioResponseCommand[] = [];
    const { deps, recorder } = harness();
    // `stop_recording` is only meaningful mid-capture; give it one so this
    // measures the success path rather than the not-recording path.
    if (op.type === 'stop_recording') recorder.recording = true;

    await handleAudioRequestEvent('session-1', audioRequest(11, op), () => deps, async (_sessionId, command) => {
      sent.push(command);
    });

    assert.equal(sent.length, 1, `${op.type} must answer exactly once, not ${sent.length} times`);
    assert.equal(sent[0]?.type, 'audio_response');
    assert.equal(sent[0]?.request_id, 11, 'the answer must carry the request_id it is correlated by');
  }
});

test('every op still produces exactly one response when the operation fails', async () => {
  const failures: { op: AudioOpDto; arrange: (h: Harness) => void }[] = [
    {
      op: { type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' },
      arrange: (h) => { h.recorder.startError = new MicrophoneCaptureError('permission_denied', 'microphone permission denied: no'); },
    },
    {
      op: { type: 'stop_recording' },
      arrange: (h) => { h.recorder.recording = true; h.recorder.stopError = new Error('the microphone recorder reported an error'); },
    },
    {
      op: { type: 'is_recording' },
      arrange: (h) => { h.recorder.isRecording = () => { throw new Error('the recorder is wedged'); }; },
    },
    {
      op: { type: 'synthesize', text: 'hello' },
      arrange: (h) => { h.synthesisError.value = new Error('speech synthesis failed: interrupted'); },
    },
  ];

  for (const { op, arrange } of failures) {
    const sent: AudioResponseCommand[] = [];
    const h = harness();
    arrange(h);

    await handleAudioRequestEvent('session-1', audioRequest(12, op), () => h.deps, async (_sessionId, command) => {
      sent.push(command);
    });

    assert.equal(sent.length, 1, `a failing ${op.type} must still answer exactly once`);
    assert.equal(sent[0]?.result.type, 'failed', `a failing ${op.type} must report a failure, not a success`);
  }
});

test('a deps factory that throws still answers, rather than stranding the engine', async () => {
  const sent: AudioResponseCommand[] = [];
  const seen: unknown[] = [];

  await handleAudioRequestEvent(
    'session-1',
    audioRequest(13, { type: 'is_recording' }),
    () => { throw new Error('the renderer has no microphone bindings'); },
    async (_sessionId, command) => { sent.push(command); },
    (cause) => seen.push(cause),
  );

  assert.equal(sent.length, 1, 'a throw while building the deps must not swallow the response');
  assert.equal(sent[0]?.result.type, 'failed');
  assert.equal(seen.length, 1, 'the underlying cause is still surfaced to the caller');
});

test('events that are not audio requests are ignored without answering anything', async () => {
  const sent: AudioResponseCommand[] = [];
  const { deps } = harness();

  await handleAudioRequestEvent('session-1', { type: 'turn_started', turn_id: 1 } as ClientEvent, () => deps, async (_s, c) => { sent.push(c); });
  await handleAudioRequestEvent('session-1', { type: 'text_delta', text: 'hi' }, () => deps, async (_s, c) => { sent.push(c); });

  assert.deepEqual(sent, [], 'only audio_request may produce an audio_response');
});

test('a response the engine drops is inert, not fatal', async () => {
  // The engine drops a response whose `request_id` it no longer knows — a
  // late answer, or one for a request a restart already discarded. That
  // rejection reaches the renderer as a rejected `host.command(...)`
  // promise. An unhandled rejection here takes the whole UI down.
  const seen: unknown[] = [];
  const { deps } = harness();

  await assert.doesNotReject(
    () => handleAudioRequestEvent(
      'session-1',
      audioRequest(999, { type: 'is_recording' }),
      () => deps,
      async () => { throw new Error('unknown audio request id 999'); },
      (cause) => seen.push(cause),
    ),
    'a rejected send must be reported, never rethrown into the event listener',
  );
  assert.equal(seen.length, 1);
  assert.match(String((seen[0] as Error).message), /unknown audio request id 999/);

  // A synchronous throw from the sender is the same hazard by another route.
  await assert.doesNotReject(() => handleAudioRequestEvent(
    'session-1',
    audioRequest(999, { type: 'is_recording' }),
    () => deps,
    () => { throw new Error('the bridge is gone'); },
  ));
});

// ---------------------------------------------------------------------------
// Honest answers.
// ---------------------------------------------------------------------------

test('transcribe reports that desktop speech recognition is unavailable, and never invents a transcript', async () => {
  for (const op of [{ type: 'transcribe' } as const, { type: 'transcribe', language: 'zh-CN' } as const]) {
    const { deps } = harness();
    const result = await serviceAudioOp(op, deps);

    assert.equal(result.type, 'failed', 'a transcribe answer must be a failure, not a success with empty text');
    assert.notEqual(result.type as string, 'transcript', 'the renderer must never fabricate a transcript');
    assert.equal(
      result.type === 'failed' ? result.kind : null,
      'unavailable',
      'the kind must round-trip to SttError::Unavailable, the same degradation a phone with no recognizer gets',
    );
    assert.equal(result.type === 'failed' ? result.message : null, DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE);
    assert.match(DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE, /not available in this build/);
  }
});

test('a successful synthesis reports the empty-PCM played-in-place pair, not a failure', async () => {
  const h = harness({ voiceSelection: 'system:Samantha', rate: 1.25 });

  const result = await serviceAudioOp({ type: 'synthesize', text: 'hello', voice: 'system:Alex' }, h.deps);

  assert.deepEqual(result, { type: 'audio', pcm_base64: '', sample_rate_hz: 0 });
  assert.deepEqual(h.spoken, [{ text: 'hello', voiceId: 'system:Alex', rate: 1.25 }],
    'the op names the voice; the rate comes from the persisted preference');
});

test('a synthesize op with no voice falls back to the persisted voice preference', async () => {
  const h = harness({ voiceSelection: 'system:Samantha', rate: 0.75 });

  await serviceAudioOp({ type: 'synthesize', text: 'hello' }, h.deps);

  assert.deepEqual(h.spoken, [{ text: 'hello', voiceId: 'system:Samantha', rate: 0.75 }]);
});

test('start and stop drive the real recorder and report its own mime type', async () => {
  const h = harness();
  h.recorder.clip = { audioBase64: 'BAUG', mimeType: 'audio/ogg;codecs=opus' };

  const started = await serviceAudioOp({ type: 'start_recording', sample_rate_hz: 24_000, format: 'ogg' }, h.deps);
  assert.deepEqual(started, { type: 'ok' });
  assert.deepEqual(h.recorder.startCalls, [{ sampleRateHz: 24_000, format: 'ogg' }]);

  assert.deepEqual(await serviceAudioOp({ type: 'is_recording' }, h.deps), { type: 'recording_state', recording: true });

  const stopped = await serviceAudioOp({ type: 'stop_recording' }, h.deps);
  assert.deepEqual(stopped, { type: 'recording', audio_base64: 'BAUG', mime_type: 'audio/ogg;codecs=opus' },
    'the mime type must be whatever the recorder actually used, never the requested format');

  assert.deepEqual(await serviceAudioOp({ type: 'is_recording' }, h.deps), { type: 'recording_state', recording: false });
});

// ---------------------------------------------------------------------------
// Failure kinds stay distinguishable.
//
// `AudioErrorKindDto` splits `permission_denied` / `unavailable` /
// `not_recording` on purpose: `audio_bridge.rs`'s `voice_error` maps each to
// a different `VoiceError`, and a round-trip identity test pins that. Every
// one of these collapsing to `other` would undo that deliberately.
// ---------------------------------------------------------------------------

test('a denied microphone is permission_denied, not a generic failure', async () => {
  const h = harness();
  h.recorder.startError = new MicrophoneCaptureError('permission_denied', 'microphone permission denied: NotAllowedError');

  const result = await serviceAudioOp({ type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' }, h.deps);

  assert.equal(result.type, 'failed');
  assert.equal(result.type === 'failed' ? result.kind : null, 'permission_denied');
  assert.match(result.type === 'failed' ? result.message : '', /permission denied/);
});

test('a missing microphone is unavailable, not permission_denied', async () => {
  const h = harness();
  h.recorder.startError = new MicrophoneCaptureError('unavailable', 'no microphone is available: NotFoundError');

  const result = await serviceAudioOp({ type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' }, h.deps);

  assert.equal(result.type === 'failed' ? result.kind : null, 'unavailable');
});

test('stopping when nothing is recording is not_recording, and never an empty clip', async () => {
  const h = harness();
  h.recorder.recording = false;

  const result = await serviceAudioOp({ type: 'stop_recording' }, h.deps);

  assert.equal(result.type, 'failed', 'an empty clip here would look exactly like a silent microphone');
  assert.equal(result.type === 'failed' ? result.kind : null, 'not_recording');
  assert.equal(h.recorder.stopCalls, 0, 'the recorder is never asked to stop a capture that was never started');
});

test('starting while a capture is already running is busy, not a second microphone grab', async () => {
  const h = harness();
  h.recorder.recording = true;

  const result = await serviceAudioOp({ type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' }, h.deps);

  assert.equal(result.type === 'failed' ? result.kind : null, 'busy');
  assert.deepEqual(h.recorder.startCalls, [], 'the live capture must not be disturbed');
});

test('a synthesis failure is synthesis_failed, so it round-trips to TtsError::SynthesisFailed', async () => {
  const h = harness();
  h.synthesisError.value = new Error('speech synthesis failed: interrupted');

  const result = await serviceAudioOp({ type: 'synthesize', text: 'hello' }, h.deps);

  assert.equal(result.type === 'failed' ? result.kind : null, 'synthesis_failed');
});

test('an op this build does not know is reported unavailable, not silently ignored', async () => {
  const { deps } = harness();

  // `AudioOpDto` is `#[non_exhaustive]` on the Rust side: a newer engine can
  // send an op this build has never heard of. Falling through without a
  // response would park that engine call for its whole deadline.
  const result = await serviceAudioOp({ type: 'record_video' } as unknown as AudioOpDto, deps);

  assert.equal(result.type === 'failed' ? result.kind : null, 'unavailable');
  assert.match(result.type === 'failed' ? result.message : '', /record_video/);
});

// ---------------------------------------------------------------------------
// The seam with the command gate.
//
// A response `main/validation.ts` rejects never reaches the engine, which
// then waits out its deadline. So "the renderer produced a result" is not
// enough — every result it can produce has to survive the gate.
// ---------------------------------------------------------------------------

test('every response this module produces survives the runtime command gate', async () => {
  const cases: { name: string; op: AudioOpDto; arrange?: (h: Harness) => void }[] = [
    { name: 'start ok', op: { type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' } },
    { name: 'is_recording', op: { type: 'is_recording' } },
    { name: 'stop success', op: { type: 'stop_recording' }, arrange: (h) => { h.recorder.recording = true; } },
    { name: 'stop when idle', op: { type: 'stop_recording' } },
    { name: 'transcribe', op: { type: 'transcribe', language: 'en-US' } },
    { name: 'synthesize ok', op: { type: 'synthesize', text: 'hello' } },
    {
      name: 'permission denied',
      op: { type: 'start_recording', sample_rate_hz: 16_000, format: 'webm' },
      arrange: (h) => { h.recorder.startError = new MicrophoneCaptureError('permission_denied', 'denied'); },
    },
    {
      // A `DOMException` message is not length-bounded, and the gate caps an
      // audio failure message. An overlong message must be trimmed here, not
      // rejected there.
      name: 'a hostile error message',
      op: { type: 'synthesize', text: 'hello' },
      arrange: (h) => { h.synthesisError.value = new Error('x'.repeat(200_000)); },
    },
    {
      // `new Error('')` has an empty message; the gate rejects empty strings.
      name: 'an unnamed error',
      op: { type: 'synthesize', text: 'hello' },
      arrange: (h) => { h.synthesisError.value = new Error(''); },
    },
    {
      // A recorder that reports no mime type would produce a `recording`
      // answer the gate rejects — that must be a reported failure, not a
      // 30-second stall.
      name: 'a recorder with no mime type',
      op: { type: 'stop_recording' },
      arrange: (h) => { h.recorder.recording = true; h.recorder.clip = { audioBase64: 'AAEC', mimeType: '' }; },
    },
  ];

  for (const { name, op, arrange } of cases) {
    const h = harness();
    arrange?.(h);
    const result: AudioResultDto = await serviceAudioOp(op, h.deps);
    assert.doesNotThrow(
      () => validateClientCommand({ type: 'audio_response', request_id: 5, result }),
      `the "${name}" response must pass the command gate; a rejected response is a parked engine call`,
    );
  }
});

test('the gate check above can actually fail', () => {
  // If `validateClientCommand` accepted anything, every assertion in the
  // test above would prove nothing.
  assert.throws(
    () => validateClientCommand({ type: 'audio_response', request_id: 5, result: { type: 'failed', kind: 'other', message: '' } }),
    /invalid audio error message/,
  );
});
