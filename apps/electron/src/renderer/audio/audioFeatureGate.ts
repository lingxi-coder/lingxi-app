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
  const operationReadiness = snapshot.capabilities?.readiness.find((entry) => entry.operation === operation)?.state;
  const ready = route.status === 'ready' && !permissionMissing && operationReadiness !== 'busy';
  return { visible: true, ready, ...(ready ? {} : { reason: permissionMissing ? '需要麦克风授权，请在语音设置中授权。' : route.reason === 'ready' ? '音频设备正在使用中。' : `音频暂不可用：${route.reason}。请检查语音设置。` }) };
}

export function audioConversationGate(configuration: AudioConfigurationV4, snapshot: NativeAudioSnapshot): AudioFeatureGate {
  if (configuration.conversation.mode === 'realtime') return { visible: snapshot.realtimeReadiness?.supported === true, ready: snapshot.realtimeReadiness?.ready === true, reason: snapshot.realtimeReadiness?.reason };
  const input = audioFeatureGate(configuration, snapshot, 'listen');
  const output = audioFeatureGate(configuration, snapshot, 'speak');
  return { visible: input.visible && output.visible, ready: input.ready && output.ready, reason: input.reason ?? output.reason };
}
