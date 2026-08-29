/**
 * Desktop voice preferences model.
 *
 * Mirrors the VALUE vocabulary of iOS's `VoicePreferencesSnapshot`
 * (`clients/ios/Sources/Voice/VoiceRuntimeConfiguration.swift`) and
 * Android's `VoiceConfig` (`clients/android/app/src/main/java/com/lingxi/code/model/SettingsModels.kt`,
 * normalized by `clients/android/.../settings/VoiceSettingsRepository.kt`)
 * byte-for-byte, so a `recognitionMode`/`voiceSelection` string written on
 * one platform means the same thing when read back on another.
 *
 * What is DELIBERATELY NOT mirrored: key names (iOS persists dotted
 * `voice.recognitionMode` keys, Android persists snake_case keys in its own
 * `voice_settings` store — desktop keeps its own settings-file conventions,
 * see `shared/settings.ts`) and legacy-key migration (iOS's
 * `legacyRecognitionMode`/`legacySystemVoice`/`voiceSpeed`/`voiceAutoPlay`
 * and the old `"on-device"` spelling; Android's
 * `migrateLegacyVoiceSelection`). Desktop has never shipped voice settings,
 * so there is no legacy state to migrate — only normalization (accepting
 * today's contract leniently) is ported.
 *
 * This lives in `shared/` — not `main/` or `renderer/` — because, like
 * `PublicSettings` in `shared/settings.ts`, it has to be reachable from the
 * main process (which persists it) and eventually the renderer (which will
 * read/render it), and a type declared independently in each of those
 * drifts exactly the way `bypassPermissionsModeAccepted` once did.
 */

/** Schema version of a persisted `VoicePreferences` value. */
export const VOICE_SCHEMA_VERSION = 2 as const;

export const LANGUAGE_AUTO = 'auto';
export const DEFAULT_VOICE_ID = 'default';
export const SYSTEM_VOICE_PREFIX = 'system:';
export const SHERPA_VOICE_PREFIX = 'sherpa:';
export const DEFAULT_VOICE_SELECTION = `${SYSTEM_VOICE_PREFIX}${DEFAULT_VOICE_ID}`;

const MIN_RATE = 0.5;
const MAX_RATE = 2.0;
const DEFAULT_RATE = 1.0;

export interface VoicePreferences {
  schemaVersion: typeof VOICE_SCHEMA_VERSION;
  /**
   * `'localOnly'` is the only non-default value mobile persists (Android's
   * `VoiceConfig.MODE_LOCAL_ONLY`; iOS's `VoiceRecognitionMode.onDevice` is
   * the Swift *case name* — its raw, persisted string is also `"localOnly"`).
   * Anything else, including the case name `"onDevice"` itself, normalizes
   * to `'automatic'`.
   */
  recognitionMode: 'automatic' | 'localOnly';
  language: string;
  voiceSelection: string;
  rate: number;
  autoPlayReplies: boolean;
}

/** The value a fresh install (no persisted voice preferences yet) gets on every platform. */
export function defaultVoicePreferences(): VoicePreferences {
  return {
    schemaVersion: VOICE_SCHEMA_VERSION,
    recognitionMode: 'automatic',
    language: LANGUAGE_AUTO,
    voiceSelection: DEFAULT_VOICE_SELECTION,
    rate: DEFAULT_RATE,
    autoPlayReplies: false,
  };
}

/** Port of Android's `normalizeRecognitionMode`. Anything but the literal `'localOnly'` falls back to `'automatic'` — lenient, never throws. */
export function normalizeRecognitionMode(raw: unknown): 'automatic' | 'localOnly' {
  return raw === 'localOnly' ? 'localOnly' : 'automatic';
}

/** Port of Android's `normalizeLanguage`. Trims; empty or case-insensitive `"auto"` becomes `LANGUAGE_AUTO`; otherwise the trimmed value. */
export function normalizeLanguage(raw: unknown): string {
  const trimmed = typeof raw === 'string' ? raw.trim() : '';
  if (trimmed === '' || trimmed.toLowerCase() === LANGUAGE_AUTO) return LANGUAGE_AUTO;
  return trimmed;
}

/** Port of iOS's `VoicePreferencesSnapshot.normalizeVoiceSelection` / Android's `normalizeVoiceSelection` (legacy-alias branches excluded — see file header). */
export function normalizeVoiceSelection(raw: string | undefined): string {
  const trimmed = (raw ?? '').trim();
  if (trimmed === '' || trimmed === DEFAULT_VOICE_ID) return DEFAULT_VOICE_SELECTION;
  if (trimmed.startsWith(SYSTEM_VOICE_PREFIX) || trimmed.startsWith(SHERPA_VOICE_PREFIX)) return trimmed;
  return `${SYSTEM_VOICE_PREFIX}${trimmed}`;
}

/** Port of Android's `rate.coerceIn(0.5f, 2.0f)` / iOS's `min(2, max(0.5, rate))` — clamps, never rejects; defaults to 1.0 when absent or unparseable. */
function normalizeRate(raw: unknown): number {
  const value = typeof raw === 'number' && Number.isFinite(raw) ? raw : DEFAULT_RATE;
  return Math.min(MAX_RATE, Math.max(MIN_RATE, value));
}

/** Parse an arbitrary (e.g. persisted-JSON or IPC-supplied) value into a complete, normalized `VoicePreferences`. Never throws. */
export function parseVoicePreferences(value: unknown): VoicePreferences {
  const raw = (value !== null && typeof value === 'object' ? value : {}) as Record<string, unknown>;
  return {
    schemaVersion: VOICE_SCHEMA_VERSION,
    recognitionMode: normalizeRecognitionMode(raw['recognitionMode']),
    language: normalizeLanguage(raw['language']),
    voiceSelection: normalizeVoiceSelection(
      typeof raw['voiceSelection'] === 'string' ? raw['voiceSelection'] : undefined,
    ),
    rate: normalizeRate(raw['rate']),
    autoPlayReplies: raw['autoPlayReplies'] === true,
  };
}
