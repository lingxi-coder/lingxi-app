import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  MicrophoneCapture,
  MicrophoneCaptureError,
  type MediaRecorderLike,
  type MediaStreamLike,
  type MicrophoneCaptureDeps,
} from '../src/renderer/audio/capture';
import { synthesize, type SynthesisDeps } from '../src/renderer/audio/synthesis';

// ---------------------------------------------------------------------------
// MicrophoneCapture fixtures
// ---------------------------------------------------------------------------

interface FakeMediaOptions {
  /** Reject `getUserMedia` as if the user (or a permissions policy) said no. */
  denyPermission?: boolean;
  /** The `DOMException.name` the denial carries; defaults to `'NotAllowedError'`. */
  denyErrorName?: string;
  /** Base64 of the bytes the recorder "captures" and reports via `ondataavailable`. */
  chunk?: string;
  /** Marks exactly this one mime type as supported. Ignored if `supportedMimeTypes` is given. */
  mimeType?: string;
  /** Full override of the mime types `isTypeSupported` accepts. `[]` means nothing is supported. */
  supportedMimeTypes?: string[];
  /** Makes `recorder.stop()` report an error instead of finishing normally. */
  failStop?: boolean;
}

interface FakeMicrophoneCaptureDeps extends MicrophoneCaptureDeps {
  tracksStoppedCount(): number;
  requestedMimeTypes(): string[];
}

function fakeMediaDeps(opts: FakeMediaOptions = {}): FakeMicrophoneCaptureDeps {
  const supported = new Set(opts.supportedMimeTypes ?? [opts.mimeType ?? 'audio/webm']);
  let tracksStopped = 0;
  const requested: string[] = [];
  const track = { stop: () => { tracksStopped += 1; } };
  const stream: MediaStreamLike = { getTracks: () => [track] };

  return {
    async getUserMedia() {
      if (opts.denyPermission) {
        throw new DOMException('denied by test fixture', opts.denyErrorName ?? 'NotAllowedError');
      }
      return stream;
    },
    isTypeSupported: (candidate) => {
      requested.push(candidate);
      return supported.has(candidate);
    },
    createRecorder: (_stream, options) => {
      const recorder: MediaRecorderLike = {
        mimeType: options.mimeType,
        ondataavailable: null,
        onstop: null,
        onerror: null,
        start() { /* recording starts immediately in this fixture */ },
        stop() {
          if (opts.failStop) {
            recorder.onerror?.(new Error('fixture-forced recorder failure'));
            return;
          }
          if (opts.chunk !== undefined) {
            const bytes = Uint8Array.from(atob(opts.chunk), (c) => c.charCodeAt(0));
            recorder.ondataavailable?.({ data: new Blob([bytes]) });
          }
          recorder.onstop?.();
        },
      };
      return recorder;
    },
    tracksStoppedCount: () => tracksStopped,
    requestedMimeTypes: () => requested,
  };
}

// ---------------------------------------------------------------------------
// Given by the plan (task-7-brief.md Step 1)
// ---------------------------------------------------------------------------

test('stop before start is an error, not an empty recording', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps());
  await assert.rejects(
    () => capture.stop(),
    /not recording/i,
    'returning an empty clip would look like a silent microphone',
  );
});

test('start then stop returns the captured bytes and the real mime type', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ chunk: 'AAEC', mimeType: 'audio/webm' }));
  await capture.start({ sampleRateHz: 16000, format: 'webm' });
  assert.equal(capture.isRecording(), true);
  const result = await capture.stop();
  assert.equal(capture.isRecording(), false);
  assert.equal(result.mimeType, 'audio/webm');
  assert.ok(result.audioBase64.length > 0);
});

test('a denied microphone surfaces the permission error verbatim', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ denyPermission: true }));
  await assert.rejects(() => capture.start({ sampleRateHz: 16000, format: 'webm' }), /permission/i);
});

// ---------------------------------------------------------------------------
// Mime selection: probed, never hardcoded
// ---------------------------------------------------------------------------

test('mime selection follows what isTypeSupported actually reports, not the requested format', async () => {
  // Nothing "webm" is supported at all; only an ogg variant is. A hardcoded
  // 'audio/webm' implementation would either throw or silently lie here.
  const fake = fakeMediaDeps({ supportedMimeTypes: ['audio/ogg;codecs=opus'], chunk: 'AAEC' });
  const capture = new MicrophoneCapture(fake);
  await capture.start({ sampleRateHz: 16000, format: 'webm' });
  const result = await capture.stop();
  assert.equal(result.mimeType, 'audio/ogg;codecs=opus');
});

test('a platform with no supported mime type at all fails loudly and releases the microphone', async () => {
  const fake = fakeMediaDeps({ supportedMimeTypes: [] });
  const capture = new MicrophoneCapture(fake);
  await assert.rejects(() => capture.start({ sampleRateHz: 16000, format: 'webm' }), /no MediaRecorder mime type is supported/);
  assert.equal(capture.isRecording(), false);
  assert.equal(
    fake.tracksStoppedCount(), 1,
    'getUserMedia already granted the microphone before mime negotiation failed; it must still be released',
  );
});

// ---------------------------------------------------------------------------
// getUserMedia error kinds: distinguishable, not collapsed
// ---------------------------------------------------------------------------

test('a missing microphone is reported as unavailable, distinct from a permission denial', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ denyPermission: true, denyErrorName: 'NotFoundError' }));
  const error = await capture.start({ sampleRateHz: 16000, format: 'webm' }).then(
    () => { throw new Error('expected start() to reject'); },
    (e: unknown) => e,
  );
  assert.ok(error instanceof MicrophoneCaptureError);
  assert.equal(error.kind, 'unavailable');
  assert.ok(
    !/permission/i.test(error.message),
    `a missing-device error must not read like a permission denial, got: ${error.message}`,
  );
});

test('a permission denial is classified as permission_denied, distinct from unavailable', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ denyPermission: true, denyErrorName: 'NotAllowedError' }));
  const error = await capture.start({ sampleRateHz: 16000, format: 'webm' }).then(
    () => { throw new Error('expected start() to reject'); },
    (e: unknown) => e,
  );
  assert.ok(error instanceof MicrophoneCaptureError);
  assert.equal(error.kind, 'permission_denied');
});

// ---------------------------------------------------------------------------
// The microphone is released on every path, not just the happy one
// ---------------------------------------------------------------------------

test('the microphone is released after a normal stop', async () => {
  const fake = fakeMediaDeps({ chunk: 'AAEC' });
  const capture = new MicrophoneCapture(fake);
  await capture.start({ sampleRateHz: 16000, format: 'webm' });
  assert.equal(fake.tracksStoppedCount(), 0, 'must not be released while still recording');
  await capture.stop();
  assert.equal(fake.tracksStoppedCount(), 1);
});

test('the microphone is released even when the recorder errors while stopping', async () => {
  const fake = fakeMediaDeps({ failStop: true });
  const capture = new MicrophoneCapture(fake);
  await capture.start({ sampleRateHz: 16000, format: 'webm' });
  await assert.rejects(() => capture.stop());
  assert.equal(capture.isRecording(), false);
  assert.equal(
    fake.tracksStoppedCount(), 1,
    'a stop-time recorder error must not leave the OS recording indicator lit',
  );
});

// ---------------------------------------------------------------------------
// synthesize()
// ---------------------------------------------------------------------------

/** A minimal, real `SpeechSynthesisVoice`-shaped object — no `as never`. Mirrors `voice-capabilities-probe.test.ts`'s helper of the same name. */
function voice(overrides: Partial<SpeechSynthesisVoice> & { name: string }): SpeechSynthesisVoice {
  return {
    default: false,
    localService: true,
    lang: 'en-US',
    voiceURI: overrides.name,
    ...overrides,
  } as SpeechSynthesisVoice;
}

interface SpeakCall { text: string; voice: SpeechSynthesisVoice | null; rate: number }

function fakeSynthesisDeps(opts: { voices?: SpeechSynthesisVoice[]; failWith?: string } = {}): SynthesisDeps & { speakCalls: SpeakCall[] } {
  const speakCalls: SpeakCall[] = [];
  const voices = opts.voices ?? [];
  return {
    synth: {
      getVoices: () => voices,
      addEventListener: () => {},
      removeEventListener: () => {},
    },
    voiceListTimeoutMs: 5,
    async speak(text, spokenVoice, rate) {
      speakCalls.push({ text, voice: spokenVoice, rate });
      if (opts.failWith) throw new Error(opts.failWith);
    },
    speakCalls,
  };
}

test('synthesize resolves the system:<name> voice, speaks at the given rate, and reports the played-in-place convention', async () => {
  const alex = voice({ name: 'Alex' });
  const deps = fakeSynthesisDeps({ voices: [alex] });
  const result = await synthesize('hello', 'system:Alex', 1.25, deps);
  assert.deepEqual(
    result, { pcmBase64: '', sampleRateHz: 0 },
    'speechSynthesis plays in place and exposes no samples — empty PCM here is success, not failure',
  );
  assert.equal(deps.speakCalls.length, 1);
  assert.equal(deps.speakCalls[0].text, 'hello');
  assert.equal(deps.speakCalls[0].voice, alex);
  assert.equal(deps.speakCalls[0].rate, 1.25);
});

test('an unmatched voice id still plays, using the platform default rather than throwing', async () => {
  const deps = fakeSynthesisDeps({ voices: [voice({ name: 'Alex' })] });
  const result = await synthesize('hi', 'system:default', 1.0, deps);
  assert.equal(deps.speakCalls[0].voice, null);
  assert.deepEqual(result, { pcmBase64: '', sampleRateHz: 0 });
});

test('a synthesis failure rejects rather than reporting a false played-in-place success', async () => {
  const deps = fakeSynthesisDeps({ voices: [], failWith: 'audio-hardware' });
  await assert.rejects(() => synthesize('hi', 'system:default', 1.0, deps), /audio-hardware/);
});
