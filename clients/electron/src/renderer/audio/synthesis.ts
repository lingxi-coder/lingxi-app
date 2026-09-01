/**
 * Speech synthesis for the desktop renderer, over `window.speechSynthesis`.
 *
 * This is the client side of the engine's `TextToSpeech` trait
 * (`lingxi-code/platform-api/src/tts.rs`), proxied over the wire by
 * `lingxi-code/apps/bridge-server/src/audio_bridge.rs`'s `AudioBridge`
 * (`AudioOpDto::Synthesize` / `AudioResultDto::Audio`). `requests.ts` is the
 * caller that services those requests and lowers the outcome onto
 * `audio_response`; this module only has to actually speak the text and
 * report the outcome.
 *
 * ## The empty-PCM "played in place" convention — read this before touching
 * ## the return value
 *
 * `window.speechSynthesis` PLAYS audio through the OS directly; it hands the
 * page no samples at all, so this module has no PCM bytes to return, ever.
 * The agreed convention (mirrored in a comment on `TextToSpeech::synthesize`
 * in `lingxi-code/apps/bridge-server/src/audio_bridge.rs`) is that a
 * successful `synthesize()` here answers with `{ pcmBase64: '', sampleRateHz:
 * 0 }`, which a caller lowers to `AudioResultDto::Audio { pcm_base64: "",
 * sample_rate_hz: 0 }` — meaning **"already played in place,"  a genuine
 * success, not a failure and not an omission**. `audio_bridge.rs`'s
 * `TextToSpeech::synthesize` base64-decodes that payload (`decode("")` is
 * `Ok(vec![])`), so this round-trips as `TtsAudio { pcm: vec![], sample_rate_hz:
 * 0 }` without erroring. Do not "fix" this into throwing on an empty
 * transcript-equivalent — that would silently kill desktop TTS. If a future
 * caller needs real PCM bytes (e.g. to also forward audio to a transcript or
 * a recording), that requires switching to `OfflineAudioContext` rendering
 * instead of `speechSynthesis`, which is explicitly a separate piece of work.
 *
 * Voice selection reuses `capabilities.ts`'s `SpeechSynthesisLike` and
 * `readSystemVoices` rather than re-deriving its own voice list or its own
 * `voiceschanged`-race handling — a second copy of that race-handling logic
 * is exactly the kind of parallel copy this codebase's consolidation under
 * `shared/` (see `preferences.ts`'s doc comment) already exists to prevent.
 * `voiceId` is expected in the same `system:<name>` grammar `probePlatform`
 * produces (`VoicePlatformSnapshot.systemVoices[].id`); an id that does not
 * match any enumerated voice — a stale selection, the `system:default`
 * sentinel, or a foreign `sherpa:` id synced from a mobile device that has no
 * meaning on desktop — resolves to the platform's own default voice (`voice:
 * null` in the Web Speech API) rather than throwing, matching
 * `resolveCapabilities`'s "fall back to the nearest available, never crash"
 * discipline.
 *
 * `rate` is forwarded as-is: `shared/voicePreferences.ts`'s `normalizeRate`
 * is the single place that clamps it to `[0.5, 2.0]`, so this module does
 * not clamp a second time.
 *
 * `speak` bundles utterance construction, event wiring, and the `speak()`
 * call into one injected function rather than splitting "create an
 * utterance" and "speak it" into two separately-testable seams: the real
 * `SpeechSynthesis.speak` API requires the SAME `SpeechSynthesisUtterance`
 * instance it was handed back, so a structurally-equivalent stand-in object
 * passed later would throw in a real browser. See `SynthesisDeps.speak`'s
 * doc comment.
 */

import { readSystemVoices, type SpeechSynthesisLike } from './capabilities.js';
import { SYSTEM_VOICE_PREFIX } from './preferences.js';

export interface SynthesisResult {
  pcmBase64: string;
  sampleRateHz: number;
}

export interface SynthesisDeps {
  /** Voice enumeration; see `capabilities.ts`'s `readSystemVoices`. */
  synth: SpeechSynthesisLike;
  /**
   * Speaks `text` with `voice` (the platform default when `null`) at `rate`,
   * resolving when playback finishes and rejecting on a synthesis error.
   * Bundled rather than split into `createUtterance` + `speak` — see the
   * module doc for why.
   */
  speak(text: string, voice: SpeechSynthesisVoice | null, rate: number): Promise<void>;
  /**
   * Stops whatever the synthesizer is speaking or has queued.
   *
   * The Web Speech API has no per-utterance stop — `speechSynthesis.cancel()`
   * clears the whole queue — so this is only ever called when this module has
   * ALREADY abandoned an utterance, which is exactly when leaving the device
   * talking would make the reported state and the real state disagree.
   */
  cancel(): void;
  /** Overrides `readSystemVoices`'s wait bound; tests use a short value. */
  voiceListTimeoutMs?: number;
}

/**
 * How long one character of text may take to speak, at the SLOWEST rate the
 * device offers. `normalizeRate` clamps to `[0.5, 2.0]` and the Web Speech
 * API's default is ~175 wpm, so the floor is ~7 characters a second; 200ms
 * leaves margin on top of that. `SYNTHESIS_MILLIS_PER_CHAR` in
 * `audio_bridge.rs` is the same number on the engine side, and
 * `audio-engine-bounds.test.ts` pins the engine's derived deadline above this
 * client's own bound so the honest, device-stopping failure below always wins
 * the race.
 */
export const SPEECH_MILLIS_PER_CHARACTER = 200;

/**
 * Fixed part of the bound: acquiring the audio session, picking the voice, and
 * — because `window.speechSynthesis` is ONE global queue shared by every
 * session — waiting behind an utterance another session started. Generous on
 * purpose: firing early aborts work that was going to succeed, and because
 * `cancel()` clears the whole queue, firing early would also cut off the other
 * session's speech.
 */
export const SPEECH_START_ALLOWANCE_MS = 60_000;

/**
 * The longest text this client will speak in one call.
 *
 * Without it the derivation below is unbounded — a caller could park the
 * engine's tool call for hours — and clamping the derivation instead would
 * just recreate the defect for texts past the clamp: a bound that reports
 * failure while the machine is still talking. Refusing up front is the honest
 * answer, and it is instant: nothing is queued, nothing is cut off, and the
 * model is told to split the text.
 */
export const MAX_SPOKEN_CHARACTERS = 4000;

/**
 * How long `text` may take to finish being spoken.
 *
 * Counted in Unicode scalar values (`[...text]`), NOT `text.length`: the engine
 * counts `text.chars()`, and a UTF-16 count would be larger for any non-BMP
 * character — which would make this client wait longer than the engine for the
 * same text, so the engine's deadline would fire first and the whole point of
 * this bound (a failure that also stops the device) would be lost.
 */
export function spokenTextTimeoutMs(text: string): number {
  return SPEECH_START_ALLOWANCE_MS + [...text].length * SPEECH_MILLIS_PER_CHARACTER;
}

/** Strips the `system:` selection prefix, or returns `null` for anything else (a foreign `sherpa:` id, an empty selection, garbage). */
function systemVoiceName(voiceId: string): string | null {
  return voiceId.startsWith(SYSTEM_VOICE_PREFIX) ? voiceId.slice(SYSTEM_VOICE_PREFIX.length) : null;
}

/**
 * Speaks `text`, and stops the device on any path that gives up waiting.
 *
 * Two things travel together here on purpose. The bound exists because
 * `deps.speak` resolves only on `utterance.onend`, which a stalled synthesizer
 * never fires — without it the only thing that ever fires is the ENGINE's
 * deadline, in a process that cannot reach `speechSynthesis`. And the
 * `cancel()` exists because the engine's deadline could only ever abandon the
 * WAIT: the machine kept talking, the model was told the synthesis failed, and
 * a retry queued a second utterance behind the still-playing first. Whoever
 * stops waiting has to stop the device, which is only possible here.
 *
 * The timer is cleared on the success path so a finished utterance leaves
 * nothing pending behind it.
 */
async function speakWithin(
  text: string,
  voice: SpeechSynthesisVoice | null,
  rate: number,
  deps: SynthesisDeps,
): Promise<void> {
  const limit = spokenTextTimeoutMs(text);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      timer = setTimeout(() => {
        reject(new Error(
          `the system speech synthesizer stopped responding: it did not finish speaking `
          + `${[...text].length} characters within ${Math.round(limit / 1000)}s`,
        ));
      }, limit);
      deps.speak(text, voice, rate).then(resolve, reject);
    });
  } catch (cause) {
    try {
      deps.cancel();
    } catch {
      // A synthesizer that cannot even be cancelled must not replace the real
      // failure with its own.
    }
    throw cause;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Speaks `text` through the system voice named by `voiceId` at `rate`, and
 * reports the "already played in place" convention described in the module
 * doc. Rejects with whatever `deps.speak` rejects with — a genuine synthesis
 * failure is never swallowed into a false success.
 */
export async function synthesize(
  text: string,
  voiceId: string,
  rate: number,
  deps: SynthesisDeps,
): Promise<SynthesisResult> {
  const characters = [...text].length;
  if (characters > MAX_SPOKEN_CHARACTERS) {
    // Refused before anything is queued: see `MAX_SPOKEN_CHARACTERS`. The
    // caller lowers this to `synthesis_failed`, so the model is told to split
    // the text rather than left waiting on speech that would outlast any bound.
    throw new Error(
      `this text is too long to speak in one call: ${characters} characters, over the `
      + `${MAX_SPOKEN_CHARACTERS} this client will speak at once — split it into shorter calls`,
    );
  }

  const targetName = systemVoiceName(voiceId);
  const voices = targetName != null ? await readSystemVoices(deps.synth, deps.voiceListTimeoutMs) : [];
  const voice = targetName != null ? voices.find((candidate) => candidate.name === targetName) ?? null : null;

  await speakWithin(text, voice, rate, deps);

  // See the module doc's "empty-PCM 'played in place' convention" section —
  // this is a success, not a placeholder for a missing implementation.
  return { pcmBase64: '', sampleRateHz: 0 };
}

/** Builds `SynthesisDeps` from the real browser globals, for production use. */
export function browserSynthesisDeps(): SynthesisDeps {
  return {
    synth: window.speechSynthesis,
    cancel: () => window.speechSynthesis.cancel(),
    speak: (text, voice, rate) => new Promise<void>((resolve, reject) => {
      const utterance = new SpeechSynthesisUtterance(text);
      utterance.voice = voice;
      utterance.rate = rate;
      utterance.onend = () => resolve();
      utterance.onerror = (event) => reject(new Error(`speech synthesis failed: ${event.error}`));
      window.speechSynthesis.speak(utterance);
    }),
  };
}
