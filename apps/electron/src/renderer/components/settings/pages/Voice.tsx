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
  'not-installed': '未安装', queued: '等待下载', downloading: '下载中', verifying: '正在校验', extracting: '正在安装', ready: '可用', failed: '失败',
};
const SOURCE_OPTIONS = [
  { value: 'automatic', label: '自动' },
  { value: 'system', label: '系统' },
  { value: 'offline', label: '离线模型' },
  { value: 'provider', label: '云端服务商' },
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
  if (snapshot.executedRoute?.kind === 'recognition') return `云端识别 · ${snapshot.executedRoute.profileId} · ${snapshot.executedRoute.modelId}`;
  const backend = snapshot.recognition?.effectiveBackend;
  if (!backend || backend === 'unavailable') return '暂无正在执行的识别操作';
  return `${backend === 'apple' ? 'macOS 系统识别' : 'Sherpa 离线识别'}${snapshot.recognition?.effectiveLanguage ? ` · ${snapshot.recognition.effectiveLanguage}` : ''}`;
}

function actualSpeech(snapshot: NativeAudioSnapshot): string {
  if (snapshot.executedRoute?.kind === 'speech') return `云端朗读 · ${snapshot.executedRoute.profileId} · ${snapshot.executedRoute.modelId}${snapshot.executedRoute.voiceId ? ` · ${snapshot.executedRoute.voiceId}` : ''}`;
  if (snapshot.activity !== 'speaking') return '暂无正在执行的播放操作';
  return `${snapshot.playback?.effectiveVoiceLabel ?? snapshot.playback?.effectiveVoiceId ?? '正在播放'}`;
}

function permissionLabel(status: NativeAudioSnapshot['permissions']['microphone'] | SpeechPermissionStatus): string {
  switch (status) {
    case 'granted':
    case 'authorized': return '已授权';
    case 'denied': return '未授权';
    case 'prompt':
    case 'not_determined': return '尚未询问';
    case 'restricted': return '受限';
    case 'unavailable': return '无法确定';
  }
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
  if (snapshot.helper.state === 'failed') notices.push(snapshot.helper.message ?? '原生音频服务启动失败。');
  if (recognitionRoute.status === 'unavailable' || recognitionRoute.status === 'invalidRequest') notices.push(`识别偏好不可用：${recognitionRoute.reason}`);
  if (speechRoute.status === 'unavailable' || speechRoute.status === 'invalidRequest') notices.push(`朗读偏好不可用：${speechRoute.reason}`);
  return {
    displayConfiguration,
    recognitionRoute,
    speechRoute,
    actualRecognition: actualRecognition(snapshot),
    actualSpeech: actualSpeech(snapshot),
    voiceOptions: snapshot.voices,
    models: offlineVoiceModels.map((entry) => ({
      ...modelSnapshot(snapshot, entry.id),
      displayName: entry.displayName.zh ?? entry.displayName.en ?? entry.id,
      kind: entry.kind,
      approxSizeBytes: entry.approxSizeBytes,
    })),
    microphonePermission: snapshot.permissions.microphone,
    speechPermission: snapshot.permissions.speech,
    notices,
  };
}

export function voicePreviewRequest(configuration: AudioConfigurationV4, deviceLocale?: string): {
  operation: Extract<AudioOperationDto, { type: 'speak' }>;
  configuration: AudioConfigurationV4;
} {
  const voice = configuration.speech.voice;
  const voiceOverride = voice?.source === 'offline'
    ? `sherpa:${voice.modelId ?? configuration.speech.offlineModelId ?? ''}:${voice.id}`
    : voice?.source === 'system' ? `system:${voice.id}` : undefined;
  return {
    operation: {
      type: 'speak',
      text: '这是 LingXi Desktop 的语音试听。This is a LingXi Desktop voice preview.',
      language: resolveAudioLanguage(configuration.language, deviceLocale ?? 'en-US'),
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
  const model = useMemo(() => voicePageModel(draft, snapshot), [draft, snapshot]);
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
        if (!cancelled && current === sequence) setOperationError(error instanceof Error ? error.message : '读取音频状态失败。');
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
      setSaveError(cause instanceof Error ? cause.message : '保存语音设置失败。');
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
      setOperationError('原生音频服务不可用。');
      return;
    }
    try {
      const response = await bridge.audioExecute(operation, undefined, configurationOverride);
      setSnapshot(response.snapshot);
      if (response.result.type === 'failed') setOperationError(`${response.result.error.kind}: ${response.result.error.message}`);
    } catch (cause) {
      setOperationError(cause instanceof Error ? cause.message : '原生音频操作失败。');
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
      .catch((cause) => setOperationError(cause instanceof Error ? cause.message : '请求权限失败。'));
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
    }).catch((cause) => setOperationError(cause instanceof Error ? cause.message : '离线模型操作失败。'));
  };

  const cloudFields = (kind: 'recognition' | 'speech' | 'conversation', cloud: AudioCloudBinding) => {
    const changeCloud = (patch: Partial<AudioCloudBinding>) => updateDraft((current) => ({
      ...current, [kind]: { ...current[kind], cloud: { ...current[kind].cloud, ...patch }, ...(kind !== 'recognition' ? { voice: null } : {}) },
    }));
    const resolvedProfile = cloud.binding === 'follow_session' ? snapshot.sessionContext?.profileId : cloud.profileId;
    const capability = kind === 'conversation' ? snapshot.realtimeReadiness : snapshot.providerCapabilities?.find((entry) => entry.profileId === resolvedProfile && entry.kind === (kind === 'recognition' ? 'recognition' : 'speech'));
    const capabilityReadiness = capability && 'readiness' in capability ? capability.readiness : snapshot.realtimeReadiness?.ready ? 'ready' : '需要配置';
    const effectiveModel = cloud.modelId ?? capability?.defaultModelId;
    const selectableModels = (capability?.modelIds ?? []).filter((id): id is string => typeof id === 'string');
    const models = kind === 'conversation' ? snapshot.realtimeReadiness?.models : snapshot.providerCatalog?.find((entry) => entry.profileId === resolvedProfile && entry.kind === kind)?.models;
    const voices = models?.find((entry) => entry.id === (effectiveModel ?? null))?.voices ?? [];
    const selectedVoice = kind === 'conversation' ? displayDraft.conversation.voice : kind === 'speech' ? displayDraft.speech.voice : null;
    const voiceId = selectedVoice?.source === 'provider' ? selectedVoice.id : '';
    const voiceSelect = <select aria-label={kind === 'conversation' ? '实时会话音色' : '云端音色'} value={voiceId} onChange={(event) => {
      const id = event.target.value;
      if (kind === 'recognition') return;
      updateDraft((current) => ({ ...current, [kind]: { ...current[kind], voice: id && resolvedProfile ? { source: 'provider', id, profileId: resolvedProfile, modelId: effectiveModel ?? null } : null } }));
    }} style={inputStyle(t)}><option value="">服务商默认音色</option>{voices.map((voice) => <option key={voice.id} value={voice.id}>{voice.label ?? voice.id}</option>)}{voiceId && !voices.some((voice) => voice.id === voiceId) && <option value={voiceId}>{voiceId}（需要检查）</option>}</select>;
    return <>
      <Row title="账号选择" desc={cloud.binding === 'follow_session' ? `跟随当前会话 · ${resolvedProfile ?? '尚未选择账号'}` : '仅此语音功能使用指定账号。'} align="center">
        <select aria-label={`${kind} 账号选择`} value={cloud.binding} onChange={(event) => changeCloud({ binding: event.target.value })} style={inputStyle(t)}><option value="follow_session">跟随当前会话</option><option value="explicit_profile">指定账号</option></select>
      </Row>
      {cloud.binding === 'explicit_profile' && <Row title="语音账号" align="center">
        <select aria-label={`${kind} profile`} value={cloud.profileId ?? ''} onChange={(event) => changeCloud({ profileId: event.target.value || null, modelId: null })} style={inputStyle(t)}>
          <option value="">选择账号</option>
          {[...new Set([...(bridge.bootstrap?.providerCredentials?.filter((entry) => entry.configured).map((entry) => entry.providerId) ?? []), ...(snapshot.providerCatalog?.map((entry) => entry.profileId) ?? []), ...(cloud.profileId ? [cloud.profileId] : [])])].map((id) => <option key={id} value={id}>{({ openai: 'OpenAI', 'openai-chatgpt': 'Codex Auth', deepseek: 'DeepSeek', kimi: 'Kimi', openrouter: 'OpenRouter', zai: 'Z.ai' } as Record<string, string>)[id] ?? id}</option>)}
        </select>
      </Row>}
      <Row title="音频模型" desc={capabilityReadiness === 'ready' ? undefined : '请检查账号和模型是否支持此语音功能。'} align="center">
        <select aria-label={`${kind} 音频模型`} value={cloud.modelId ?? ''} onChange={(event) => changeCloud({ modelId: event.target.value || null })} style={inputStyle(t)}>
          <option value="">{capability?.modelIds?.includes(null) ? '原生服务（无模型选择）' : '服务商默认'}{capability?.defaultModelId ? ` · ${capability.defaultModelId}` : ''}</option>
          {selectableModels.map((id) => <option key={id} value={id}>{models?.find((model) => model.id === id)?.label ?? id}</option>)}
          {cloud.modelId && !capability?.modelIds?.includes(cloud.modelId) && <option value={cloud.modelId}>{cloud.modelId}（需要检查）</option>}
        </select>
      </Row>
      {kind === 'conversation' && <Row title="实时会话音色" desc="选择实时会话使用的音色。" align="center">{voiceSelect}</Row>}
      {kind === 'speech' && <Row title="云端音色" desc="" align="center"><div style={{ display: 'flex', gap: 10 }}>{voiceSelect}<button type="button" onClick={previewVoice} style={ghostButtonStyle(t)}>试听所选偏好</button></div></Row>}
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
  const languageName = (value: string) => { try { return new Intl.DisplayNames(['zh'], { type: 'language' }).of(value) ?? value; } catch { return value; } };
  const sourceDescription = (route: AudioRouteResolution, operation: string) => {
    if (route.status !== 'ready') return '此来源暂不可用，请选择其他来源或安装模型。';
    if (route.effective?.source === 'system') return `使用系统${operation}。`;
    if (route.effective?.source === 'offline') return `使用${model.models.find((entry) => entry.modelId === route.effective?.modelId)?.displayName ?? '离线模型'}。`;
    return `使用云端${operation}，需要联网。`;
  };

  return (
    <>
      <Card title="语音输入">
        <Row title="语言" desc="跟随系统不会自动识别说话语言；说中文请选择中文。" align="center">
          <select aria-label="语音语言" value={displayDraft.language} onChange={(event) => updateDraft((current) => ({ ...current, language: event.target.value }))} style={inputStyle(t)}>
            {languageOptions.map((value) => <option key={value} value={value}>{value === 'auto' ? '跟随系统' : languageName(value)}</option>)}
          </select>
        </Row>

        <Row title="识别来源" desc={sourceDescription(model.recognitionRoute, "识别")} align="center">
          <select aria-label="识别来源" value={displayDraft.recognition.source} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, source: event.target.value } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.recognition.source) && <option value={displayDraft.recognition.source}>未知来源（不可用）</option>}
          </select>
        </Row>
        {displayDraft.recognition.source === 'offline' && (
        <Row title="离线识别模型" desc="Zipformer、Moonshine 用于识别。MeloTTS、Kitten 请在下方朗读设置中选择。" align="center">
          <select aria-label="离线识别模型" value={recognitionModelValue} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">按语言自动选择</option>
            {model.models.filter((entry) => entry.kind === 'stt').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName} · {entry.state.type === 'ready' ? '已安装' : entry.state.type === 'not-installed' ? '未安装' : '安装中或需重试'}</option>)}
            {displayDraft.recognition.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.recognition.offlineModelId) && <option value={displayDraft.recognition.offlineModelId}>未知模型（不可用）</option>}
          </select>
        </Row>
        )}
        {displayDraft.recognition.source === 'provider' && cloudFields('recognition', displayDraft.recognition.cloud)}
      </Card>
      <Card title="语音输出">
        <Row title="朗读来源" desc={sourceDescription(model.speechRoute, "朗读")} align="center">
          <select aria-label="朗读来源" value={displayDraft.speech.source} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, source: event.target.value, ...(event.target.value === 'automatic' ? { voice: null } : {}) } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.speech.source) && <option value={displayDraft.speech.source}>未知来源（不可用）</option>}
          </select>
        </Row>
        {displayDraft.speech.source === 'offline' && (
        <Row title="离线朗读模型" desc="MeloTTS 支持中英文朗读，Kitten 支持英文朗读。" align="center">
          <select aria-label="离线朗读模型" value={speechModelValue} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">按语言自动选择</option>
            {model.models.filter((entry) => entry.kind === 'tts').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName} · {entry.state.type === 'ready' ? '已安装' : entry.state.type === 'not-installed' ? '未安装' : '安装中或需重试'}</option>)}
            {displayDraft.speech.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.speech.offlineModelId) && <option value={displayDraft.speech.offlineModelId}>未知模型（不可用）</option>}
          </select>
        </Row>
        )}
        {displayDraft.speech.source === 'provider' && cloudFields('speech', displayDraft.speech.cloud)}
        {displayDraft.speech.source !== 'provider' && (
        <Row title="音色" align="center">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <select aria-label="朗读音色" value={speechVoiceValue} onChange={(event) => updateDraft((current) => {
              const voice = voiceSelectionFromValue(event.target.value, model.voiceOptions);
              if (!voice) return { ...current, speech: { ...current.speech, voice: null } };
              return { ...current, speech: { ...current.speech, source: voice.source, offlineModelId: voice.source === 'offline' ? voice.modelId ?? null : current.speech.offlineModelId, voice } };
            })} style={inputStyle(t)}>
              <option value="automatic">自动音色</option>
              {unresolvedVoiceValue && <option value={unresolvedVoiceValue}>未知音色（不可用）：{displayDraft.speech.voice?.id}</option>}
              {systemVoices.map((voice) => <option key={voice.id} value={`system:${voice.id.replace(/^system:/, '')}`}>{voice.label}</option>)}
              {offlineVoices.map((voice) => <option key={voice.id} value={voice.id}>{voice.label}（离线）</option>)}
            </select>
            <button type="button" onClick={previewVoice} style={ghostButtonStyle(t)} aria-label="试听所选偏好">试听</button>
          </div>
        </Row>
        )}
        {displayDraft.speech.source === 'provider' ? (
          <Row title="语速" desc={displayDraft.rate === 1 ? '云端朗读目前仅支持 1.0× 语速。' : `云端朗读目前仅支持 1.0×；保留的 ${displayDraft.rate.toFixed(2)}× 偏好暂不可用。`} align="center">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span style={{ fontSize: 12.5, color: t.text3 }}>1.0×</span>
              {displayDraft.rate !== 1 && <button type="button" onClick={() => updateDraft((current) => ({ ...current, rate: 1 }))} style={ghostButtonStyle(t)}>恢复 1.0×</button>}
            </div>
          </Row>
        ) : (
          <Row title="语速" desc="0.5× – 2.0×" align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><input type="range" min={0.5} max={2} step={0.05} value={displayDraft.rate} onChange={(event) => updateDraft((current) => ({ ...current, rate: Number(event.target.value) }))} aria-label="朗读语速" /><span style={{ fontSize: 12.5, color: t.text3, minWidth: 40 }}>{displayDraft.rate.toFixed(2)}×</span></div></Row>
        )}
        <Row title="自动朗读回复" desc="只朗读普通输入发送后的最终回复；心流模式使用自己的流式播报。" align="center"><label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 12.5, color: t.text2 }}><input type="checkbox" checked={displayDraft.autoPlayReplies} onChange={(event) => updateDraft((current) => ({ ...current, autoPlayReplies: event.target.checked }))} aria-label="自动朗读回复" />{displayDraft.autoPlayReplies ? '已开启' : '已关闭'}</label></Row>
      </Card>

      {(audioConfigurationError || saveError || operationError || model.notices.length > 0) && <div role="status" aria-live="polite" style={{ padding: '0 0 16px', color: t.danger, fontSize: 12.5, lineHeight: 1.5 }}>
        {audioConfigurationError || saveError || (operationError?.includes('429') ? '云端语音额度暂不可用，可切换账号或使用系统、离线语音。' : operationError ? '语音操作失败，请检查语言、来源和模型。' : '所选语音来源暂不可用，请调整来源或安装对应模型。')}
      </div>}
      <Card>        <Row title="保存设置" desc={dirty ? '修改尚未保存。' : '仅保存在此设备。'} align="center"><button type="button" disabled={!dirty || saving} onClick={() => void save()} style={ghostButtonStyle(t, !dirty || saving)}>{saving ? '正在保存…' : '保存'}</button></Row>
      </Card>
      <VoiceSettingsDisclosure title="离线模型" detail={`${model.models.filter((entry) => entry.state.type === 'ready').length} 个已安装`} initiallyOpen={displayDraft.recognition.source === 'offline' || displayDraft.speech.source === 'offline' || model.models.some((entry) => ['downloading', 'failed'].includes(entry.state.type))}>
      <Card>
        <div style={{ padding: "12px 18px", color: t.text3, fontSize: 12 }}>识别模型用于输入，朗读模型用于输出。下载完成后在对应列表中选择。</div>
        {model.models.map((entry) => {
          const busy = ['queued', 'downloading', 'verifying', 'extracting'].includes(entry.state.type);
          const action = entry.state.type === 'ready' ? '删除' : busy ? '取消' : entry.state.type === 'failed' ? '重试' : '下载';
          return <Row key={entry.modelId} title={entry.displayName} desc={`${entry.kind === 'stt' ? '识别' : '朗读'} · ${formatBytes(entry.approxSizeBytes)}${entry.state.type === 'failed' ? ` · ${entry.state.message}` : ''}`} align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><span style={{ fontSize: 12, color: entry.state.type === 'failed' ? t.danger : t.text3 }}>{MODEL_STATE_LABELS[entry.state.type]}{modelProgress(entry) ? ` ${modelProgress(entry)}` : ''}</span><button type="button" onClick={() => changeModel(entry)} style={ghostButtonStyle(t, false, entry.state.type === 'ready')}>{action}</button></div></Row>;
        })}
      </Card>

      </VoiceSettingsDisclosure>
      <VoiceSettingsDisclosure title="高级设置与权限" initiallyOpen={snapshot.permissions.microphone !== 'granted'}>
      <Card title="语音会话">
        <Row title="会话模式" desc={displayDraft.conversation.mode === 'realtime' ? snapshot.realtimeReadiness?.reason ?? '当前宿主尚未提供通过 Agent 工具与权限链路的实时会话。' : '识别输入后交给当前 Agent，按输出设置朗读回复。'} align="center">
          <select aria-label="语音会话模式" value={displayDraft.conversation.mode} onChange={(event) => updateDraft((current) => ({ ...current, conversation: { ...current.conversation, mode: event.target.value } }))} style={inputStyle(t)}>
            <option value="agent">Agent 会话</option>
            <option value="realtime">原生实时会话{snapshot.realtimeReadiness?.ready ? '' : '（需要配置或宿主支持）'}</option>
          </select>
        </Row>
        {displayDraft.conversation.mode === 'realtime' && <><Row title="交互方式" desc="轮流说话：聆听结束后播放回复；此设备尚未验证回声消除。" align="center"><select aria-label="实时交互方式" value={displayDraft.conversation.interaction} onChange={(event) => updateDraft((current) => ({ ...current, conversation: { ...current.conversation, interaction: event.target.value } }))} style={inputStyle(t)}><option value="turn_based">轮流说话</option>{displayDraft.conversation.interaction !== 'turn_based' && <option value={displayDraft.conversation.interaction}>插话模式（当前不可用）</option>}</select></Row>{cloudFields('conversation', displayDraft.conversation.cloud)}</>}
      </Card>
      <Card title="权限">
        <Row title="麦克风" desc={`当前状态：${permissionLabel(model.microphonePermission)}`} align="center">{model.microphonePermission === 'denied' ? <button type="button" onClick={() => void bridge.openSystemSettings('microphone')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.microphonePermission === 'prompt' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.microphonePermission)}</span>}</Row>
        <Row title="系统语音识别" desc={`当前状态：${permissionLabel(model.speechPermission)}`} align="center">{model.speechPermission === 'denied' || model.speechPermission === 'restricted' ? <button type="button" onClick={() => void bridge.openSystemSettings('speech_recognition')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.speechPermission === 'not_determined' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.speechPermission)}</span>}</Row>
        <Row title="原生服务" desc={snapshot.helper.message ?? '语音在本机处理。'} align="center"><span style={{ color: t.text3, fontSize: 12.5 }}>{snapshot.helper.state === "running" ? "可用" : "需要检查"}</span></Row>
      </Card>
        <Card title="实际使用">
          <Row title="识别" desc={model.actualRecognition}>{null}</Row>
          <Row title="朗读" desc={model.actualSpeech}>{null}</Row>
          <Row title="自定义语言" desc="填写语言地区代码，例如 zh-CN。" align="center"><input aria-label="自定义语音语言" value={displayDraft.language} onChange={(event) => updateDraft((current) => ({ ...current, language: event.target.value.trim() || AUDIO_LANGUAGE_AUTO }))} style={inputStyle(t)} /></Row>
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
