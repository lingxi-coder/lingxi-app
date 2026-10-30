import { currentLocale, matchLocale, tr, translate } from '../../../i18n/locale';
import { useI18n } from '../../../i18n/useI18n';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { AudioOperationDto } from '@lingxi/bridge-client';

import {
  defaultNativeAudioSnapshot,
  type NativeAudioModelSnapshot,
  type NativeAudioSnapshot,
  type NativeAudioVoiceOption,
  type SpeechPermissionStatus,
} from '../../../../shared/nativeAudio';
import {
  AUDIO_LANGUAGE_AUTO,
  audioConfigurationDefaults,
  resolveAudioLanguage,
  resolveAudioRoute,
  type AudioConfigurationV4,
  type AudioOfflineModelAvailability,
  type AudioRouteResolution,
  type AudioVoiceSelection,
  type AudioCloudBinding,
} from '../../../../shared/generatedAudioConfiguration';
import { offlineVoiceModels } from '../../../../shared/voiceModelCatalog';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { Card, Row } from '../rows';
import { ghostButtonStyle, inputStyle } from './ghostButton';

const EMPTY_NATIVE_AUDIO_SNAPSHOT = defaultNativeAudioSnapshot();
const MODEL_STATE_LABELS: Record<NativeAudioModelSnapshot['state']['type'], string> = {
  'not-installed': "audio_model_not_installed", queued: "voice_model_state_queued", downloading: "apps_status_downloading", verifying: "voice_model_state_verifying", extracting: "voice_model_state_installing", ready: "audio_ready", failed: "voice_model_state_failed",
};
const SOURCE_OPTIONS = [
  { value: 'automatic', label: "composer_context_policy_auto" },
  { value: 'system', label: "audio_source_system" },
  { value: 'offline', label: "audio_settings_offline_models" },
  { value: 'provider', label: "audio_settings_cloud_source" },
] as const;

export interface VoicePageModel {
  displayConfiguration: AudioConfigurationV4;
  recognitionRoute: AudioRouteResolution;
  speechRoute: AudioRouteResolution;
  actualRecognition: string;
  actualSpeech: string;
  voiceOptions: NativeAudioVoiceOption[];
  models: Array<NativeAudioModelSnapshot & { displayName: string; kind: 'stt' | 'tts'; approxSizeBytes: number }>;
  microphonePermission: NativeAudioSnapshot['permissions']['microphone'];
  speechPermission: SpeechPermissionStatus;
  notices: string[];
}

function modelSnapshot(snapshot: NativeAudioSnapshot, modelId: string): NativeAudioModelSnapshot {
  return snapshot.models.find((model) => model.modelId === modelId) ?? { modelId, state: { type: 'not-installed' } };
}

function audioModels(snapshot: NativeAudioSnapshot): AudioOfflineModelAvailability[] {
  return offlineVoiceModels.map((model) => ({
    id: model.id,
    kind: model.kind === 'stt' ? 'recognition' : 'speech',
    languages: model.languages,
    installed: modelSnapshot(snapshot, model.id).state.type === 'ready',
    ...(model.kind === 'tts' ? { voiceIds: model.voices.map((voice) => voice.id) } : {}),
  }));
}

function readiness(snapshot: NativeAudioSnapshot, kind: 'recognition' | 'speech') {
  if (kind === 'speech') return snapshot.voices.some((voice) => voice.source === 'system') ? 'available' as const : 'unavailable' as const;
  if (snapshot.permissions.speech === 'denied' || snapshot.permissions.speech === 'restricted') return 'denied' as const;
  if (snapshot.permissions.speech === 'not_determined') return 'permissionRequired' as const;
  if (snapshot.permissions.speech === 'authorized' && snapshot.recognizerAvailable) return 'available' as const;
  return 'unavailable' as const;
}

function actualRecognition(snapshot: NativeAudioSnapshot): string {
  if (snapshot.executedRoute?.kind === 'recognition') return `${tr('desktop_voice_cloud_recognition')} · ${snapshot.executedRoute.profileId} · ${snapshot.executedRoute.modelId}`;
  const backend = snapshot.recognition?.effectiveBackend;
  if (!backend || backend === 'unavailable') return tr("desktop_voice_no_recognition");
  return `${backend === 'apple' ? tr("desktop_voice_system_recognition") : tr("desktop_voice_offline_recognition")}${snapshot.recognition?.effectiveLanguage ? ` · ${snapshot.recognition.effectiveLanguage}` : ''}`;
}

function actualSpeech(snapshot: NativeAudioSnapshot): string {
  if (snapshot.executedRoute?.kind === 'speech') return `${tr('desktop_voice_cloud_speech')} · ${snapshot.executedRoute.profileId} · ${snapshot.executedRoute.modelId}${snapshot.executedRoute.voiceId ? ` · ${snapshot.executedRoute.voiceId}` : ''}`;
  if (snapshot.activity !== 'speaking') return tr("desktop_voice_no_playback");
  return `${snapshot.playback?.effectiveVoiceLabel ?? snapshot.playback?.effectiveVoiceId ?? tr("desktop_voice_playing")}`;
}

function permissionLabel(status: NativeAudioSnapshot['permissions']['microphone'] | SpeechPermissionStatus): string {
  switch (status) {
    case 'granted':
    case 'authorized': return tr("voice_permission_authorized_label");
    case 'denied': return tr("voice_permission_denied_label");
    case 'prompt':
    case 'not_determined': return tr("voice_permission_pending_label");
    case 'restricted': return tr("voice_permission_restricted_label");
    case 'unavailable': return tr("voice_permission_unknown_label");
  }
}

function operationLabel(kind: 'recognition' | 'speech' | 'conversation'): string {
  return tr(kind === 'recognition' ? 'desktop_voice_recognition' : kind === 'speech' ? 'audio_settings_output' : 'audio_conversation_section');
}

function systemVoiceOptionId(voice: NativeAudioVoiceOption): string {
  return voice.id.replace(/^system:/, '');
}

export function voicePageModel(config: AudioConfigurationV4, snapshot: NativeAudioSnapshot): VoicePageModel {
  const displayConfiguration = config;
  const language = resolveAudioLanguage(displayConfiguration.language, snapshot.localeTag ?? 'en-US');
  const offlineModels = audioModels(snapshot);
  const systemVoiceIds = snapshot.voices.filter((voice) => voice.source === 'system').map((voice) => voice.id.replace(/^system:/, ''));
  const recognitionRoute = resolveAudioRoute({
    kind: 'recognition', preference: displayConfiguration.recognition, language,
    systemStatus: readiness(snapshot, 'recognition'), offlineModels, sessionContext: snapshot.sessionContext, providerCapabilities: snapshot.providerCapabilities,
  });
  const speechRoute = resolveAudioRoute({
    kind: 'speech', preference: displayConfiguration.speech, language,
    systemStatus: readiness(snapshot, 'speech'), offlineModels, systemVoiceIds, sessionContext: snapshot.sessionContext, providerCapabilities: snapshot.providerCapabilities,
  });
  const notices: string[] = [];
  if (snapshot.helper.state === 'failed') notices.push(snapshot.helper.message ?? tr("desktop_voice_helper_failed"));
  if (recognitionRoute.status === 'unavailable' || recognitionRoute.status === 'invalidRequest') notices.push(tr('desktop_voice_recognition_unavailable', recognitionRoute.reason));
  if (speechRoute.status === 'unavailable' || speechRoute.status === 'invalidRequest') notices.push(tr('desktop_voice_speech_unavailable', speechRoute.reason));
  return {
    displayConfiguration,
    recognitionRoute,
    speechRoute,
    actualRecognition: actualRecognition(snapshot),
    actualSpeech: actualSpeech(snapshot),
    voiceOptions: snapshot.voices,
    models: offlineVoiceModels.map((entry) => ({
      ...modelSnapshot(snapshot, entry.id),
      displayName: localizedModelName(entry.id, entry.displayName),
      kind: entry.kind,
      approxSizeBytes: entry.approxSizeBytes,
    })),
    microphonePermission: snapshot.permissions.microphone,
    speechPermission: snapshot.permissions.speech,
    notices,
  };
}

function localizedModelName(id: string, names: Record<string, string>): string {
  const keys: Record<string, string> = {
    'sherpa.zipformer-zh-14m-mobile': 'voice_model_name_zipformer_zh_14m',
    'sherpa.moonshine-tiny-en': 'voice_model_name_moonshine_tiny_en',
    'sherpa.melo-zh-en': 'voice_model_name_melo_zh_en',
    'sherpa.kitten-nano-en': 'voice_model_name_kitten_nano_en',
  };
  return keys[id] ? tr(keys[id]) : names[currentLocale()] ?? names.en ?? id;
}

export function voicePreviewRequest(configuration: AudioConfigurationV4, deviceLocale?: string): {
  operation: Extract<AudioOperationDto, { type: 'speak' }>;
  configuration: AudioConfigurationV4;
} {
  const voice = configuration.speech.voice;
  const voiceOverride = voice?.source === 'offline'
    ? `sherpa:${voice.modelId ?? configuration.speech.offlineModelId ?? ''}:${voice.id}`
    : voice?.source === 'system' ? `system:${voice.id}` : undefined;
  const language = resolveAudioLanguage(configuration.language, deviceLocale ?? 'en-US');
  return {
    operation: {
      type: 'speak',
      text: translate('desktop_voice_preview_text', matchLocale(language) ?? 'en'),
      language,
      rate: configuration.rate,
      ...(voiceOverride ? { voice: voiceOverride } : {}),
    },
    configuration: structuredClone(configuration),
  };
}

function formatBytes(value: number): string {
  if (value < 1024 * 1024) return `${Math.max(1, Math.round(value / 1024))} KB`;
  return `${(value / (1024 * 1024)).toFixed(value >= 100 * 1024 * 1024 ? 0 : 1)} MB`;
}

function modelProgress(model: NativeAudioModelSnapshot): string | null {
  if (model.state.type !== 'downloading') return null;
  if (model.state.totalBytes <= 0) return formatBytes(model.state.receivedBytes);
  return `${Math.min(100, Math.round((model.state.receivedBytes / model.state.totalBytes) * 100))}%`;
}

export function voiceSelectionOptionValue(voice: AudioVoiceSelection | null, fallbackModelId?: string | null): string {
  if (!voice) return 'automatic';
  if (voice.source === 'provider') return `provider:${voice.profileId ?? ''}:${voice.modelId ?? ''}:${voice.id}`;
  if (voice.source === 'system') return `system:${voice.id.replace(/^system:/, '')}`;
  return `sherpa:${voice.modelId ?? fallbackModelId ?? ''}:${voice.id}`;
}

export function voiceSelectionFromValue(value: string, voices: NativeAudioVoiceOption[]): AudioVoiceSelection | null {
  if (value === 'automatic') return null;
  if (value.startsWith('system:')) return { source: 'system', id: value.slice('system:'.length) };
  const selected = voices.find((voice) => voice.id === value && voice.source === 'sherpa');
  if (!selected) return null;
  const prefix = `sherpa:${selected.familyId}:`;
  return { source: 'offline', modelId: selected.familyId, id: selected.id.startsWith(prefix) ? selected.id.slice(prefix.length) : selected.id };
}

export async function saveVoiceDraftIfUnchanged(
  configuration: AudioConfigurationV4,
  editRevisionAtSave: number,
  currentEditRevision: () => number,
  persist: (configuration: AudioConfigurationV4) => Promise<unknown>,
): Promise<boolean> {
  await persist(configuration);
  return currentEditRevision() === editRevisionAtSave;
}

export function Voice({ bridge }: PageContentProps) {
  const { locale } = useI18n();
  const t = useT();
  const saved = bridge.bootstrap?.settings?.voice ?? audioConfigurationDefaults();
  const snapshotFromBridge = bridge.audioSnapshot ?? EMPTY_NATIVE_AUDIO_SNAPSHOT;
  const [snapshot, setSnapshot] = useState(snapshotFromBridge);
  const [draft, setDraft] = useState<AudioConfigurationV4>(saved);
  const draftEditRevision = useRef(0);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const model = useMemo(() => voicePageModel(draft, snapshot), [draft, snapshot, locale]);
  const displayDraft = model.displayConfiguration;
  const audioConfigurationError = bridge.bootstrap?.settings?.audioConfigurationError;

  useEffect(() => setSnapshot(snapshotFromBridge), [snapshotFromBridge]);
  useEffect(() => {
    if (!dirty) setDraft(saved);
  }, [saved, dirty]);
  useEffect(() => {
    let cancelled = false;
    let sequence = 0;
    const refresh = () => {
      const current = ++sequence;
      if (typeof bridge.audioRequest !== 'function') return;
      void bridge.audioRequest({ type: 'get_snapshot', configuration: draft }).then((response) => {
        if (!cancelled && current === sequence) setSnapshot(response.snapshot);
      }).catch((error) => {
        if (!cancelled && current === sequence) setOperationError(error instanceof Error ? error.message : tr("desktop_voice_read_failed"));
      });
    };
    refresh();
    window.addEventListener('focus', refresh);
    return () => { cancelled = true; sequence += 1; window.removeEventListener('focus', refresh); };
  }, [bridge.audioRequest, bridge.bootstrap?.activeSession?.sessionId, draft]);

  const updateDraft = (change: (previous: AudioConfigurationV4) => AudioConfigurationV4) => {
    draftEditRevision.current += 1;
    setDraft((previous) => change(previous));
    setDirty(true);
    setSaveError(null);
  };
  const save = async () => {
    const saveEditRevision = draftEditRevision.current;
    setSaving(true);
    setSaveError(null);
    try {
      const unchanged = await saveVoiceDraftIfUnchanged(
        displayDraft,
        saveEditRevision,
        () => draftEditRevision.current,
        (configuration) => bridge.setVoicePreferences(configuration, bridge.bootstrap?.settings?.voiceRevision ?? 0),
      );
      if (unchanged) setDirty(false);
    } catch (cause) {
      setSaveError(cause instanceof Error ? cause.message : tr("desktop_voice_save_failed"));
    } finally {
      setSaving(false);
    }
  };
  const perform = async (
    operation: Parameters<NonNullable<typeof bridge.audioExecute>>[0],
    configurationOverride?: AudioConfigurationV4,
  ) => {
    setOperationError(null);
    if (typeof bridge.audioExecute !== 'function') {
      setOperationError(tr("desktop_voice_service_unavailable"));
      return;
    }
    try {
      const response = await bridge.audioExecute(operation, undefined, configurationOverride);
      setSnapshot(response.snapshot);
      if (response.result.type === 'failed' || response.result.type === 'failed_without_resource') setOperationError(`${response.result.error.kind}: ${response.result.error.message}`);
    } catch (cause) {
      setOperationError(cause instanceof Error ? cause.message : tr("desktop_voice_operation_failed"));
    }
  };
  const previewVoice = () => {
    const request = voicePreviewRequest(displayDraft, snapshot.localeTag);
    void perform(request.operation, request.configuration);
  };
  const requestPermissions = () => {
    if (typeof bridge.audioRequest !== 'function') return;
    void bridge.audioRequest({ type: 'request_authorization', permissions: ['microphone', 'speech'] })
      .then((response) => setSnapshot(response.snapshot))
      .catch((cause) => setOperationError(cause instanceof Error ? cause.message : tr("desktop_voice_permission_failed")));
  };
  const changeModel = (entry: VoicePageModel['models'][number]) => {
    if (typeof bridge.audioRequest !== 'function') return;
    const busy = ['queued', 'downloading', 'verifying', 'extracting'].includes(entry.state.type);
    const command = entry.state.type === 'ready'
      ? { type: 'remove_model', modelId: entry.modelId } as const
      : busy ? { type: 'cancel_model', modelId: entry.modelId } as const
        : { type: 'install_model', modelId: entry.modelId } as const;
    setOperationError(null);
    void bridge.audioRequest(command).then((response) => {
      setSnapshot(response.snapshot);
      if (response.type === 'error') setOperationError(response.error.message);
    }).catch((cause) => setOperationError(cause instanceof Error ? cause.message : tr("desktop_voice_model_failed")));
  };

  const cloudFields = (kind: 'recognition' | 'speech' | 'conversation', cloud: AudioCloudBinding) => {
    const changeCloud = (patch: Partial<AudioCloudBinding>) => updateDraft((current) => ({
      ...current, [kind]: { ...current[kind], cloud: { ...current[kind].cloud, ...patch }, ...(kind !== 'recognition' ? { voice: null } : {}) },
    }));
    const resolvedProfile = cloud.binding === 'follow_session' ? snapshot.sessionContext?.profileId : cloud.profileId;
    const capability = kind === 'conversation' ? snapshot.realtimeReadiness : snapshot.providerCapabilities?.find((entry) => entry.profileId === resolvedProfile && entry.kind === (kind === 'recognition' ? 'recognition' : 'speech'));
    const capabilityReadiness = capability && 'readiness' in capability ? capability.readiness : snapshot.realtimeReadiness?.ready ? 'ready' : tr("audio_configuration_required");
    const effectiveModel = cloud.modelId ?? capability?.defaultModelId;
    const selectableModels = (capability?.modelIds ?? []).filter((id): id is string => typeof id === 'string');
    const models = kind === 'conversation' ? snapshot.realtimeReadiness?.models : snapshot.providerCatalog?.find((entry) => entry.profileId === resolvedProfile && entry.kind === kind)?.models;
    const voices = models?.find((entry) => entry.id === (effectiveModel ?? null))?.voices ?? [];
    const selectedVoice = kind === 'conversation' ? displayDraft.conversation.voice : kind === 'speech' ? displayDraft.speech.voice : null;
    const voiceId = selectedVoice?.source === 'provider' ? selectedVoice.id : '';
    const voiceSelect = <select aria-label={kind === 'conversation' ? tr("desktop_voice_realtime_voice") : tr("desktop_voice_cloud_voice")} value={voiceId} onChange={(event) => {
      const id = event.target.value;
      if (kind === 'recognition') return;
      updateDraft((current) => ({ ...current, [kind]: { ...current[kind], voice: id && resolvedProfile ? { source: 'provider', id, profileId: resolvedProfile, modelId: effectiveModel ?? null } : null } }));
    }} style={inputStyle(t)}><option value="">{tr("desktop_voice_provider_voice")}</option>{voices.map((voice) => <option key={voice.id} value={voice.id}>{voice.label ?? voice.id}</option>)}{voiceId && !voices.some((voice) => voice.id === voiceId) && <option value={voiceId}>{voiceId}{tr("desktop_voice_check_suffix")}</option>}</select>;
    return <>
      <Row title={tr("desktop_voice_account_selection")} desc={cloud.binding === 'follow_session' ? `${tr('audio_follow_session')} · ${resolvedProfile ?? tr("desktop_voice_no_account")}` : tr("desktop_voice_explicit_account_hint")} align="center">
        <select aria-label={tr('desktop_voice_account_a11y', operationLabel(kind))} value={cloud.binding} onChange={(event) => changeCloud({ binding: event.target.value })} style={inputStyle(t)}><option value="follow_session">{tr('audio_follow_session')}</option><option value="explicit_profile">{tr("desktop_voice_explicit_account")}</option></select>
      </Row>
      {cloud.binding === 'explicit_profile' && <Row title={tr('audio_settings_account')} align="center">
        <select aria-label={tr('desktop_voice_account_a11y', operationLabel(kind))} value={cloud.profileId ?? ''} onChange={(event) => changeCloud({ profileId: event.target.value || null, modelId: null })} style={inputStyle(t)}>
          <option value="">{tr('audio_settings_select_account')}</option>
          {[...new Set([...(bridge.bootstrap?.providerCredentials?.filter((entry) => entry.configured).map((entry) => entry.providerId) ?? []), ...(snapshot.providerCatalog?.map((entry) => entry.profileId) ?? []), ...(cloud.profileId ? [cloud.profileId] : [])])].map((id) => <option key={id} value={id}>{({ openai: 'OpenAI', 'openai-chatgpt': 'Codex Auth', deepseek: 'DeepSeek', kimi: 'Kimi', openrouter: 'OpenRouter', zai: 'Z.ai' } as Record<string, string>)[id] ?? id}</option>)}
        </select>
      </Row>}
      <Row title={tr('audio_model')} desc={capabilityReadiness === 'ready' ? undefined : tr("desktop_voice_check_capability")} align="center">
        <select aria-label={tr('desktop_voice_model_a11y', operationLabel(kind))} value={cloud.modelId ?? ''} onChange={(event) => changeCloud({ modelId: event.target.value || null })} style={inputStyle(t)}>
          <option value="">{capability?.modelIds?.includes(null) ? tr("desktop_voice_native_model") : tr("desktop_voice_provider_default")}{capability?.defaultModelId ? ` · ${capability.defaultModelId}` : ''}</option>
          {selectableModels.map((id) => <option key={id} value={id}>{models?.find((model) => model.id === id)?.label ?? id}</option>)}
          {cloud.modelId && !capability?.modelIds?.includes(cloud.modelId) && <option value={cloud.modelId}>{cloud.modelId}{tr("desktop_voice_check_suffix")}</option>}
        </select>
      </Row>
      {kind === 'conversation' && <Row title={tr("desktop_voice_realtime_voice")} desc={tr("desktop_voice_realtime_voice_hint")} align="center">{voiceSelect}</Row>}
      {kind === 'speech' && <Row title={tr("desktop_voice_cloud_voice")} desc="" align="center"><div style={{ display: 'flex', gap: 10 }}>{voiceSelect}<button type="button" onClick={previewVoice} style={ghostButtonStyle(t)}>{tr("desktop_voice_preview")}</button></div></Row>}
    </>;
  };

  const recognitionModelValue = displayDraft.recognition.offlineModelId ?? 'language-default';
  const speechModelValue = displayDraft.speech.offlineModelId ?? 'language-default';
  const speechVoiceValue = voiceSelectionOptionValue(displayDraft.speech.voice, displayDraft.speech.offlineModelId);
  const systemVoices = model.voiceOptions.filter((voice) => displayDraft.speech.source !== 'offline' && voice.source === 'system');
  const offlineVoices = model.voiceOptions.filter((voice) => displayDraft.speech.source !== 'system' && voice.source === 'sherpa' && (!displayDraft.speech.offlineModelId || voice.familyId === displayDraft.speech.offlineModelId));
  const availableVoiceValues = new Set([
    ...systemVoices.map((voice) => `system:${systemVoiceOptionId(voice)}`),
    ...offlineVoices.map((voice) => voice.id),
  ]);
  const unresolvedVoiceValue = speechVoiceValue !== 'automatic' && !availableVoiceValues.has(speechVoiceValue)
    ? speechVoiceValue
    : null;

  const languageOptions = [...new Set(['auto', 'zh-CN', 'en-US', 'zh-TW', 'ja-JP', ...snapshot.voices.map((voice) => voice.languageTag), displayDraft.language])].filter(Boolean);
  const languageName = (value: string) => { try { return new Intl.DisplayNames([locale], { type: 'language' }).of(value) ?? value; } catch { return value; } };
  const sourceDescription = (route: AudioRouteResolution, operation: string) => {
    if (route.status !== 'ready') return tr("desktop_voice_source_unavailable");
    if (route.effective?.source === 'system') return tr('desktop_voice_system_source', operation);
    if (route.effective?.source === 'offline') return tr('desktop_voice_offline_source', model.models.find((entry) => entry.modelId === route.effective?.modelId)?.displayName ?? tr("audio_settings_offline_models"));
    return tr('desktop_voice_cloud_source', operation);
  };

  return (
    <>
      <Card title={tr('settings_section_voice_input')}>
        <Row title={tr('settings_language_title')} desc={tr("audio_language_hint")} align="center">
          <select aria-label={tr("settings_language_title")} value={displayDraft.language} onChange={(event) => updateDraft((current) => ({ ...current, language: event.target.value }))} style={inputStyle(t)}>
            {languageOptions.map((value) => <option key={value} value={value}>{value === 'auto' ? tr('onboarding_voice_language_system') : languageName(value)}</option>)}
          </select>
        </Row>

        <Row title={tr("desktop_voice_recognition_source")} desc={sourceDescription(model.recognitionRoute, tr("desktop_voice_recognition"))} align="center">
          <select aria-label={tr("desktop_voice_recognition_source")} value={displayDraft.recognition.source} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, source: event.target.value } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{tr(option.label)}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.recognition.source) && <option value={displayDraft.recognition.source}>{tr("desktop_voice_unknown_source")}</option>}
          </select>
        </Row>
        {displayDraft.recognition.source === 'offline' && (
        <Row title={tr('audio_offline_recognition_models')} desc={tr("audio_offline_recognition_hint")} align="center">
          <select aria-label={tr('audio_offline_recognition_models')} value={recognitionModelValue} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">{tr('audio_language_default_model')}</option>
            {model.models.filter((entry) => entry.kind === 'stt').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName} · {entry.state.type === 'ready' ? tr('apps_installed') : entry.state.type === 'not-installed' ? tr('settings_linux_not_installed') : tr("desktop_voice_model_pending")}</option>)}
            {displayDraft.recognition.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.recognition.offlineModelId) && <option value={displayDraft.recognition.offlineModelId}>{tr("desktop_voice_unknown_model")}</option>}
          </select>
        </Row>
        )}
        {displayDraft.recognition.source === 'provider' && cloudFields('recognition', displayDraft.recognition.cloud)}
      </Card>
      <Card title={tr('settings_section_voice_output')}>
        <Row title={tr("desktop_voice_speech_source")} desc={sourceDescription(model.speechRoute, tr("audio_settings_output"))} align="center">
          <select aria-label={tr("desktop_voice_speech_source")} value={displayDraft.speech.source} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, source: event.target.value, ...(event.target.value === 'automatic' ? { voice: null } : {}) } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{tr(option.label)}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.speech.source) && <option value={displayDraft.speech.source}>{tr("desktop_voice_unknown_source")}</option>}
          </select>
        </Row>
        {displayDraft.speech.source === 'offline' && (
        <Row title={tr('audio_offline_speech_models')} desc={tr("desktop_voice_offline_speech_hint")} align="center">
          <select aria-label={tr('audio_offline_speech_models')} value={speechModelValue} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">{tr('audio_language_default_model')}</option>
            {model.models.filter((entry) => entry.kind === 'tts').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName} · {entry.state.type === 'ready' ? tr('apps_installed') : entry.state.type === 'not-installed' ? tr('settings_linux_not_installed') : tr("desktop_voice_model_pending")}</option>)}
            {displayDraft.speech.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.speech.offlineModelId) && <option value={displayDraft.speech.offlineModelId}>{tr("desktop_voice_unknown_model")}</option>}
          </select>
        </Row>
        )}
        {displayDraft.speech.source === 'provider' && cloudFields('speech', displayDraft.speech.cloud)}
        {displayDraft.speech.source !== 'provider' && (
        <Row title={tr("desktop_voice_voice")} align="center">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <select aria-label={tr("desktop_voice_speech_voice")} value={speechVoiceValue} onChange={(event) => updateDraft((current) => {
              const voice = voiceSelectionFromValue(event.target.value, model.voiceOptions);
              if (!voice) return { ...current, speech: { ...current.speech, voice: null } };
              return { ...current, speech: { ...current.speech, source: voice.source, offlineModelId: voice.source === 'offline' ? voice.modelId ?? null : current.speech.offlineModelId, voice } };
            })} style={inputStyle(t)}>
              <option value="automatic">{tr("desktop_voice_automatic_voice")}</option>
              {unresolvedVoiceValue && <option value={unresolvedVoiceValue}>{tr("desktop_voice_unknown_voice")}{displayDraft.speech.voice?.id}</option>}
              {systemVoices.map((voice) => <option key={voice.id} value={`system:${voice.id.replace(/^system:/, '')}`}>{voice.label}</option>)}
              {offlineVoices.map((voice) => <option key={voice.id} value={voice.id}>{voice.label}{tr("desktop_voice_offline_suffix")}</option>)}
            </select>
            <button type="button" onClick={previewVoice} style={ghostButtonStyle(t)} aria-label={tr("desktop_voice_preview")}>{tr('voice_preview')}</button>
          </div>
        </Row>
        )}
        {displayDraft.speech.source === 'provider' ? (
          <Row title={tr('voice_speech_rate')} desc={displayDraft.rate === 1 ? tr("desktop_voice_cloud_rate") : tr('desktop_voice_cloud_rate_saved', displayDraft.rate.toFixed(2))} align="center">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span style={{ fontSize: 12.5, color: t.text3 }}>1.0×</span>
              {displayDraft.rate !== 1 && <button type="button" onClick={() => updateDraft((current) => ({ ...current, rate: 1 }))} style={ghostButtonStyle(t)}>{tr("audio_cloud_rate_reset")}</button>}
            </div>
          </Row>
        ) : (
          <Row title={tr('voice_speech_rate')} desc="0.5× – 2.0×" align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><input type="range" min={0.5} max={2} step={0.05} value={displayDraft.rate} onChange={(event) => updateDraft((current) => ({ ...current, rate: Number(event.target.value) }))} aria-label={tr("desktop_voice_speech_rate")} /><span style={{ fontSize: 12.5, color: t.text3, minWidth: 40 }}>{displayDraft.rate.toFixed(2)}×</span></div></Row>
        )}
        <Row title={tr("desktop_voice_autoplay")} desc={tr("desktop_voice_autoplay_hint")} align="center"><label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 12.5, color: t.text2 }}><input type="checkbox" checked={displayDraft.autoPlayReplies} onChange={(event) => updateDraft((current) => ({ ...current, autoPlayReplies: event.target.checked }))} aria-label={tr("desktop_voice_autoplay")} />{displayDraft.autoPlayReplies ? tr("settings_status_on") : tr("settings_status_off")}</label></Row>
      </Card>

      {(audioConfigurationError || saveError || operationError || model.notices.length > 0) && <div role="status" aria-live="polite" style={{ padding: '0 0 16px', color: t.danger, fontSize: 12.5, lineHeight: 1.5 }}>
        {audioConfigurationError || saveError || (operationError?.includes('429') ? tr("desktop_voice_quota") : operationError ? tr("desktop_voice_failed_hint") : tr("desktop_voice_unavailable_hint"))}
      </div>}
      <Card>        <Row title={tr("desktop_voice_save")} desc={dirty ? tr("desktop_voice_unsaved") : tr("desktop_voice_saved_locally")} align="center"><button type="button" disabled={!dirty || saving} onClick={() => void save()} style={ghostButtonStyle(t, !dirty || saving)}>{saving ? tr('audio_settings_saving') : tr('settings_save')}</button></Row>
      </Card>
      <VoiceSettingsDisclosure title={tr('audio_settings_offline_models')} detail={tr('desktop_voice_installed_count', model.models.filter((entry) => entry.state.type === 'ready').length)} initiallyOpen={displayDraft.recognition.source === 'offline' || displayDraft.speech.source === 'offline' || model.models.some((entry) => ['downloading', 'failed'].includes(entry.state.type))}>
      <Card>
        <div style={{ padding: "12px 18px", color: t.text3, fontSize: 12 }}>{tr("audio_offline_models_hint")}</div>
        {model.models.map((entry) => {
          const busy = ['queued', 'downloading', 'verifying', 'extracting'].includes(entry.state.type);
          const action = entry.state.type === 'ready' ? tr("common_delete") : busy ? tr("common_cancel") : entry.state.type === 'failed' ? tr("common_retry") : tr("voice_offline_download");
          return <Row key={entry.modelId} title={entry.displayName} desc={`${entry.kind === 'stt' ? tr("desktop_voice_recognition") : tr("audio_settings_output")} · ${formatBytes(entry.approxSizeBytes)}${entry.state.type === 'failed' ? ` · ${entry.state.message}` : ''}`} align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><span style={{ fontSize: 12, color: entry.state.type === 'failed' ? t.danger : t.text3 }}>{tr(MODEL_STATE_LABELS[entry.state.type])}{modelProgress(entry) ? ` ${modelProgress(entry)}` : ''}</span><button type="button" onClick={() => changeModel(entry)} style={ghostButtonStyle(t, false, entry.state.type === 'ready')}>{action}</button></div></Row>;
        })}
      </Card>

      </VoiceSettingsDisclosure>
      <VoiceSettingsDisclosure title={tr('audio_settings_advanced')} initiallyOpen={snapshot.permissions.microphone !== 'granted'}>
      <Card title={tr("audio_conversation_section")}>
        <Row title={tr("desktop_voice_conversation_mode")} desc={displayDraft.conversation.mode === 'realtime' ? snapshot.realtimeReadiness?.reason ?? tr("desktop_voice_realtime_host_unavailable") : tr("desktop_voice_agent_conversation_hint")} align="center">
          <select aria-label={tr("desktop_voice_voice_mode")} value={displayDraft.conversation.mode} onChange={(event) => updateDraft((current) => ({ ...current, conversation: { ...current.conversation, mode: event.target.value } }))} style={inputStyle(t)}>
            <option value="agent">{tr("audio_agent_conversation")}</option>
            <option value="realtime">{tr("audio_realtime_conversation")}{snapshot.realtimeReadiness?.ready ? '' : tr("desktop_voice_realtime_suffix")}</option>
          </select>
        </Row>
        {displayDraft.conversation.mode === 'realtime' && <><Row title={tr("desktop_voice_interaction")} desc={tr("desktop_voice_turn_hint")} align="center"><select aria-label={tr("desktop_voice_realtime_interaction")} value={displayDraft.conversation.interaction} onChange={(event) => updateDraft((current) => ({ ...current, conversation: { ...current.conversation, interaction: event.target.value } }))} style={inputStyle(t)}><option value="turn_based">{tr("audio_realtime_turn_based")}</option>{displayDraft.conversation.interaction !== 'turn_based' && <option value={displayDraft.conversation.interaction}>{tr("desktop_voice_interrupt_unavailable")}</option>}</select></Row>{cloudFields('conversation', displayDraft.conversation.cloud)}</>}
      </Card>
      <Card title={tr('settings_parity_permissions')}>
        <Row title={tr('voice_microphone')} desc={tr('desktop_voice_status', permissionLabel(model.microphonePermission))} align="center">{model.microphonePermission === 'denied' ? <button type="button" onClick={() => void bridge.openSystemSettings('microphone')} style={ghostButtonStyle(t)}>{tr("desktop_voice_open_settings")}</button> : model.microphonePermission === 'prompt' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>{tr("desktop_voice_request_permission")}</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.microphonePermission)}</span>}</Row>
        <Row title={tr("desktop_voice_system_speech")} desc={tr('desktop_voice_status', permissionLabel(model.speechPermission))} align="center">{model.speechPermission === 'denied' || model.speechPermission === 'restricted' ? <button type="button" onClick={() => void bridge.openSystemSettings('speech_recognition')} style={ghostButtonStyle(t)}>{tr("desktop_voice_open_settings")}</button> : model.speechPermission === 'not_determined' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>{tr("desktop_voice_request_permission")}</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.speechPermission)}</span>}</Row>
        <Row title={tr("desktop_voice_native_service")} desc={snapshot.helper.message ?? tr("desktop_voice_local_processing")} align="center"><span style={{ color: t.text3, fontSize: 12.5 }}>{snapshot.helper.state === "running" ? tr('settings_status_available') : tr("desktop_voice_check")}</span></Row>
      </Card>
        <Card title={tr("desktop_voice_actual")}>
          <Row title={tr("desktop_voice_recognition")} desc={model.actualRecognition}>{null}</Row>
          <Row title={tr('audio_settings_output')} desc={model.actualSpeech}>{null}</Row>
          <Row title={tr("desktop_voice_custom_language")} desc={tr("desktop_voice_language_hint")} align="center"><input aria-label={tr("desktop_voice_custom_voice_language")} value={displayDraft.language} onChange={(event) => updateDraft((current) => ({ ...current, language: event.target.value.trim() || AUDIO_LANGUAGE_AUTO }))} style={inputStyle(t)} /></Row>
        </Card>
      </VoiceSettingsDisclosure>
    </>
  );
}

function VoiceSettingsDisclosure({ title, detail, initiallyOpen, children }: { title: string; detail?: string; initiallyOpen?: boolean; children: ReactNode }) {
  const t = useT();
  const [expanded, setExpanded] = useState(Boolean(initiallyOpen));
  useEffect(() => { if (initiallyOpen) setExpanded(true); }, [initiallyOpen]);
  return <details open={expanded} onToggle={(event) => setExpanded(event.currentTarget.open)} style={{ marginBottom: 20 }}>
    <summary style={{ cursor: 'pointer', padding: '12px 0', color: t.text2, fontSize: 13, fontWeight: 600 }}>
      {title}{detail && <span style={{ marginLeft: 10, color: t.text3, fontWeight: 400 }}>{detail}</span>}
    </summary>
    {children}
  </details>;
}
