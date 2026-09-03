import { useEffect, useMemo, useState } from 'react';

import {
  defaultNativeAudioSnapshot,
  type NativeAudioModelSnapshot,
  type NativeAudioSnapshot,
  type NativeAudioVoiceOption,
  type SpeechPermissionStatus,
} from '../../../../shared/nativeAudio';
import { offlineVoiceModels } from '../../../../shared/voiceModelCatalog';
import {
  defaultVoicePreferences,
  isLegacySystemVoiceAlias,
  LANGUAGE_AUTO,
  SYSTEM_VOICE_PREFIX,
  type VoicePreferences,
} from '../../../../shared/voicePreferences';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { Card, Row } from '../rows';
import { ghostButtonStyle, inputStyle } from './ghostButton';

const EMPTY_NATIVE_AUDIO_SNAPSHOT = defaultNativeAudioSnapshot();

const MICROPHONE_LABELS: Record<NativeAudioSnapshot['permissions']['microphone'], string> = {
  granted: '已授权', denied: '未授权', prompt: '尚未询问', unavailable: '无法确定',
};
const SPEECH_LABELS: Record<SpeechPermissionStatus, string> = {
  authorized: '已授权', denied: '已拒绝', restricted: '受系统限制', not_determined: '尚未询问', unavailable: '不可用',
};
const MODEL_STATE_LABELS: Record<NativeAudioModelSnapshot['state']['type'], string> = {
  'not-installed': '未安装', queued: '等待下载', downloading: '下载中', verifying: '正在校验', extracting: '正在安装', ready: '可用', failed: '失败',
};

export interface VoiceRecognitionOption {
  id: VoicePreferences['recognitionMode'];
  label: string;
  disabled: false;
  disabledReason: null;
}

export interface VoicePageModel {
  recognitionOptions: VoiceRecognitionOption[];
  recognitionMode: VoicePreferences['recognitionMode'];
  effectiveBackend: 'apple' | 'sherpa' | 'unavailable';
  effectiveLanguage: string;
  recognitionDetail: string;
  fallbackReason: string | null;
  microphonePermission: NativeAudioSnapshot['permissions']['microphone'];
  microphonePermissionLabel: string;
  speechPermission: SpeechPermissionStatus;
  speechPermissionLabel: string;
  recognizerAvailable: boolean;
  voiceOptions: NativeAudioVoiceOption[];
  voiceSelection: string;
  effectiveVoiceId: string | null;
  effectiveVoiceLabel: string | null;
  rate: number;
  autoPlayReplies: boolean;
  models: Array<NativeAudioModelSnapshot & { displayName: string; kind: 'stt' | 'tts'; approxSizeBytes: number }>;
  notices: string[];
}

export function canonicalVoicePreferencesForSave(
  preferences: VoicePreferences,
  snapshot: NativeAudioSnapshot,
): VoicePreferences {
  if (!isLegacySystemVoiceAlias(preferences.voiceSelection)) return preferences;
  const legacyName = preferences.voiceSelection.slice(SYSTEM_VOICE_PREFIX.length);
  const resolved = snapshot.voices.find((voice) => (
    voice.source === 'system' && voice.label.localeCompare(legacyName, undefined, { sensitivity: 'accent' }) === 0
  ));
  return { ...preferences, voiceSelection: resolved?.id ?? 'system:default' };
}

function languageBase(language: string): string {
  return language.replace(/_/g, '-').split('-')[0]?.toLowerCase() ?? '';
}

function modelSnapshot(snapshot: NativeAudioSnapshot, modelId: string): NativeAudioModelSnapshot {
  return snapshot.models.find((model) => model.modelId === modelId) ?? { modelId, state: { type: 'not-installed' } };
}

export function voicePageModel(prefs: VoicePreferences, snapshot: NativeAudioSnapshot): VoicePageModel {
  const configuredLanguage = prefs.language === LANGUAGE_AUTO ? (snapshot.localeTag ?? 'auto') : prefs.language;
  const effectiveLanguage = snapshot.recognition?.effectiveLanguage ?? configuredLanguage;
  const readyOfflineRecognizer = offlineVoiceModels.some((model) => (
    model.kind === 'stt'
    && model.languages.includes(languageBase(effectiveLanguage))
    && modelSnapshot(snapshot, model.id).state.type === 'ready'
  ));
  const inferredBackend = prefs.recognitionMode === 'localOnly' && readyOfflineRecognizer ? 'sherpa' : 'unavailable';
  const effectiveBackend = snapshot.recognition?.effectiveBackend ?? inferredBackend;
  const legacyVoiceName = isLegacySystemVoiceAlias(prefs.voiceSelection)
    ? prefs.voiceSelection.slice(SYSTEM_VOICE_PREFIX.length)
    : null;
  const requestedVoice = snapshot.voices.find((voice) => voice.id === prefs.voiceSelection)
    ?? (legacyVoiceName
      ? snapshot.voices.find((voice) => voice.source === 'system' && voice.label.localeCompare(legacyVoiceName, undefined, { sensitivity: 'accent' }) === 0)
      : undefined);
  const effectiveVoice = snapshot.voices.find((voice) => voice.id === snapshot.playback?.effectiveVoiceId)
    ?? requestedVoice ?? snapshot.voices.find((voice) => voice.isDefault) ?? snapshot.voices[0] ?? null;
  const notices: string[] = [];
  if (snapshot.helper.state === 'failed') notices.push(snapshot.helper.message ?? '原生音频服务启动失败。');
  if (snapshot.permissions.microphone !== 'granted') notices.push('尚未获得麦克风权限，录音功能无法使用。');
  if (prefs.recognitionMode === 'localOnly' && !readyOfflineRecognizer) notices.push(`当前语言 ${effectiveLanguage} 尚未安装离线识别模型。`);
  if (prefs.recognitionMode === 'automatic' && effectiveBackend === 'sherpa' && snapshot.recognition?.fallbackReason) {
    notices.push(`系统识别已回退到离线模型：${snapshot.recognition.fallbackReason}`);
  }
  if (prefs.voiceSelection !== 'system:default' && !requestedVoice) notices.push('之前选择的音色当前不可用，正在使用可用的替代音色。');
  if (!effectiveVoice) notices.push('没有可用于朗读的系统或离线音色。');
  return {
    recognitionOptions: [
      { id: 'automatic', label: '自动', disabled: false, disabledReason: null },
      { id: 'localOnly', label: '仅本设备（离线）', disabled: false, disabledReason: null },
    ],
    recognitionMode: prefs.recognitionMode,
    effectiveBackend,
    effectiveLanguage,
    recognitionDetail: snapshot.recognition?.detail
      ?? (effectiveBackend === 'unavailable' ? '等待可用的系统识别器或已安装的离线模型。' : '语音识别已就绪。'),
    fallbackReason: snapshot.recognition?.fallbackReason ?? null,
    microphonePermission: snapshot.permissions.microphone,
    microphonePermissionLabel: MICROPHONE_LABELS[snapshot.permissions.microphone],
    speechPermission: snapshot.permissions.speech,
    speechPermissionLabel: SPEECH_LABELS[snapshot.permissions.speech],
    recognizerAvailable: snapshot.recognizerAvailable === true,
    voiceOptions: snapshot.voices,
    voiceSelection: prefs.voiceSelection,
    effectiveVoiceId: effectiveVoice?.id ?? null,
    effectiveVoiceLabel: snapshot.playback?.effectiveVoiceLabel ?? effectiveVoice?.label ?? null,
    rate: prefs.rate,
    autoPlayReplies: prefs.autoPlayReplies,
    models: offlineVoiceModels.map((entry) => ({
      ...modelSnapshot(snapshot, entry.id),
      displayName: entry.displayName.zh ?? entry.displayName.en ?? entry.id,
      kind: entry.kind,
      approxSizeBytes: entry.approxSizeBytes,
    })),
    notices,
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

export function Voice({ bridge }: PageContentProps) {
  const t = useT();
  const prefs = bridge.bootstrap?.settings?.voice ?? defaultVoicePreferences();
  const bridgeAudioSnapshot = bridge.audioSnapshot ?? EMPTY_NATIVE_AUDIO_SNAPSHOT;
  const [snapshot, setSnapshot] = useState(bridgeAudioSnapshot);
  const [languageDraft, setLanguageDraft] = useState(() => (prefs.language === LANGUAGE_AUTO ? '' : prefs.language));
  const [operationError, setOperationError] = useState<string | null>(null);
  const model = useMemo(() => voicePageModel(prefs, snapshot), [prefs, snapshot]);

  useEffect(() => setSnapshot(bridgeAudioSnapshot), [bridgeAudioSnapshot]);
  useEffect(() => {
    const refresh = () => {
      if (typeof bridge.audioRequest === 'function') {
        void bridge.audioRequest({ type: 'get_snapshot' }).then((response) => setSnapshot(response.snapshot));
        return;
      }
      if (typeof bridge.microphonePermission === 'function') {
        void bridge.microphonePermission().then((microphone) => {
          setSnapshot((current) => ({
            ...current,
            permissions: { ...current.permissions, microphone },
          }));
        });
      }
    };
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [bridge.audioRequest, bridge.microphonePermission]);

  const persist = (next: VoicePreferences) => void bridge.setVoicePreferences(canonicalVoicePreferencesForSave(next, snapshot)).catch(() => undefined);
  const perform = async (command: Parameters<typeof bridge.audioRequest>[0]) => {
    setOperationError(null);
    if (typeof bridge.audioRequest !== 'function') {
      setOperationError('原生音频服务不可用。');
      return null;
    }
    try {
      const response = await bridge.audioRequest(command);
      setSnapshot(response.snapshot);
      if (response.type === 'error') setOperationError(response.error.message);
      return response;
    } catch (cause) {
      setOperationError(cause instanceof Error ? cause.message : '原生音频操作失败。');
      return null;
    }
  };
  const commitLanguage = () => {
    const language = languageDraft.trim();
    persist({ ...prefs, language: language || LANGUAGE_AUTO });
  };
  const previewVoice = () => void perform({
    type: 'speak', owner: { kind: 'preview', id: 'settings-voice-preview' },
    text: '这是 LingXi Desktop 的语音试听。This is a LingXi Desktop voice preview.', voiceId: prefs.voiceSelection, rate: prefs.rate,
  });
  const requestPermissions = () => void perform({ type: 'request_authorization', permissions: ['microphone', 'speech'] });
  const changeModel = (entry: VoicePageModel['models'][number]) => {
    if (entry.state.type === 'ready') return void perform({ type: 'remove_model', modelId: entry.modelId });
    if (entry.state.type === 'queued' || entry.state.type === 'downloading' || entry.state.type === 'verifying' || entry.state.type === 'extracting') {
      return void perform({ type: 'cancel_model', modelId: entry.modelId });
    }
    return void perform({ type: 'install_model', modelId: entry.modelId });
  };

  return (
    <>
      <Card title="识别">
        <Row title="识别模式" desc={`当前生效：${model.effectiveBackend === 'apple' ? 'macOS 系统识别' : model.effectiveBackend === 'sherpa' ? 'Sherpa 离线识别' : '不可用'} · ${model.effectiveLanguage}`} align="center">
          <div style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
            {model.recognitionOptions.map((option) => {
              const active = model.recognitionMode === option.id;
              return <button key={option.id} type="button" data-recognition-mode={option.id} aria-pressed={active} onClick={() => persist({ ...prefs, recognitionMode: option.id })} style={{ padding: '5px 14px', borderRadius: 7, border: 'none', fontFamily: 'inherit', cursor: 'pointer', background: active ? t.surface : 'transparent', color: active ? t.text : t.text3, fontSize: 12.5, fontWeight: active ? 600 : 500 }}>{option.label}</button>;
            })}
          </div>
        </Row>
        <Row title="识别语言" desc={`当前生效：${model.effectiveLanguage}${prefs.language === LANGUAGE_AUTO ? '（跟随系统）' : ''}`} align="center">
          <input value={languageDraft} onChange={(event) => setLanguageDraft(event.target.value)} onBlur={commitLanguage} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); }} placeholder="auto" aria-label="识别语言" style={inputStyle(t)} />
        </Row>
        <Row title="实际后端" desc={model.recognitionDetail} align="center"><span style={{ color: t.text3, fontSize: 12.5 }}>{model.fallbackReason ?? '无回退'}</span></Row>
      </Card>

      <Card title="音色">
        <Row title="朗读音色" desc={`请求：${model.voiceSelection} · 实际：${model.effectiveVoiceLabel ?? '不可用'}`} align="center">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <select value={model.voiceOptions.some((voice) => voice.id === model.voiceSelection) ? model.voiceSelection : 'system:default'} onChange={(event) => persist({ ...prefs, voiceSelection: event.target.value })} aria-label="朗读音色" style={inputStyle(t)}>
              <option value="system:default">系统默认</option>
              {model.voiceOptions.filter((voice) => voice.id !== 'system:default').map((voice) => <option key={voice.id} value={voice.id}>{voice.label}{voice.source === 'sherpa' ? '（离线）' : voice.networkRequired ? '（需要网络）' : ''}</option>)}
            </select>
            <button type="button" onClick={previewVoice} disabled={!model.effectiveVoiceId} style={ghostButtonStyle(t, !model.effectiveVoiceId)}>试听</button>
          </div>
        </Row>
      </Card>

      <Card title="离线模型">
        {model.models.map((entry) => {
          const busy = entry.state.type === 'queued' || entry.state.type === 'downloading' || entry.state.type === 'verifying' || entry.state.type === 'extracting';
          const action = entry.state.type === 'ready' ? '删除' : busy ? '取消' : entry.state.type === 'failed' ? '重试' : '下载';
          return <Row key={entry.modelId} title={entry.displayName} desc={`${entry.kind === 'stt' ? '语音识别' : '语音合成'} · ${formatBytes(entry.approxSizeBytes)}${entry.state.type === 'failed' ? ` · ${entry.state.message}` : ''}`} align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><span style={{ fontSize: 12, color: entry.state.type === 'failed' ? t.danger : t.text3 }}>{MODEL_STATE_LABELS[entry.state.type]}{modelProgress(entry) ? ` ${modelProgress(entry)}` : ''}</span><button type="button" onClick={() => changeModel(entry)} style={ghostButtonStyle(t, false, entry.state.type === 'ready')}>{action}</button></div></Row>;
        })}
      </Card>

      <Card title="播放">
        <Row title="语速" desc="0.5× – 2.0×" align="center"><div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><input type="range" min={0.5} max={2} step={0.05} value={model.rate} onChange={(event) => persist({ ...prefs, rate: Number(event.target.value) })} aria-label="朗读语速" /><span style={{ fontSize: 12.5, color: t.text3, minWidth: 40 }}>{model.rate.toFixed(2)}×</span></div></Row>
        <Row title="自动朗读回复" desc="只朗读普通输入发送后的最终回复；心流模式使用自己的流式播报。" align="center"><label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 12.5, color: t.text2 }}><input type="checkbox" checked={model.autoPlayReplies} onChange={(event) => persist({ ...prefs, autoPlayReplies: event.target.checked })} aria-label="自动朗读回复" />{model.autoPlayReplies ? '已开启' : '已关闭'}</label></Row>
      </Card>

      <Card title="权限与状态">
        <Row title="麦克风" desc={`当前状态：${model.microphonePermissionLabel}`} align="center">{model.microphonePermission === 'denied' ? <button type="button" onClick={() => void bridge.openSystemSettings('microphone')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.microphonePermission === 'prompt' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{model.microphonePermissionLabel}</span>}</Row>
        <Row title="语音识别" desc={`当前状态：${model.speechPermissionLabel}`} align="center">{model.speechPermission === 'denied' || model.speechPermission === 'restricted' ? <button type="button" onClick={() => void bridge.openSystemSettings('speech_recognition')} style={ghostButtonStyle(t)}>打开系统设置</button> : model.speechPermission === 'not_determined' ? <button type="button" onClick={requestPermissions} style={ghostButtonStyle(t)}>请求授权</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{model.speechPermissionLabel}</span>}</Row>
        <Row title="系统识别器" desc={model.recognizerAvailable ? 'Apple Speech 设备端识别可用。' : '设备端系统识别不可用；automatic 会尝试已安装的 Sherpa 模型。'} align="center"><span style={{ color: model.recognizerAvailable ? t.accent : t.text3, fontSize: 12.5 }}>{model.recognizerAvailable ? '可用' : '不可用'}</span></Row>
        <Row title="原生服务" desc={snapshot.helper.message ?? '录音、识别、模型与朗读均由本机 Helper 处理。'} align="center">{snapshot.helper.state === 'failed' ? <button type="button" onClick={() => void perform({ type: 'get_snapshot' })} style={ghostButtonStyle(t)}>重试原生服务</button> : <span style={{ color: t.text3, fontSize: 12.5 }}>{snapshot.helper.state}</span>}</Row>
        {(operationError || model.notices.length > 0) && <div role="status" aria-live="polite" data-testid="voice-notices" style={{ padding: '14px 18px', display: 'flex', flexDirection: 'column', gap: 8 }}>{operationError && <div style={{ color: t.danger, fontSize: 12.5 }}>{operationError}</div>}{model.notices.map((notice) => <div key={notice} style={{ color: t.text2, fontSize: 12.5 }}>· {notice}</div>)}</div>}
      </Card>
    </>
  );
}
