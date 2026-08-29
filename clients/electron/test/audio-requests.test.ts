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
import { MAX_AUDIO_BASE64_LENGTH } from '../src/shared/audioResponse';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

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

test('a diagnostics callback that throws cannot strand the engine either', async () => {
  // `useBridge`'s own `capture` helper sets the global error AND RETHROWS —
  // that rethrow is pinned by `bridge-error-reaches-callers.test.ts`, because
  // every settings page depends on it. It is therefore the obvious thing for
  // a future caller to hand to `onError`, and if a throw from there escaped,
  // the response would never be sent and the engine would sit on its
  // deadline.
  const sent: AudioResponseCommand[] = [];
  const rethrow = (cause: unknown) => { throw cause; };

  await assert.doesNotReject(() => handleAudioRequestEvent(
    'session-1',
    audioRequest(14, { type: 'is_recording' }),
    () => { throw new Error('no microphone bindings'); },
    async (_sessionId, command) => { sent.push(command); },
    rethrow,
  ));
  assert.equal(sent.length, 1, 'the response must survive a throwing diagnostics callback');

  // The same hazard on the send path: there the response is already gone, so
  // only the crash matters.
  await assert.doesNotReject(() => handleAudioRequestEvent(
    'session-1',
    audioRequest(15, { type: 'is_recording' }),
    () => harness().deps,
    async () => { throw new Error('the bridge is gone'); },
    rethrow,
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
// An oversize clip must FAIL, not stall.
//
// The user chooses how long to hold the microphone, so a clip larger than the
// wire bound is reachable by ordinary use. If it were sent anyway, the command
// gate would reject it, nothing would reach the engine, and `stop_recording`
// would park for its full 30-second `DEVICE_CONTROL_DEADLINE` before failing
// with no explanation — the exact outcome the rest of this module exists to
// prevent. Detecting it here turns a silent 30s stall into an immediate,
// named failure.
// ---------------------------------------------------------------------------

/** A base64 string that genuinely crosses `MAX_AUDIO_BASE64_LENGTH`. */
const OVERSIZE_BASE64 = 'A'.repeat(MAX_AUDIO_BASE64_LENGTH + 4);

test('a clip too large for the wire is reported as a failure, not sent and stalled', async () => {
  assert.ok(OVERSIZE_BASE64.length > MAX_AUDIO_BASE64_LENGTH, 'the fixture must actually cross the bound it names');

  const h = harness();
  h.recorder.recording = true;
  h.recorder.clip = { audioBase64: OVERSIZE_BASE64, mimeType: 'audio/webm;codecs=opus' };

  const result = await serviceAudioOp({ type: 'stop_recording' }, h.deps);

  assert.equal(result.type, 'failed', 'an oversize clip must be reported, never handed to a gate that will drop it');
  assert.equal(result.type === 'failed' ? result.kind : null, 'other');
  assert.match(
    result.type === 'failed' ? result.message : '',
    new RegExp(String(MAX_AUDIO_BASE64_LENGTH)),
    'the message must name the limit the clip crossed, so the failure is actionable',
  );

  // And the failure itself must survive the gate — otherwise the fix would
  // have traded one stall for another.
  assert.doesNotThrow(() => validateClientCommand({ type: 'audio_response', request_id: 8, result }));
});

test('a clip just inside the bound is still sent', async () => {
  // The A/B for the test above: the check must reject only what actually
  // exceeds the bound, not shrink the usable recording length.
  const h = harness();
  h.recorder.recording = true;
  h.recorder.clip = { audioBase64: 'A'.repeat(MAX_AUDIO_BASE64_LENGTH), mimeType: 'audio/webm' };

  const result = await serviceAudioOp({ type: 'stop_recording' }, h.deps);

  assert.equal(result.type, 'recording', 'a clip exactly at the bound is legal and must be delivered');
});

test('the audio payload bounds have exactly one declaration', () => {
  // The renderer must stay inside the same numbers `main/validation.ts`
  // enforces. Two copies would drift, and the symptom of the drift is the
  // stall above — the hardest possible thing to attribute back to a constant.
  const validation = readFileSync(join(import.meta.dirname, '../src/main/validation.ts'), 'utf8');
  const requests = readFileSync(join(import.meta.dirname, '../src/renderer/audio/requests.ts'), 'utf8');
  for (const [name, source] of [['validation.ts', validation], ['requests.ts', requests]] as const) {
    assert.ok(
      /from '\.\.\/shared\/audioResponse\.js'|from '\.\.\/\.\.\/shared\/audioResponse\.js'/.test(source),
      `${name} must import the audio bounds from shared/audioResponse.ts rather than restating them`,
    );
    assert.ok(
      !/(?:const|let)\s+MAX_AUDIO_BASE64_LENGTH\s*=/.test(source),
      `${name} must not declare its own MAX_AUDIO_BASE64_LENGTH`,
    );
  }
  // Positive control for that second regex.
  assert.ok(/(?:const|let)\s+MAX_AUDIO_BASE64_LENGTH\s*=/.test('const MAX_AUDIO_BASE64_LENGTH = 1;'));
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
    {
      // Same hazard, reachable by simply recording for a while.
      name: 'an oversize clip',
      op: { type: 'stop_recording' },
      arrange: (h) => {
        h.recorder.recording = true;
        h.recorder.clip = { audioBase64: OVERSIZE_BASE64, mimeType: 'audio/webm' };
      },
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

// ---------------------------------------------------------------------------
// The wiring itself.
//
// Everything above tests a function nobody has to call. Tasks 1-7 shipped a
// capture module and a synthesis module in exactly that state — complete,
// green, and unreachable, with the only mentions of `audio_request` under
// `src/` being two doc comments deferring the wiring to "a later task". So
// the call site is asserted here too.
//
// It is asserted structurally rather than by running the hook because
// `useBridge`'s subscription lives in a `useEffect`, and the only renderer
// this test setup has is `react-dom/server`'s `renderToString`, which runs
// the component body and deliberately skips effects (see the note in
// `bridge-error-reaches-callers.test.ts`). Each check below is paired with a
// positive control, because a structural check that silently matches nothing
// is worth less than no check at all.
// ---------------------------------------------------------------------------

/** The body of the `host.onEvent(...)` subscription in `useBridge.ts`. */
function onEventHandlerSource(): string {
  const source = readFileSync(join(import.meta.dirname, '../src/renderer/bridge/useBridge.ts'), 'utf8');
  const start = source.indexOf('host.onEvent(');
  assert.notEqual(start, -1, 'useBridge no longer subscribes to engine events at all');
  let depth = 0;
  for (let index = source.indexOf('(', start); index < source.length; index += 1) {
    if (source[index] === '(') depth += 1;
    else if (source[index] === ')') {
      depth -= 1;
      if (depth === 0) return source.slice(start, index + 1);
    }
  }
  assert.fail('the host.onEvent(...) call is unbalanced');
}

test('useBridge answers audio requests from inside its engine-event subscription', () => {
  const handler = onEventHandlerSource();

  // Positive control: the extraction really did capture the handler body.
  assert.ok(
    handler.includes('reduceEvent(') && handler.includes("event.type === 'error'"),
    'the extracted source is not the event handler, so every assertion below would prove nothing',
  );

  assert.ok(
    handler.includes('handleAudioRequestEvent('),
    'nothing services ClientEvent::AudioRequest — the engine would park every microphone '
    + 'and speech call until its deadline expired',
  );

  // The branch is matched in full, not just searched for the wire name: a
  // `handleAudioRequestEvent` sitting behind an extra condition (`if (false
  // && …)`, a feature flag defaulting off) is unreachable in exactly the way
  // this test exists to catch, and a mere `includes` cannot tell the two
  // apart — measured, by mutating the branch to `if (false && …)` and
  // watching the looser check stay green.
  const branch = /if \(event\.type === 'audio_request'\) \{/;
  assert.match(
    handler,
    branch,
    'the audio_request branch must be reached unconditionally; anything guarding it further '
    + 'silently un-wires every microphone and speech call',
  );
  assert.ok(
    !branch.test("if (false && event.type === 'audio_request') {"),
    'if this regex accepted a disabled branch, the assertion above would prove nothing',
  );
});

test('the audio dispatch is not handed useBridge\'s rethrowing capture helper', () => {
  // `capture` sets the global error AND RETHROWS. Passed as the reporter, a
  // failure to build the microphone bindings would throw before the response
  // was sent, stranding the parked engine call — the one hazard this whole
  // module exists to avoid.
  const handler = onEventHandlerSource();
  const start = handler.indexOf('handleAudioRequestEvent(');
  assert.notEqual(start, -1);
  let depth = 0;
  let end = start;
  for (let index = handler.indexOf('(', start); index < handler.length; index += 1) {
    if (handler[index] === '(') depth += 1;
    else if (handler[index] === ')') {
      depth -= 1;
      if (depth === 0) { end = index + 1; break; }
    }
  }
  const call = handler.slice(start, end);
  assert.ok(call.length > 'handleAudioRequestEvent()'.length, 'failed to slice the call arguments');
  assert.ok(!/(^|[\s,(])capture([\s,)])/.test(call), `capture must not be the audio reporter: ${call}`);
  // Positive control for that regex: it does find a bare argument.
  assert.ok(/(^|[\s,(])capture([\s,)])/.test('handleAudioRequestEvent(sessionId, event, deps, send, capture)'));
});

test('the production microphone and speech bindings are the ones actually used', () => {
  // `AudioRequestDeps` is an interface; importing only the type would type-check
  // perfectly while the hook fed it nothing real.
  const source = readFileSync(join(import.meta.dirname, '../src/renderer/bridge/useBridge.ts'), 'utf8');
  for (const binding of ['browserMicrophoneCaptureDeps', 'browserSynthesisDeps', 'new MicrophoneCapture(']) {
    assert.ok(source.includes(binding), `useBridge must build its audio deps with ${binding}`);
  }

  // The recorder must be RETAINED, not rebuilt per request: a capture spans a
  // `start_recording`/`stop_recording` pair of separate engine requests, so a
  // fresh `MicrophoneCapture` each time would answer `not_recording` to every
  // stop and never release the microphone (leaving the OS recording indicator
  // lit — `capture.ts`'s hazard 2). Structurally, that means the construction
  // sits behind the once-only guard rather than in the request path.
  const guard = source.indexOf('if (!audioBindings.current)');
  const construction = source.indexOf('new MicrophoneCapture(');
  assert.notEqual(guard, -1, 'the audio bindings are no longer built once and cached');
  assert.equal(source.indexOf('new MicrophoneCapture(', construction + 1), -1, 'the recorder is constructed in more than one place');
  assert.ok(guard < construction, 'the recorder must be constructed inside the once-only guard, not per request');
});
