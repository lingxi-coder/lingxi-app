/**
 * Servicing the engine's `audio_request` events in the desktop renderer.
 *
 * This is the piece that makes everything else under `renderer/audio/`
 * reachable. The engine has no microphone or speaker of its own on desktop:
 * `lingxi-code/apps/bridge-server/src/audio_bridge.rs`'s `AudioBridge`
 * implements the `VoiceRecorder` / `TextToSpeech` / `SpeechToText` traits by
 * emitting `ClientEvent::AudioRequest` and waiting for the connected client
 * to answer with `ClientCommand::AudioResponse`, correlated by `request_id`.
 * `capture.ts` and `synthesis.ts` produce the bytes; this module is what the
 * engine actually talks to.
 *
 * ## Every request must produce exactly one response
 *
 * An unanswered request is NOT a no-op. `AudioBridge::request` parks the
 * engine call on a deadline — 5s for `is_recording` (`STATE_QUERY_DEADLINE`),
 * 30s for start/stop (`DEVICE_CONTROL_DEADLINE`), 180s for transcribe and
 * synthesize (`CAPTURE_DEADLINE`) — so a dropped response is a hang of
 * exactly that length followed by a failure. That is why
 * {@link handleAudioRequestEvent} has exactly ONE `send` call on every path
 * past its early return, why {@link serviceAudioOp} answers rather than
 * throws (including for an op this build has never heard of — `AudioOpDto`
 * is `#[non_exhaustive]`), and why the failure messages built here are
 * trimmed and bounded to what `main/validation.ts`'s gate accepts: a
 * response that gate rejects never reaches the engine either.
 *
 * The converse hazard is a response the engine no longer wants. It drops a
 * response for an unknown or already-answered `request_id` by design, which
 * surfaces here as a rejected `host.command(...)` promise. That is reported,
 * never rethrown — an unhandled rejection inside the event listener would
 * take the UI down over an answer nobody was waiting for.
 *
 * ## Failure kinds are not interchangeable
 *
 * `AudioErrorKindDto` splits `permission_denied`, `unavailable`,
 * `not_recording` and `busy` apart on purpose: `audio_bridge.rs`'s
 * `voice_error` / `stt_error` / `tts_error` map each to a different trait
 * error, and a round-trip identity test on the Rust side pins that mapping.
 * Collapsing them into `other` here would quietly undo that. `capture.ts`
 * already spells its own `MicrophoneErrorKind` with the wire's strings for
 * exactly this hand-off, so {@link failureFrom}'s translation is a checked
 * literal match rather than a fresh vocabulary.
 *
 * ## Transcription is reported unavailable, honestly
 *
 * Desktop has no speech recognizer, and the renderer cannot call a hosted
 * transcription API either: `main/host.ts` forwards a provider credential
 * straight to the engine and keeps nothing, so there is no key in this
 * process to authenticate with (a repo-wide search for any credential
 * read-back in the main process finds none — that is Part A's deliberate
 * design). So `AudioOpDto::Transcribe` is answered `failed` /
 * `unavailable`, which round-trips to `SttError::Unavailable` — exactly the
 * degradation a phone with no recognizer gets. It is NOT answered with an
 * empty transcript, which would read as "the microphone heard nothing", and
 * not with invented text. `main/validation.ts` keeps `transcript` off the
 * command gate so that stays true even if some future code tried otherwise;
 * wiring real transcription means widening the gate deliberately.
 */

import type { AudioErrorKindDto, AudioOpDto, AudioResultDto, ClientEvent } from '@lingxi/bridge-client';

import { MAX_AUDIO_BASE64_LENGTH, MAX_AUDIO_FAILURE_MESSAGE_LENGTH } from '../../shared/audioResponse.js';
import { MicrophoneCaptureError, type CapturedRecording, type MicrophoneCaptureOptions } from './capture.js';
import type { SynthesisResult } from './synthesis.js';

/**
 * What `AudioOpDto::Transcribe` is answered with. Exported so the test that
 * pins the honesty of this answer asserts the real string rather than a
 * copy of it.
 */
export const DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE =
  'desktop speech recognition is not available in this build: the LingXi desktop client has no speech '
  + 'recognizer, and it holds no provider credential to call a hosted transcription API with';

/**
 * The recorder surface this module drives — structurally `MicrophoneCapture`
 * from `capture.ts`. Declared as an interface rather than taking the class
 * so a test can drive branches a real microphone cannot be made to take on
 * demand, the same injection discipline `capture.ts` itself uses.
 */
export interface AudioRecorderLike {
  isRecording(): boolean;
  start(opts: MicrophoneCaptureOptions): Promise<void>;
  stop(): Promise<CapturedRecording>;
}

/**
 * The persisted voice settings a `Synthesize` op needs but does not carry.
 * `AudioOpDto::Synthesize` names a `text` and optionally a `voice`; the rate
 * is a device preference (`shared/voicePreferences.ts`), and the voice falls
 * back to the user's own selection when the engine does not name one.
 */
export interface VoicePlaybackPreference {
  voiceSelection: string;
  rate: number;
}

export interface AudioRequestDeps {
  recorder: AudioRecorderLike;
  /** `synthesis.ts`'s `synthesize`, with its `SynthesisDeps` already bound. */
  synthesize(text: string, voiceId: string, rate: number): Promise<SynthesisResult>;
  playback(): VoicePlaybackPreference;
}

/** The one command shape this module sends. */
export type AudioResponseCommand = { type: 'audio_response'; request_id: number; result: AudioResultDto };

/**
 * Sends one `audio_response` for `sessionId`. In production this is
 * `host.command`; the return value is awaited only so a rejection can be
 * caught rather than escaping as an unhandled rejection.
 */
export type AudioResponseSender = (sessionId: string, command: AudioResponseCommand) => Promise<void> | void;

/**
 * Reports a cause that was turned into a failure, or that the send itself
 * produced. Diagnostics only, and {@link report} invokes it defensively:
 * `useBridge`'s own `capture` helper sets the global error AND RETHROWS
 * (pinned by `bridge-error-reaches-callers.test.ts`, since every settings
 * page depends on that rethrow), which makes it the obvious thing to pass
 * here — and a throw out of it would cost the parked engine call its answer.
 */
export type AudioFailureReporter = (cause: unknown) => void;

/** Invokes a reporter without ever letting it become this function's problem. */
function report(onError: AudioFailureReporter | undefined, cause: unknown): void {
  try {
    onError?.(cause);
  } catch {
    // A diagnostics callback that throws must not strand the engine.
  }
}

/** A non-empty, bounded description of an arbitrary thrown value. */
function describe(cause: unknown): string {
  const raw = cause instanceof Error ? cause.message : String(cause);
  const trimmed = raw.trim();
  // The gate rejects an empty message, and `new Error('')` produces one.
  if (trimmed.length === 0) return 'the desktop client reported an unnamed audio failure';
  return trimmed.length > MAX_AUDIO_FAILURE_MESSAGE_LENGTH
    ? `${trimmed.slice(0, MAX_AUDIO_FAILURE_MESSAGE_LENGTH - 1)}…`
    : trimmed;
}

function failed(kind: AudioErrorKindDto, message: string): AudioResultDto {
  return { type: 'failed', kind, message };
}

/**
 * Rejects a base64 payload the command gate would drop for size.
 *
 * Unlike every other bound here this one is reachable by ordinary use — the
 * USER decides how long to hold the microphone — and the consequence of
 * sending it anyway is not a validation error anybody sees: the gate drops the
 * response, nothing reaches the engine, and `stop_recording` parks for its
 * full 30-second deadline before failing with no explanation. Reporting it as
 * a failure turns that stall into an immediate, named answer.
 *
 * The fix for a clip that genuinely needs to be larger is a chunked wire
 * format, not a bigger constant — see `shared/audioResponse.ts`.
 */
function oversizePayload(base64: string, what: string): AudioResultDto | null {
  if (base64.length <= MAX_AUDIO_BASE64_LENGTH) return null;
  return failed(
    'other',
    `the ${what} is too large to send to the engine: ${base64.length} base64 characters, `
    + `over the ${MAX_AUDIO_BASE64_LENGTH} limit`,
  );
}

/**
 * Translates a thrown value into a typed failure. A `MicrophoneCaptureError`
 * already carries the right kind — `capture.ts` spells `MicrophoneErrorKind`
 * with the wire's own strings precisely so this assignment is a checked
 * literal match. Everything else takes `fallback`, which each caller picks
 * for the trait that asked (a synthesis throw is `synthesis_failed`, so it
 * round-trips to `TtsError::SynthesisFailed` rather than `TtsError::Other`).
 */
function failureFrom(cause: unknown, fallback: AudioErrorKindDto): AudioResultDto {
  if (cause instanceof MicrophoneCaptureError) return failed(cause.kind, describe(cause));
  return failed(fallback, describe(cause));
}

/**
 * Performs one audio operation and reports its outcome. Never throws: the
 * caller is answering an engine call that is parked on a deadline, so every
 * path here — including an unrecognised op — has to end in an
 * `AudioResultDto`.
 */
export async function serviceAudioOp(op: AudioOpDto, deps: AudioRequestDeps): Promise<AudioResultDto> {
  switch (op.type) {
    case 'is_recording':
      try {
        return { type: 'recording_state', recording: deps.recorder.isRecording() };
      } catch (cause) {
        return failureFrom(cause, 'other');
      }

    case 'start_recording':
      try {
        // Asked here rather than left to `MicrophoneCapture.start`'s own
        // guard so the answer is `busy` — which round-trips to
        // `VoiceError::Busy` — instead of the generic `other` a plain
        // `Error` would collapse to.
        if (deps.recorder.isRecording()) {
          return failed('busy', 'a microphone capture is already in progress on this client');
        }
        await deps.recorder.start({ sampleRateHz: op.sample_rate_hz, format: op.format });
        return { type: 'ok' };
      } catch (cause) {
        return failureFrom(cause, 'other');
      }

    case 'stop_recording':
      try {
        // Same reason as `busy` above: `not_recording` is a distinct wire
        // kind mapping to `VoiceError::NotRecording`. Answering with an
        // empty clip instead would be indistinguishable from a microphone
        // that heard nothing.
        if (!deps.recorder.isRecording()) {
          return failed('not_recording', 'the desktop client is not recording, so there is nothing to stop');
        }
        const recording = await deps.recorder.stop();
        if (recording.mimeType.trim().length === 0) {
          // The mime type travels verbatim into `VoiceRecording.mime_type`.
          // An empty one is rejected by the command gate, which would strand
          // the engine for 30s — report the broken recorder instead.
          return failed('other', 'the microphone recorder reported no mime type for the captured clip');
        }
        return oversizePayload(recording.audioBase64, 'captured clip')
          ?? { type: 'recording', audio_base64: recording.audioBase64, mime_type: recording.mimeType };
      } catch (cause) {
        return failureFrom(cause, 'other');
      }

    case 'transcribe':
      // See the module doc. This is deliberately a failure, not empty text.
      return failed('unavailable', DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE);

    case 'synthesize':
      try {
        const preference = deps.playback();
        const result = await deps.synthesize(op.text, op.voice ?? preference.voiceSelection, preference.rate);
        // `{ pcm_base64: '', sample_rate_hz: 0 }` is a SUCCESS — the
        // "already played in place" convention documented in `synthesis.ts`
        // and on `TextToSpeech::synthesize` in `audio_bridge.rs`.
        return oversizePayload(result.pcmBase64, 'synthesized audio')
          ?? { type: 'audio', pcm_base64: result.pcmBase64, sample_rate_hz: result.sampleRateHz };
      } catch (cause) {
        return failureFrom(cause, 'synthesis_failed');
      }

    default:
      // `AudioOpDto` is `#[non_exhaustive]`: a newer engine can ask for
      // something this build cannot do. Saying so is the honest answer, and
      // it is the only one that does not park the caller.
      return failed(
        'unavailable',
        `the desktop client cannot perform the audio operation "${(op as { type: string }).type}"`,
      );
  }
}

/**
 * Answers one engine event. Ignores everything that is not an
 * `audio_request`; for one that is, sends exactly one `audio_response`.
 *
 * `deps` is a factory rather than a value so the caller only touches the
 * browser audio globals when a request actually arrives — and a factory that
 * throws still produces a response rather than stranding the engine.
 * `onError` receives any cause that was turned into a reported failure or
 * that the send itself produced; it is for diagnostics, and this function
 * resolves either way.
 */
export async function handleAudioRequestEvent(
  sessionId: string,
  event: ClientEvent,
  deps: () => AudioRequestDeps,
  send: AudioResponseSender,
  onError?: AudioFailureReporter,
): Promise<void> {
  if (event.type !== 'audio_request') return;

  let result: AudioResultDto;
  try {
    result = await serviceAudioOp(event.op, deps());
  } catch (cause) {
    // `serviceAudioOp` is written not to throw, but `deps()` can, and the
    // engine is parked either way — so a throw that reaches here becomes a
    // reported failure instead of an unanswered request.
    report(onError, cause);
    result = failed('other', `the desktop client could not service the audio request: ${describe(cause)}`);
  }

  try {
    await send(sessionId, { type: 'audio_response', request_id: event.request_id, result });
  } catch (cause) {
    // The engine drops responses for request ids it no longer knows. Losing
    // the UI over one is far worse than losing the answer.
    report(onError, cause);
  }
}
