/** Current device-local v4 audio configuration. */
import {
  AUDIO_CONFIGURATION_SCHEMA_VERSION,
  AUDIO_LANGUAGE_AUTO,
  audioConfigurationDefaults,
  normalizeAudioConfiguration,
  type AudioConfigurationV4,
} from './generatedAudioConfiguration.js';

export const VOICE_SCHEMA_VERSION = AUDIO_CONFIGURATION_SCHEMA_VERSION;
export const LANGUAGE_AUTO = AUDIO_LANGUAGE_AUTO;
export type VoicePreferences = AudioConfigurationV4;
export function defaultVoicePreferences(): VoicePreferences { return audioConfigurationDefaults(); }
export function parseVoicePreferences(value: unknown): VoicePreferences { return normalizeAudioConfiguration(value); }
export { normalizeAudioConfiguration };
