import type { AudioOperationKindDto } from '@lingxi/bridge-client';
import type { AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import { resolveAudioLanguage, resolveAudioRoute } from '../../shared/generatedAudioConfiguration.js';
import type { NativeAudioSnapshot } from '../../shared/nativeAudio.js';
import { offlineVoiceModels } from '../../shared/voiceModelCatalog.js';

export interface AudioFeatureGate { visible: boolean; ready: boolean; reason?: string }

/** Settings remain reachable even when a usage operation is unsupported. */
export function audioFeatureGate(configuration: AudioConfigurationV4, snapshot: NativeAudioSnapshot, operation: AudioOperationKindDto): AudioFeatureGate {
  const kind = operation === 'listen' ? 'recognition' : 'speech';
  const preference = kind === 'recognition' ? configuration.recognition : configuration.speech;
  if (preference.source !== 'provider' && !snapshot.capabilities?.supported_operations.includes(operation)) return { visible: false, ready: false, reason: '当前设备不支持此音频操作。' };
  const route = resolveAudioRoute({
    kind, preference, language: resolveAudioLanguage(configuration.language, snapshot.localeTag),
    systemStatus: kind === 'speech' ? snapshot.voices.some((voice) => voice.source === 'system') ? 'available' : 'unavailable'
      : snapshot.permissions.speech === 'authorized' && snapshot.recognizerAvailable ? 'available'
        : snapshot.permissions.speech === 'not_determined' ? 'permissionRequired'
          : snapshot.permissions.speech === 'denied' || snapshot.permissions.speech === 'restricted' ? 'denied' : 'unavailable',
    offlineModels: offlineVoiceModels.map((entry) => ({ id: entry.id, kind: entry.kind === 'stt' ? 'recognition' : 'speech', languages: entry.languages, installed: snapshot.models.some((model) => model.modelId === entry.id && model.state.type === 'ready') })),
    sessionContext: snapshot.sessionContext, providerCapabilities: snapshot.providerCapabilities,
  });
  if (route.reason === 'providerOperationUnsupported') return { visible: false, ready: false, reason: '当前服务商不支持此音频操作。' };
  const permissionMissing = operation === 'listen' && snapshot.permissions.microphone !== 'granted';
  // Provider transcription uses native capture; provider speech uses native playback.
  // Their model readiness comes from the hosted route, not the helper's recognizer.
  const operationReadiness = preference.source === 'provider'
    ? operation === 'listen'
      ? snapshot.capabilities?.readiness.find((entry) => entry.operation === 'record')?.state
      : snapshot.activeRecordingCount > 0 || snapshot.activePlaybackCount > 0 ? 'busy' : 'ready'
    : snapshot.capabilities?.readiness.find((entry) => entry.operation === operation)?.state;
  const ready = route.status === 'ready' && !permissionMissing && (!operationReadiness || operationReadiness === 'ready');
  const deviceReason = operationReadiness === 'busy' ? '音频设备正在使用中。'
    : operationReadiness === 'missing_model' ? '当前语言缺少可用的识别模型，请在语音设置中下载模型。'
      : operationReadiness === 'needs_permission' ? '需要语音授权，请在语音设置中授权。'
        : '当前语言的音频服务不可用，请检查语音设置。';
  return { visible: true, ready, ...(ready ? {} : { reason: permissionMissing ? '需要麦克风授权，请在语音设置中授权。' : route.reason === 'ready' ? deviceReason : `音频暂不可用：${route.reason}。请检查语音设置。` }) };
}

export function audioConversationGate(configuration: AudioConfigurationV4, snapshot: NativeAudioSnapshot): AudioFeatureGate {
  if (configuration.conversation.mode === 'realtime') return { visible: snapshot.realtimeReadiness?.supported === true, ready: snapshot.realtimeReadiness?.ready === true, reason: snapshot.realtimeReadiness?.reason };
  const input = audioFeatureGate(configuration, snapshot, 'listen');
  const output = audioFeatureGate(configuration, snapshot, 'speak');
  return { visible: input.visible && output.visible, ready: input.ready && output.ready, reason: input.reason ?? output.reason };
}

/** A temporary failed attempt does not change the device's declared support. */
export function voiceStateAfterAudioFailure(kind: string): 'idle' | 'denied' | 'unsupported' {
  if (kind === 'permission_denied') return 'denied';
  if (kind === 'unsupported') return 'unsupported';
  return 'idle';
}

export interface AudioSetupGuidance { title: string; description: string }

/** Offer an actionable setting, without exposing transport or route internals. */
export function audioSetupGuidance(kind: string, message = ''): AudioSetupGuidance | null {
  if (kind === 'cancelled') return null;
  if (kind === 'permission_denied' || /授权|permission|denied/i.test(message)) return {
    title: '需要语音权限', description: '在语音设置中检查麦克风和语音识别授权。',
  };
  if (/foreground/i.test(message)) return {
    title: '请在前台使用麦克风', description: '返回此窗口后重试；也可在语音设置中检查输入设备和识别来源。',
  };
  if (/\b429\b/.test(message)) return {
    title: '云端语音服务暂不可用', description: '服务返回 HTTP 429。请检查账号额度，或在语音设置中切换到系统或离线来源。',
  };
  if (kind === 'no_speech' || /no speech|no-speech/i.test(message)) return {
    title: '未识别到语音', description: '请确认麦克风能收到声音，并在语音设置中选择与你说话一致的语言。',
  };
  if (kind === 'unsupported' || /providerOperationUnsupported|不支持/i.test(message)) return {
    title: '当前来源不支持此语音操作', description: '在语音设置中选择系统、离线模型或支持该操作的云端服务商。',
  };
  if (/systemUnavailable|noAvailableSource|missing_model|missingModel|语言|模型/i.test(message)) return {
    title: '需要配置识别语言或模型', description: '当前语言的识别服务不可用。可在语音设置中更改语言、下载离线模型或选择云端来源。',
  };
  if (kind === 'busy') return {
    title: '音频设备正在使用中', description: '结束当前录音或播放后重试；可在语音设置中检查所选来源。',
  };
  return { title: '语音暂不可用', description: '请在语音设置中检查识别语言、音频来源和模型，然后重试。' };
}
