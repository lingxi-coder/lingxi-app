/**
 * Speech synthesis for the desktop renderer, over `window.speechSynthesis`.
 *
 * This is the client side of the engine's `TextToSpeech` trait
 * (`lingxi-code/traits/src/tts.rs`), proxied over the wire by
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
  /** Overrides `readSystemVoices`'s wait bound; tests use a short value. */
  voiceListTimeoutMs?: number;
}

/** Strips the `system:` selection prefix, or returns `null` for anything else (a foreign `sherpa:` id, an empty selection, garbage). */
function systemVoiceName(voiceId: string): string | null {
  return voiceId.startsWith(SYSTEM_VOICE_PREFIX) ? voiceId.slice(SYSTEM_VOICE_PREFIX.length) : null;
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
  const targetName = systemVoiceName(voiceId);
  const voices = targetName != null ? await readSystemVoices(deps.synth, deps.voiceListTimeoutMs) : [];
  const voice = targetName != null ? voices.find((candidate) => candidate.name === targetName) ?? null : null;

  await deps.speak(text, voice, rate);

  // See the module doc's "empty-PCM 'played in place' convention" section —
  // this is a success, not a placeholder for a missing implementation.
  return { pcmBase64: '', sampleRateHz: 0 };
}

/** Builds `SynthesisDeps` from the real browser globals, for production use. */
export function browserSynthesisDeps(): SynthesisDeps {
  return {
    synth: window.speechSynthesis,
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
