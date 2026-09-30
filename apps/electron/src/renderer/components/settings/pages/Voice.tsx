import { useEffect, useMemo, useRef, useState } from 'react';
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
  type AudioConfigurationV3,
  type AudioOfflineModelAvailability,
  type AudioRouteResolution,
  type AudioVoiceSelection,
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
] as const;

export interface VoicePageModel {
  displayConfiguration: AudioConfigurationV3;
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
  const backend = snapshot.recognition?.effectiveBackend;
  if (!backend || backend === 'unavailable') return '暂无正在执行的识别操作';
  return `${backend === 'apple' ? 'macOS 系统识别' : 'Sherpa 离线识别'}${snapshot.recognition?.effectiveLanguage ? ` · ${snapshot.recognition.effectiveLanguage}` : ''}`;
}

function actualSpeech(snapshot: NativeAudioSnapshot): string {
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

function resolveSystemVoiceAlias(voice: AudioVoiceSelection | null, voices: NativeAudioVoiceOption[]): AudioVoiceSelection | null {
  if (!voice || voice.source !== 'system') return voice;
  const requested = voice.id.replace(/^system:/, '').trim();
  const systemVoices = voices.filter((option) => option.source === 'system');
  const exact = systemVoices.find((option) => systemVoiceOptionId(option) === requested);
  if (exact) return { ...voice, id: systemVoiceOptionId(exact) };

  const matches = systemVoices.filter((option) => option.label.trim().toLowerCase() === requested.toLowerCase());
  return matches.length === 1 ? { ...voice, id: systemVoiceOptionId(matches[0]!) } : voice;
}

function resolveVoiceAliasesForDisplay(config: AudioConfigurationV3, voices: NativeAudioVoiceOption[]): AudioConfigurationV3 {
  const voice = resolveSystemVoiceAlias(config.speech.voice, voices);
  if (voice === config.speech.voice || voice?.id === config.speech.voice?.id) return config;
  return { ...config, speech: { ...config.speech, voice } };
}

export function voicePageModel(config: AudioConfigurationV3, snapshot: NativeAudioSnapshot): VoicePageModel {
  const displayConfiguration = resolveVoiceAliasesForDisplay(config, snapshot.voices);
  const language = resolveAudioLanguage(displayConfiguration.language, snapshot.localeTag ?? 'en-US');
  const offlineModels = audioModels(snapshot);
  const systemVoiceIds = snapshot.voices.filter((voice) => voice.source === 'system').map((voice) => voice.id.replace(/^system:/, ''));
  const recognitionRoute = resolveAudioRoute({
    kind: 'recognition', preference: displayConfiguration.recognition, language,
    systemStatus: readiness(snapshot, 'recognition'), offlineModels,
  });
  const speechRoute = resolveAudioRoute({
    kind: 'speech', preference: displayConfiguration.speech, language,
    systemStatus: readiness(snapshot, 'speech'), offlineModels, systemVoiceIds,
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

export function voicePreviewRequest(configuration: AudioConfigurationV3, deviceLocale?: string): {
  operation: Extract<AudioOperationDto, { type: 'speak' }>;
  configuration: AudioConfigurationV3;
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

function routeDescription(route: AudioRouteResolution): string {
  if (route.effective?.source === 'system') return `系统${route.effective.voiceId ? ` · ${route.effective.voiceId}` : ''}`;
  if (route.effective?.source === 'offline') return `离线 · ${route.effective.modelId ?? '语言默认模型'}`;
  return `不可用 · ${route.reason}`;
}

export function voiceSelectionOptionValue(voice: AudioVoiceSelection | null, fallbackModelId?: string | null): string {
  if (!voice) return 'automatic';
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
  configuration: AudioConfigurationV3,
  editRevisionAtSave: number,
  currentEditRevision: () => number,
  persist: (configuration: AudioConfigurationV3) => Promise<unknown>,
): Promise<boolean> {
  await persist(configuration);
  return currentEditRevision() === editRevisionAtSave;
}

export function Voice({ bridge }: PageContentProps) {
  const t = useT();
  const saved = bridge.bootstrap?.settings?.voice ?? audioConfigurationDefaults();
  const snapshotFromBridge = bridge.audioSnapshot ?? EMPTY_NATIVE_AUDIO_SNAPSHOT;
  const [snapshot, setSnapshot] = useState(snapshotFromBridge);
  const [draft, setDraft] = useState<AudioConfigurationV3>(saved);
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
    const refresh = () => {
      if (typeof bridge.audioRequest === 'function') {
        void bridge.audioRequest({ type: 'get_snapshot' }).then((response) => setSnapshot(response.snapshot)).catch((error) => {
          setOperationError(error instanceof Error ? error.message : '读取原生音频状态失败。');
        });
        return;
      }
      if (typeof bridge.microphonePermission === 'function') {
        void bridge.microphonePermission().then((microphone) => setSnapshot((current) => ({
          ...current, permissions: { ...current.permissions, microphone },
        })));
      }
    };
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [bridge.audioRequest, bridge.microphonePermission]);

  const updateDraft = (change: (previous: AudioConfigurationV3) => AudioConfigurationV3) => {
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
    configurationOverride?: AudioConfigurationV3,
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

  const recognitionModelValue = displayDraft.recognition.offlineModelId ?? 'language-default';
  const speechModelValue = displayDraft.speech.offlineModelId ?? 'language-default';
  const speechVoiceValue = voiceSelectionOptionValue(displayDraft.speech.voice, displayDraft.speech.offlineModelId);
  const systemVoices = model.voiceOptions.filter((voice) => voice.source === 'system');
  const offlineVoices = model.voiceOptions.filter((voice) => voice.source === 'sherpa' && (!displayDraft.speech.offlineModelId || voice.familyId === displayDraft.speech.offlineModelId));
  const availableVoiceValues = new Set([
    ...systemVoices.map((voice) => `system:${systemVoiceOptionId(voice)}`),
    ...offlineVoices.map((voice) => voice.id),
  ]);
  const unresolvedVoiceValue = speechVoiceValue !== 'automatic' && !availableVoiceValues.has(speechVoiceValue)
    ? speechVoiceValue
    : null;

  return (
    <>
      <Card title="语音设置">
        <Row title="识别来源" desc={`请求：${displayDraft.recognition.source} · 预览：${routeDescription(model.recognitionRoute)} · 实际：${model.actualRecognition}`} align="center">
          <select aria-label="识别来源" value={displayDraft.recognition.source} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, source: event.target.value } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.recognition.source) && <option value={displayDraft.recognition.source}>未知来源（不可用）</option>}
          </select>
        </Row>
        <Row title="离线识别模型" desc="自动使用共享目录中第一个已安装且语言匹配的模型；指定模型不会回退。" align="center">
          <select aria-label="离线识别模型" value={recognitionModelValue} onChange={(event) => updateDraft((current) => ({ ...current, recognition: { ...current.recognition, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">按语言自动选择</option>
            {model.models.filter((entry) => entry.kind === 'stt').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName}</option>)}
            {displayDraft.recognition.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.recognition.offlineModelId) && <option value={displayDraft.recognition.offlineModelId}>未知模型（不可用）</option>}
          </select>
        </Row>
        <Row title="朗读来源" desc={`请求：${displayDraft.speech.source} · 预览：${routeDescription(model.speechRoute)} · 实际：${model.actualSpeech}`} align="center">
          <select aria-label="朗读来源" value={displayDraft.speech.source} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, source: event.target.value, ...(event.target.value === 'automatic' ? { voice: null } : {}) } }))} style={inputStyle(t)}>
            {SOURCE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            {!SOURCE_OPTIONS.some((option) => option.value === displayDraft.speech.source) && <option value={displayDraft.speech.source}>未知来源（不可用）</option>}
          </select>
        </Row>
        <Row title="离线朗读模型" desc="只影响朗读；选择系统来源时不会使用离线模型。" align="center">
          <select aria-label="离线朗读模型" value={speechModelValue} onChange={(event) => updateDraft((current) => ({ ...current, speech: { ...current.speech, offlineModelId: event.target.value === 'language-default' ? null : event.target.value } }))} style={inputStyle(t)}>
            <option value="language-default">按语言自动选择</option>
            {model.models.filter((entry) => entry.kind === 'tts').map((entry) => <option key={entry.modelId} value={entry.modelId}>{entry.displayName}</option>)}
            {displayDraft.speech.offlineModelId && !model.models.some((entry) => entry.modelId === displayDraft.speech.offlineModelId) && <option value={displayDraft.speech.offlineModelId}>未知模型（不可用）</option>}
          </select>
        </Row>
        <Row title="朗读音色" desc={displayDraft.speech.voice ? `请求：${displayDraft.speech.voice.source} · ${displayDraft.speech.voice.id} · 实际：${model.speechRoute.effective?.voiceId ?? '不可用'}` : `请求：自动 · 实际：${model.speechRoute.effective?.voiceId ?? '不可用'}`} align="center">
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
            <button type="button" onClick={previewVoice} style={ghostButtonStyle(t)}>试听所选偏好</button>
          </div>
        </Row>
        <Row title="语言" desc={`auto 将在每次操作开始时跟随系统语言；当前：${resolveAudioLanguage(displayDraft.language, snapshot.localeTag ?? 'en-US')}`} align="center">
          <input aria-label="语音语言" value={displayDraft.language === AUDIO_LANGUAGE_AUTO ? '' : displayDraft.language} onChange={(event) => updateDraft((current) => ({ ...current, language: event.target.value.trim() || AUDIO_LANGUAGE_AUTO }))} placeholder="auto" style={inputStyle(t)} />
        </Row>
        <Row title="语速" desc="0.5× – 2.0×" align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><input type="range" min={0.5} max={2} step={0.05} value={displayDraft.rate} onChange={(event) => updateDraft((current) => ({ ...current, rate: Number(event.target.value) }))} aria-label="朗读语速" /><span style={{ fontSize: 12.5, color: t.text3, minWidth: 40 }}>{displayDraft.rate.toFixed(2)}×</span></div></Row>
        <Row title="自动朗读回复" desc="只朗读普通输入发送后的最终回复；心流模式使用自己的流式播报。" align="center"><label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 12.5, color: t.text2 }}><input type="checkbox" checked={displayDraft.autoPlayReplies} onChange={(event) => updateDraft((current) => ({ ...current, autoPlayReplies: event.target.checked }))} aria-label="自动朗读回复" />{displayDraft.autoPlayReplies ? '已开启' : '已关闭'}</label></Row>
        <Row title="保存语音偏好" desc={dirty ? '修改尚未保存。' : '设置保存在此设备，不会写入项目或模型配置。'} align="center"><button type="button" disabled={!dirty || saving} onClick={() => void save()} style={ghostButtonStyle(t, !dirty || saving)}>{saving ? '正在保存…' : '保存'}</button></Row>
        {(audioConfigurationError || saveError || operationError || model.notices.length > 0) && <div role="status" aria-live="polite" data-testid="voice-notices" style={{ padding: '12px 18px', display: 'flex', flexDirection: 'column', gap: 8 }}>{audioConfigurationError && <div style={{ color: t.danger, fontSize: 12.5 }}>{audioConfigurationError}</div>}{saveError && <div style={{ color: t.danger, fontSize: 12.5 }}>{saveError}</div>}{operationError && <div style={{ color: t.danger, fontSize: 12.5 }}>{operationError}</div>}{model.notices.map((notice) => <div key={notice} style={{ color: t.text2, fontSize: 12.5 }}>· {notice}</div>)}</div>}
      </Card>

      <Card title="离线模型">
        {model.models.map((entry) => {
          const busy = ['queued', 'downloading', 'verifying', 'extracting'].includes(entry.state.type);
          const action = entry.state.type === 'ready' ? '删除' : busy ? '取消' : entry.state.type === 'failed' ? '重试' : '下载';
          return <Row key={entry.modelId} title={entry.displayName} desc={`${entry.kind === 'stt' ? '语音识别' : '语音合成'} · ${formatBytes(entry.approxSizeBytes)}${entry.state.type === 'failed' ? ` · ${entry.state.message}` : ''}`} align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><span style={{ fontSize: 12, color: entry.state.type === 'failed' ? t.danger : t.text3 }}>{MODEL_STATE_LABELS[entry.state.type]}{modelProgress(entry) ? ` ${modelProgress(entry)}` : ''}</span><button type="button" onClick={() => changeModel(entry)} style={ghostButtonStyle(t, false, entry.state.type === 'ready')}>{action}</button></div></Row>;
        })}
      </Card>

      <Card title="权限与状态">
        <Row title="麦克风" desc={`当前状态：${permissionLabel(model.microphonePermission)}`} align="center">{model.microphonePermission === 'denied' ? <button type="button" onClick={() => void bridge.openSystemSettings('microphone')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.microphonePermission === 'prompt' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.microphonePermission)}</span>}</Row>
        <Row title="系统语音识别" desc={`当前状态：${permissionLabel(model.speechPermission)}`} align="center">{model.speechPermission === 'denied' || model.speechPermission === 'restricted' ? <button type="button" onClick={() => void bridge.openSystemSettings('speech_recognition')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.speechPermission === 'not_determined' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{permissionLabel(model.speechPermission)}</span>}</Row>
        <Row title="原生服务" desc={snapshot.helper.message ?? '识别、合成与播放均由本机 Helper 处理。'} align="center"><span style={{ color: t.text3, fontSize: 12.5 }}>{snapshot.helper.state}</span></Row>
      </Card>
    </>
  );
}
