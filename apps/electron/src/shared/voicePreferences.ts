/**
 * Device-local audio configuration compatibility exports.
 *
 * The v3 schema and normalization/migration rules are generated from
 * `resources/voice`; this file keeps the existing Desktop settings property
 * name and call sites stable while all consumers move to independent STT/TTS
 * source selections.
 */

import {
  AUDIO_CONFIGURATION_SCHEMA_VERSION,
  AUDIO_LANGUAGE_AUTO,
  audioConfigurationDefaults,
  migrateLegacyAudioConfiguration,
  normalizeAudioConfiguration,
  type AudioConfigurationV3,
} from './generatedAudioConfiguration.js';

export const VOICE_SCHEMA_VERSION = AUDIO_CONFIGURATION_SCHEMA_VERSION;
export const LANGUAGE_AUTO = AUDIO_LANGUAGE_AUTO;

/** Historical spellings retained only for migration and legacy aliases. */
export const DEFAULT_VOICE_ID = 'default';
export const SYSTEM_VOICE_PREFIX = 'system:';
export const SHERPA_VOICE_PREFIX = 'sherpa:';
export const DEFAULT_VOICE_SELECTION = 'system:default';

export type VoicePreferences = AudioConfigurationV3;

export function defaultVoicePreferences(): VoicePreferences {
  return audioConfigurationDefaults();
}

/** Normalize current values and migrate saved pre-v3 Desktop settings. */
export function parseVoicePreferences(value: unknown): VoicePreferences {
  return migrateLegacyAudioConfiguration(value);
}

export function isLegacySystemVoiceAlias(value: string): boolean {
  if (!value.startsWith(SYSTEM_VOICE_PREFIX)) return false;
  const payload = value.slice(SYSTEM_VOICE_PREFIX.length);
  return payload.length > 0 && !payload.includes('.');
}

export { normalizeAudioConfiguration };
