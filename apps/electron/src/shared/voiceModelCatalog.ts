import {
  GENERATED_VOICE_MODEL_CATALOG,
  type GeneratedOfflineModelEntry,
  type GeneratedSherpaRuntimeParams,
  type GeneratedTtsVoiceEntry,
  type GeneratedVoicePack,
} from './generatedVoiceModels.js';

export type SherpaRuntimeParams = GeneratedSherpaRuntimeParams;
export type OfflineVoiceEntry = GeneratedTtsVoiceEntry;
export type OfflineModelEntry = GeneratedOfflineModelEntry;
export type VoicePackEntry = GeneratedVoicePack;

const macOSRuntimeArtifact = GENERATED_VOICE_MODEL_CATALOG.macOSRuntimeArtifact;

/** Desktop-only runtime pin; mobile artifacts retain their independent 1.13.2 pin. */
export const desktopSherpaRuntime = {
  version: macOSRuntimeArtifact.sherpaVersion,
  onnxRuntimeVersion: macOSRuntimeArtifact.onnxruntimeVersion,
  macos: macOSRuntimeArtifact,
} as const;

/** Canonical model and pack data generated from resources/voice/models.json. */
export const offlineVoiceModels: readonly OfflineModelEntry[] = GENERATED_VOICE_MODEL_CATALOG.models;
export const voicePacks: readonly VoicePackEntry[] = GENERATED_VOICE_MODEL_CATALOG.packs;

const byId = new Map<string, OfflineModelEntry>(offlineVoiceModels.map((model) => [model.id, model]));

export function offlineVoiceModelById(id: string): OfflineModelEntry | undefined {
  return byId.get(id);
}

export function voicePackFor(languageIdentifier: string): readonly OfflineModelEntry[] {
  const base = languageIdentifier.replace(/_/g, '-').split('-')[0]?.toLowerCase() ?? '';
  const pack = voicePacks.find((entry) => entry.language === base);
  return (pack?.modelIds ?? []).flatMap((id) => {
    const model = byId.get(id);
    return model ? [model] : [];
  });
}
