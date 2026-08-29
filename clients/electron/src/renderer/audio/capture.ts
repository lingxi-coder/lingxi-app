/**
 * Microphone capture for the desktop renderer.
 *
 * This is the client side of the engine's `VoiceRecorder` trait
 * (`lingxi-code/traits/src/voice.rs`), proxied over the wire by
 * `lingxi-code/apps/bridge-server/src/audio_bridge.rs`'s `AudioBridge`: the
 * engine has no microphone of its own on desktop, so it asks the connected
 * client (`AudioOpDto::StartRecording` / `StopRecording`) and this module is
 * what actually talks to the OS microphone in response. Wiring this into the
 * bridge's `audio_request`/`audio_response` round trip is a later task; this
 * module only has to produce the bytes and the honest mime type.
 *
 * Two real-world hazards drive this file's shape:
 *
 * 1. **`MediaRecorder`'s supported container varies by platform/build.**
 *    There is no single mime type guaranteed to work everywhere, so this
 *    module never hardcodes one — it asks `MediaRecorder.isTypeSupported`
 *    what is actually usable (see `pickSupportedMimeType`) and reports back
 *    whichever one it really used. The engine puts that string verbatim into
 *    `VoiceRecording.mime_type`; a wrong value there is a lie that travels
 *    all the way to whatever decodes the bytes.
 * 2. **A `MediaStream` that is never released leaves the OS recording
 *    indicator lit** after the user believes recording has stopped — a
 *    privacy-visible bug, not just a resource leak. Every path that acquired
 *    the microphone — `stop()` succeeding, `stop()`'s recorder reporting an
 *    error, and `start()` itself failing after `getUserMedia` already
 *    granted access — releases it. See `releaseStream` and its call sites.
 *
 * `getUserMedia` rejects with different `DOMException` names for different
 * real conditions, and those differences matter to the caller: the wire's
 * `AudioErrorKindDto` has `PermissionDenied` and `Unavailable` as SEPARATE
 * kinds precisely so "the user said no" and "there is no microphone" stay
 * distinguishable end to end (`voice_error` in `audio_bridge.rs` maps them
 * to `VoiceError::PermissionDenied` / a `VoiceError::Other` naming
 * unavailability). `classifyGetUserMediaError` mirrors that split locally
 * with `MicrophoneErrorKind` rather than importing the wire `AudioErrorKindDto`
 * type — this module has no bridge wiring yet, and coupling it to that wire
 * shape before there is a caller to feed it would be the same premature
 * coupling `capabilities.ts`'s `ProviderConfiguredFact` doc comment already
 * argues against. The two kinds it does distinguish use the WIRE's own
 * string spelling (`'permission_denied'`, `'unavailable'`) so a future
 * caller's translation to `AudioErrorKindDto` is a checked literal match,
 * not a fresh vocabulary to invent.
 *
 * Every browser entry point is injected (`MicrophoneCaptureDeps`) so every
 * branch — permission granted/denied/no-device, which mime type actually
 * gets picked, a mid-recording device error, release-on-every-path — can be
 * driven from `voice-capture.test.ts` with no real browser involved, the
 * same discipline `capabilities.ts` uses for its `ProbeDeps`.
 * `browserMicrophoneCaptureDeps` at the bottom wires the injected shape to
 * the real globals for production use.
 */

import { bytesToBase64 } from '../../shared/imageInput.js';

/**
 * Structural subset of `MediaStreamTrack` this module needs — just enough to
 * release the microphone. Mirrors `SpeechSynthesisLike` in `capabilities.ts`.
 */
export interface MediaStreamTrackLike {
  stop(): void;
}

/** Structural subset of `MediaStream` this module needs. */
export interface MediaStreamLike {
  getTracks(): MediaStreamTrackLike[];
}

/**
 * Structural subset of `MediaRecorder` this module needs. `mimeType` is
 * read-only because it reports whatever the recorder actually settled on —
 * this module never invents its own record of "the mime type in use"
 * separately from what the recorder itself says, which is exactly the kind
 * of drift `pickSupportedMimeType` exists to prevent.
 */
export interface MediaRecorderLike {
  readonly mimeType: string;
  start(): void;
  stop(): void;
  ondataavailable: ((event: { data: Blob }) => void) | null;
  onstop: (() => void) | null;
  onerror: ((event: unknown) => void) | null;
}

export interface MicrophoneCaptureDeps {
  getUserMedia(constraints: MediaStreamConstraints): Promise<MediaStreamLike>;
  isTypeSupported(mimeType: string): boolean;
  createRecorder(stream: MediaStreamLike, options: { mimeType: string }): MediaRecorderLike;
}

export interface MicrophoneCaptureOptions {
  sampleRateHz: number;
  /**
   * A hint at the desired container (e.g. `'webm'`), mirroring
   * `AudioOpDto::StartRecording`'s `format` field. Honoured only as far as
   * `MediaRecorder.isTypeSupported` allows — see `pickSupportedMimeType`.
   */
  format: string;
}

export interface CapturedRecording {
  audioBase64: string;
  /** The mime type the recorder actually used — never the requested `format` verbatim. */
  mimeType: string;
}

/**
 * Container candidates, most-preferred first. Every desktop Chromium build
 * supports at least `audio/webm`, but this module still probes rather than
 * assuming — see the module doc's hazard (1).
 */
const CANDIDATE_MIME_TYPES: readonly string[] = [
  'audio/webm;codecs=opus',
  'audio/webm',
  'audio/ogg;codecs=opus',
  'audio/ogg',
  'audio/mp4',
];

/**
 * Picks the mime type `MediaRecorder` will actually be constructed with:
 * candidates matching the requested `format` first (so a caller's hint is
 * honoured when possible), then every other known candidate, in preference
 * order, and only ever a mime type `isTypeSupported` itself reports as
 * usable. Throws — never silently falls back to an unsupported value — when
 * nothing on the candidate list is supported at all.
 */
function pickSupportedMimeType(
  isTypeSupported: (mimeType: string) => boolean,
  requestedFormat: string,
): string {
  const preferred = CANDIDATE_MIME_TYPES.filter((mime) => mime.startsWith(`audio/${requestedFormat}`));
  const ordered = [...preferred, ...CANDIDATE_MIME_TYPES];
  for (const mime of ordered) {
    if (isTypeSupported(mime)) return mime;
  }
  throw new Error(
    `no MediaRecorder mime type is supported on this platform (tried ${CANDIDATE_MIME_TYPES.join(', ')})`,
  );
}

/**
 * The two `getUserMedia` failure classes this module keeps distinguishable,
 * spelled to match `AudioErrorKindDto`'s wire strings — see the module doc.
 * `'other'` covers every real condition that is neither of those (an aborted
 * request, an invalid constraint, a future DOMException name).
 */
export type MicrophoneErrorKind = 'permission_denied' | 'unavailable' | 'other';

export class MicrophoneCaptureError extends Error {
  readonly kind: MicrophoneErrorKind;
  /**
   * The original `DOMException` this was classified from. Set as a plain own
   * property, not via `Error`'s ES2022 `cause` constructor option — this
   * project's `lib` target is ES2020 (`tsconfig.web.json`), which does not
   * declare that overload.
   */
  readonly cause?: unknown;

  constructor(kind: MicrophoneErrorKind, message: string, options?: { cause?: unknown }) {
    super(message);
    this.name = 'MicrophoneCaptureError';
    this.kind = kind;
    this.cause = options?.cause;
  }
}

/**
 * Classifies a `getUserMedia` rejection by its `DOMException.name`. Every
 * name real browsers actually produce is listed explicitly, not swallowed by
 * a bare default, so a name added by a future browser falls to `'other'`
 * deliberately rather than by omission.
 */
function classifyGetUserMediaError(error: unknown): MicrophoneCaptureError {
  const name = error instanceof Error ? error.name : undefined;
  const detail = error instanceof Error ? error.message : String(error);
  switch (name) {
    // The user (or an enclosing permissions policy) said no.
    case 'NotAllowedError':
    case 'SecurityError':
      return new MicrophoneCaptureError(
        'permission_denied',
        `microphone permission denied: ${detail}`,
        { cause: error },
      );
    // No microphone exists, or none satisfies the requested constraints, or
    // one exists but the OS cannot currently read it (e.g. claimed by
    // another application). All three mean "no usable microphone reachable
    // right now" — the same thing `TtsError::Unavailable` /
    // `AudioFailure::NoClient` mean on the synthesis side of this bridge.
    case 'NotFoundError':
    case 'DevicesNotFoundError':
    case 'NotReadableError':
    case 'OverconstrainedError':
      return new MicrophoneCaptureError(
        'unavailable',
        `no microphone is available: ${detail}`,
        { cause: error },
      );
    default:
      return new MicrophoneCaptureError('other', `microphone capture failed: ${detail}`, { cause: error });
  }
}

/** Stops every track so the OS recording indicator turns off. */
function releaseStream(stream: MediaStreamLike): void {
  for (const track of stream.getTracks()) track.stop();
}

/**
 * Records one microphone clip. `start`/`stop` bracket a single capture;
 * `isRecording` reflects only this instance's own state (there is no shared
 * global recording flag — `AudioBridge::is_recording` on the engine side
 * asks the client this exact question over the wire).
 */
export class MicrophoneCapture {
  private readonly deps: MicrophoneCaptureDeps;
  private stream: MediaStreamLike | null = null;
  private recorder: MediaRecorderLike | null = null;
  private chunks: Blob[] = [];

  constructor(deps: MicrophoneCaptureDeps) {
    this.deps = deps;
  }

  isRecording(): boolean {
    return this.recorder !== null;
  }

  async start(opts: MicrophoneCaptureOptions): Promise<void> {
    if (this.isRecording()) {
      throw new Error('already recording; call stop() before starting a new capture');
    }

    let stream: MediaStreamLike;
    try {
      stream = await this.deps.getUserMedia({ audio: { sampleRate: opts.sampleRateHz }, video: false });
    } catch (error) {
      throw classifyGetUserMediaError(error);
    }

    try {
      const mimeType = pickSupportedMimeType(this.deps.isTypeSupported, opts.format);
      const recorder = this.deps.createRecorder(stream, { mimeType });
      const chunks: Blob[] = [];
      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) chunks.push(event.data);
      };
      recorder.start();
      this.stream = stream;
      this.recorder = recorder;
      this.chunks = chunks;
    } catch (error) {
      // The microphone was already granted above (hazard (2) in the module
      // doc): a failure setting up the recorder must not leave it held for a
      // capture that never actually starts.
      releaseStream(stream);
      throw error;
    }
  }

  async stop(): Promise<CapturedRecording> {
    const { recorder, stream } = this;
    if (!recorder || !stream) {
      // Returning an empty clip here would look exactly like a silent
      // microphone rather than a caller bug; this must be a distinct error.
      throw new Error('not recording; call start() before stop()');
    }
    try {
      await new Promise<void>((resolve, reject) => {
        recorder.onstop = () => resolve();
        recorder.onerror = (event) => reject(
          event instanceof Error ? event : new Error(`the microphone recorder reported an error: ${String(event)}`),
        );
        recorder.stop();
      });
      const blob = new Blob(this.chunks, { type: recorder.mimeType });
      const bytes = new Uint8Array(await blob.arrayBuffer());
      return { audioBase64: bytesToBase64(bytes), mimeType: recorder.mimeType };
    } finally {
      // Release on BOTH the success path and the reject-above error path —
      // a `stop()` that throws must not leave the indicator lit either.
      releaseStream(stream);
      this.stream = null;
      this.recorder = null;
      this.chunks = [];
    }
  }
}

/**
 * Builds `MicrophoneCaptureDeps` from the real browser globals, for
 * production use. Wraps the native `MediaRecorder` rather than returning it
 * directly: the real DOM event types (`BlobEvent`, `MediaRecorderErrorEvent`)
 * carry far more than `MediaRecorderLike`'s narrow structural surface needs,
 * so this glue narrows them explicitly instead of leaning on TypeScript to
 * reconcile two differently-shaped event-handler properties.
 */
export function browserMicrophoneCaptureDeps(): MicrophoneCaptureDeps {
  return {
    getUserMedia: (constraints) => navigator.mediaDevices.getUserMedia(constraints),
    isTypeSupported: (mimeType) => MediaRecorder.isTypeSupported(mimeType),
    createRecorder: (stream, options) => {
      const native = new MediaRecorder(stream as MediaStream, options);
      const wrapper: MediaRecorderLike = {
        get mimeType() { return native.mimeType; },
        start: () => native.start(),
        stop: () => native.stop(),
        ondataavailable: null,
        onstop: null,
        onerror: null,
      };
      native.ondataavailable = (event) => wrapper.ondataavailable?.({ data: event.data });
      native.onstop = () => wrapper.onstop?.();
      native.onerror = (event) => wrapper.onerror?.(event);
      return wrapper;
    },
  };
}
