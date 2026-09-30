// Generated from resources/voice/audio-config-schema.json and audio-config-fixtures.json.
// Do not edit by hand; run node resources/voice/scripts/generate-audio-config.mjs.

export const AUDIO_CONFIGURATION_SCHEMA_VERSION = 3 as const;
export const AUDIO_LANGUAGE_AUTO = "auto" as const;
export const AUDIO_MIN_RATE = 0.5 as const;
export const AUDIO_MAX_RATE = 2.0 as const;
export const AUDIO_DEFAULT_RATE = 1.0 as const;
export const AUDIO_CONFIGURATION_DEFAULTS = {
  "schemaVersion": 3,
  "recognition": {
    "source": "automatic",
    "offlineModelId": null
  },
  "speech": {
    "source": "automatic",
    "offlineModelId": null,
    "voice": null
  },
  "language": "auto",
  "rate": 1,
  "autoPlayReplies": false
} as const;

export type AudioSource = string;
export type AudioProviderKind = 'recognition' | 'speech';
export type AudioReadiness = 'available' | 'permissionRequired' | 'denied' | 'unavailable';
export type AudioRouteStatus = 'ready' | 'permissionRequired' | 'unavailable' | 'invalidRequest';
export type AudioFallbackFailure = 'permission' | 'unavailable' | 'busy' | 'cancelled' | 'timeout' | 'invalidRequest' | 'noSpeech' | 'nativeFailure';

export interface AudioVoiceSelection {
  source: AudioSource;
  id: string;
  modelId?: string;
}

export interface AudioRecognitionPreference {
  source: AudioSource;
  offlineModelId: string | null;
}

export interface AudioSpeechPreference extends AudioRecognitionPreference {
  voice: AudioVoiceSelection | null;
}

export interface AudioConfigurationV3 {
  schemaVersion: typeof AUDIO_CONFIGURATION_SCHEMA_VERSION;
  recognition: AudioRecognitionPreference;
  speech: AudioSpeechPreference;
  language: string;
  rate: number;
  autoPlayReplies: boolean;
}

export interface AudioVoiceCatalogEntry {
  source: 'system' | 'offline' | string;
  id: string;
  modelId?: string;
  label?: string;
  displayName?: string;
  aliases?: readonly string[];
}

export interface AudioOfflineModelAvailability {
  id: string;
  kind: AudioProviderKind;
  languages: readonly string[];
  installed: boolean;
  voiceIds?: readonly string[];
}

export interface AudioRouteRequest {
  kind: AudioProviderKind;
  preference: AudioRecognitionPreference | AudioSpeechPreference;
  /** Must be resolved from the config/device locale when the operation begins. */
  language: string;
  systemStatus: AudioReadiness;
  /** Keep this in shared model catalog order for deterministic automatic selection. */
  offlineModels: readonly AudioOfflineModelAvailability[];
  systemVoiceIds?: readonly string[];
  /** An explicit per-operation selection overrides a stored voice, including null. */
  voiceOverride?: AudioVoiceSelection | null;
}

export interface AudioRouteResolution {
  requested: { source: string; offlineModelId: string | null; voice: AudioVoiceSelection | null };
  effective: { source: 'system' | 'offline'; modelId: string | null; voiceId: string | null } | null;
  status: AudioRouteStatus;
  reason: string;
  fallbackReason: string | null;
}

function asObject(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function normalizeSource(value: unknown, fallback = 'automatic'): string {
  const source = typeof value === 'string' ? value.trim() : '';
  return source || fallback;
}

function normalizeModelId(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function normalizeLanguage(value: unknown): string {
  const language = typeof value === 'string' ? value.trim() : '';
  return !language || language.toLowerCase() === AUDIO_LANGUAGE_AUTO ? AUDIO_LANGUAGE_AUTO : language;
}

function normalizeRate(value: unknown): number {
  const rate = typeof value === 'number' && Number.isFinite(value) ? value : AUDIO_DEFAULT_RATE;
  return Math.min(AUDIO_MAX_RATE, Math.max(AUDIO_MIN_RATE, rate));
}

function uniqueVoiceMatch(catalog: readonly AudioVoiceCatalogEntry[], selector: string): AudioVoiceCatalogEntry | null {
  const needle = selector.toLowerCase();
  const matches = catalog.filter((entry) => [entry.id, entry.label, entry.displayName, ...(entry.aliases ?? [])]
    .some((name) => typeof name === 'string' && name.toLowerCase() === needle));
  return matches.length === 1 ? matches[0] : null;
}

function migrateLegacyVoice(value: string, catalog: readonly AudioVoiceCatalogEntry[]): AudioVoiceSelection | null {
  const raw = value.trim();
  if (!raw) return null;
  if (raw.startsWith('system:')) {
    const id = raw.slice('system:'.length);
    const match = uniqueVoiceMatch(catalog.filter((entry) => entry.source === 'system'), id);
    return { source: 'system', id: match?.id ?? id };
  }
  if (raw.startsWith('sherpa:')) {
    const payload = raw.slice('sherpa:'.length);
    const splitAt = payload.indexOf(':');
    const modelKey = splitAt < 0 ? payload : payload.slice(0, splitAt);
    const voiceKey = splitAt < 0 ? payload : payload.slice(splitAt + 1);
    const offlineCatalog = catalog.filter((entry) => entry.source === 'offline');
    const match = uniqueVoiceMatch(offlineCatalog, `${modelKey}:${voiceKey}`)
      ?? uniqueVoiceMatch(offlineCatalog, voiceKey);
    if (match?.modelId && (match.modelId === modelKey || match.modelId.endsWith(modelKey))) {
      return { source: 'offline', modelId: match.modelId, id: match.id };
    }
    return { source: 'offline', modelId: modelKey, id: voiceKey };
  }
  if (raw === 'default') return { source: 'system', id: 'default' };
  const match = uniqueVoiceMatch(catalog, raw);
  if (match?.source === 'offline' && match.modelId) return { source: 'offline', modelId: match.modelId, id: match.id };
  if (match?.source === 'system') return { source: 'system', id: match.id };
  return { source: 'system', id: raw };
}

function normalizeVoice(value: unknown, catalog: readonly AudioVoiceCatalogEntry[] = []): AudioVoiceSelection | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return migrateLegacyVoice(value, catalog);
  const raw = asObject(value);
  const source = normalizeSource(raw.source, '');
  const id = typeof raw.id === 'string' ? raw.id : '';
  if (!source || !id) return null;
  if (source === 'offline') {
    const modelId = normalizeModelId(raw.modelId);
    return modelId ? { source, modelId, id } : { source, id };
  }
  return { source, id };
}

function normalizePreference(value: unknown, kind: AudioProviderKind): AudioRecognitionPreference | AudioSpeechPreference {
  const raw = asObject(value);
  const preference: AudioRecognitionPreference | AudioSpeechPreference = {
    source: normalizeSource(raw.source),
    offlineModelId: normalizeModelId(raw.offlineModelId),
    ...(kind === 'speech' ? { voice: normalizeVoice(raw.voice) } : {}),
  } as AudioRecognitionPreference | AudioSpeechPreference;
  return preference;
}

export function audioConfigurationDefaults(): AudioConfigurationV3 {
  return JSON.parse(JSON.stringify(AUDIO_CONFIGURATION_DEFAULTS)) as AudioConfigurationV3;
}

export function normalizeAudioConfiguration(value: unknown): AudioConfigurationV3 {
  const raw = asObject(value);
  const recognition = normalizePreference(raw.recognition, 'recognition') as AudioRecognitionPreference;
  const speech = normalizePreference(raw.speech, 'speech') as AudioSpeechPreference;
  if (speech.source === 'automatic' && speech.voice) {
    speech.source = speech.voice.source;
    if (speech.voice.source === 'offline' && speech.offlineModelId === null) {
      speech.offlineModelId = speech.voice.modelId ?? null;
    }
  }
  return {
    schemaVersion: AUDIO_CONFIGURATION_SCHEMA_VERSION,
    recognition,
    speech,
    language: normalizeLanguage(raw.language),
    rate: normalizeRate(raw.rate),
    autoPlayReplies: raw.autoPlayReplies === true,
  };
}

function mapLegacySource(value: unknown, fallback: string): string {
  if (value === null || value === undefined || value === '') return fallback;
  const raw = typeof value === 'string' ? value.trim() : String(value);
  switch (raw.toLowerCase()) {
    case 'auto':
    case 'automatic': return 'automatic';
    case 'system': return 'system';
    case 'offline':
    case 'localonly':
    case 'on-device':
    case 'ondevice': return 'offline';
    default: return raw;
  }
}

function firstDefined(raw: Record<string, unknown>, names: readonly string[]): unknown {
  for (const name of names) if (raw[name] !== undefined && raw[name] !== null) return raw[name];
  return undefined;
}

export function migrateLegacyAudioConfiguration(value: unknown, voiceCatalog: readonly AudioVoiceCatalogEntry[] = []): AudioConfigurationV3 {
  const raw = asObject(value);
  if (raw.schemaVersion === AUDIO_CONFIGURATION_SCHEMA_VERSION) return normalizeAudioConfiguration(raw);
  const recognitionRaw = asObject(raw.recognition);
  const speechRaw = asObject(raw.speech);
  const recognitionSource = mapLegacySource(
    recognitionRaw.source ?? firstDefined(raw, ['inputProvider', 'recognitionMode']), 'automatic');
  const speechSource = mapLegacySource(
    speechRaw.source ?? firstDefined(raw, ['outputProvider', 'speechProvider']), 'automatic');
  const selectedVoice = speechRaw.voice ?? firstDefined(raw, ['voiceSelection', 'voiceId', 'voice']);
  const voice = typeof selectedVoice === 'string' ? migrateLegacyVoice(selectedVoice, voiceCatalog) : normalizeVoice(selectedVoice, voiceCatalog);
  let language = firstDefined(raw, ['language', 'inputLanguage', 'legacyInputLanguage', 'voiceLanguage', 'voiceLang']);
  if ((!language || language === 'auto') && typeof raw.legacyVoiceLang === 'string') {
    const locale = raw.legacyVoiceLang.toLowerCase();
    if (locale === 'zh') language = 'zh-CN';
    if (locale === 'en') language = 'en-US';
  }
  const speech: AudioSpeechPreference = {
    source: speechSource,
    offlineModelId: normalizeModelId(speechRaw.offlineModelId ?? firstDefined(raw, ['speechModelId', 'outputModelId'])),
    voice,
  };
  if (speech.source === 'automatic' && voice) {
    speech.source = voice.source;
    if (voice.source === 'offline' && speech.offlineModelId === null) speech.offlineModelId = voice.modelId ?? null;
  }
  return normalizeAudioConfiguration({
    schemaVersion: AUDIO_CONFIGURATION_SCHEMA_VERSION,
    recognition: {
      source: recognitionSource,
      offlineModelId: recognitionRaw.offlineModelId ?? firstDefined(raw, ['recognitionModelId', 'inputModelId']),
    },
    speech,
    language,
    rate: firstDefined(raw, ['rate', 'speed', 'voiceSpeed']),
    autoPlayReplies: firstDefined(raw, ['autoPlayReplies', 'autoPlay', 'voiceAutoPlay']),
  });
}

export function resolveAudioLanguage(configured: unknown, deviceLocale: unknown): string {
  const language = normalizeLanguage(configured);
  if (language !== AUDIO_LANGUAGE_AUTO) return language;
  return typeof deviceLocale === 'string' && deviceLocale.trim() ? deviceLocale.trim() : 'en-US';
}

function matchesLanguage(languages: readonly string[], requested: string): boolean {
  const language = requested.toLowerCase();
  return languages.some((supported) => {
    const candidate = supported.toLowerCase();
    return language === candidate || language.startsWith(`${candidate}-`);
  });
}

function routeResult(
  requested: AudioRouteResolution['requested'],
  effective: AudioRouteResolution['effective'],
  status: AudioRouteStatus,
  reason: string,
  fallbackReason: string | null = null,
): AudioRouteResolution {
  return { requested, effective, status, reason, fallbackReason };
}

function unavailable(requested: AudioRouteResolution['requested'], reason: string, status: AudioRouteStatus = 'unavailable', fallbackReason: string | null = null): AudioRouteResolution {
  return routeResult(requested, null, status, reason, fallbackReason);
}

function resolveSystem(
  requested: AudioRouteResolution['requested'],
  request: AudioRouteRequest,
  voice: AudioVoiceSelection | null,
): AudioRouteResolution {
  const state = request.systemStatus;
  if (state === 'available' || state === 'permissionRequired') {
    const voiceId = voice?.id && voice.id !== 'default' ? voice.id : null;
    if (voiceId && request.systemVoiceIds && !request.systemVoiceIds.includes(voiceId)) {
      return unavailable(requested, 'systemVoiceUnknown');
    }
    return routeResult(requested, { source: 'system', modelId: null, voiceId }, state === 'available' ? 'ready' : 'permissionRequired', state === 'available' ? 'ready' : 'systemPermissionRequired');
  }
  return unavailable(requested, state === 'denied' ? 'systemDenied' : 'systemUnavailable');
}

function resolveOffline(
  requested: AudioRouteResolution['requested'],
  request: AudioRouteRequest,
  modelId: string | null,
  voice: AudioVoiceSelection | null,
): AudioRouteResolution {
  const models = request.offlineModels;
  let model: AudioOfflineModelAvailability | undefined;
  if (modelId !== null) {
    model = models.find((entry) => entry.id === modelId);
    if (!model) return unavailable(requested, 'offlineModelUnknown');
    if (model.kind !== request.kind) return unavailable(requested, 'offlineModelKindMismatch');
    if (!matchesLanguage(model.languages, request.language)) return unavailable(requested, 'offlineModelUnsupportedLanguage');
    if (!model.installed) return unavailable(requested, 'offlineModelNotInstalled');
  } else {
    const compatible = models.filter((entry) => entry.kind === request.kind && matchesLanguage(entry.languages, request.language));
    model = compatible.find((entry) => entry.installed);
    if (!model) return unavailable(requested, compatible.length ? 'offlineModelNotInstalled' : 'noCompatibleOfflineModel');
  }
  if (voice?.source === 'offline') {
    if (voice.modelId !== model.id) return unavailable(requested, 'offlineModelConflict', 'invalidRequest');
    if (model.voiceIds && !model.voiceIds.includes(voice.id)) return unavailable(requested, 'offlineVoiceUnknown');
  }
  return routeResult(requested, { source: 'offline', modelId: model.id, voiceId: voice?.source === 'offline' ? voice.id : null }, 'ready', 'ready');
}

export function resolveAudioRoute(request: AudioRouteRequest): AudioRouteResolution {
  const preference = request.preference;
  const source = normalizeSource(preference.source);
  const offlineModelId = normalizeModelId(preference.offlineModelId);
  const storedVoice = 'voice' in preference ? preference.voice : null;
  const voice = request.voiceOverride !== undefined ? request.voiceOverride : storedVoice;
  const requested = { source, offlineModelId, voice: voice ?? null };
  if (!request.language || normalizeLanguage(request.language) === AUDIO_LANGUAGE_AUTO) return unavailable(requested, 'languageUnresolved', 'invalidRequest');
  if (source !== 'automatic' && source !== 'system' && source !== 'offline') return unavailable(requested, 'unsupportedSource');
  if (voice && voice.source !== 'system' && voice.source !== 'offline') return unavailable(requested, 'unsupportedVoiceSource', 'invalidRequest');
  if (voice && source !== 'automatic' && voice.source !== source) return unavailable(requested, 'voiceSourceMismatch', 'invalidRequest');
  if (voice?.source === 'offline' && offlineModelId && offlineModelId !== voice.modelId) return unavailable(requested, 'offlineModelConflict', 'invalidRequest');
  if (source === 'system' || (source === 'automatic' && voice?.source === 'system')) {
    return resolveSystem(requested, request, voice?.source === 'system' ? voice : null);
  }
  if (source === 'offline' || (source === 'automatic' && voice?.source === 'offline')) {
    return resolveOffline(requested, request, voice?.source === 'offline' ? voice.modelId ?? null : offlineModelId, voice);
  }
  const system = resolveSystem(requested, request, null);
  if (system.status === 'ready' || system.status === 'permissionRequired') return system;
  const offline = resolveOffline(requested, request, offlineModelId, null);
  return { ...offline, fallbackReason: system.reason };
}

export function isAudioFallbackAllowed(failure: AudioFallbackFailure | string, operationStarted: boolean): boolean {
  return !operationStarted && (failure === 'permission' || failure === 'unavailable');
}
