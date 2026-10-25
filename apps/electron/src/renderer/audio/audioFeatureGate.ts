import type { AudioOperationKindDto } from '@lingxi/bridge-client';
import type { AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import { resolveAudioLanguage, resolveAudioRoute } from '../../shared/generatedAudioConfiguration.js';
import type { NativeAudioSnapshot } from '../../shared/nativeAudio.js';
import { offlineVoiceModels } from '../../shared/voiceModelCatalog.js';
import { tr } from '../i18n/locale';

export interface AudioFeatureGate { visible: boolean; ready: boolean; reason?: string; reasonCode?: string }

/** Settings remain reachable even when a usage operation is unsupported. */
export function audioFeatureGate(configuration: AudioConfigurationV4, snapshot: NativeAudioSnapshot, operation: AudioOperationKindDto): AudioFeatureGate {
  const kind = operation === 'listen' ? 'recognition' : 'speech';
  const preference = kind === 'recognition' ? configuration.recognition : configuration.speech;
  if (preference.source !== 'provider' && !snapshot.capabilities?.supported_operations.includes(operation)) return { visible: false, ready: false, reason: tr('audio_operation_unsupported'), reasonCode: 'unsupported' };
  const route = resolveAudioRoute({
    kind, preference, language: resolveAudioLanguage(configuration.language, snapshot.localeTag),
    systemStatus: kind === 'speech' ? snapshot.voices.some((voice) => voice.source === 'system') ? 'available' : 'unavailable'
      : snapshot.permissions.speech === 'authorized' && snapshot.recognizerAvailable ? 'available'
        : snapshot.permissions.speech === 'not_determined' ? 'permissionRequired'
          : snapshot.permissions.speech === 'denied' || snapshot.permissions.speech === 'restricted' ? 'denied' : 'unavailable',
    offlineModels: offlineVoiceModels.map((entry) => ({ id: entry.id, kind: entry.kind === 'stt' ? 'recognition' : 'speech', languages: entry.languages, installed: snapshot.models.some((model) => model.modelId === entry.id && model.state.type === 'ready') })),
    sessionContext: snapshot.sessionContext, providerCapabilities: snapshot.providerCapabilities,
  });
  if (route.reason === 'providerOperationUnsupported') return { visible: false, ready: false, reason: tr('audio_operation_unsupported'), reasonCode: route.reason };
  const permissionMissing = operation === 'listen' && snapshot.permissions.microphone !== 'granted';
  // Provider transcription uses native capture; provider speech uses native playback.
  // Their model readiness comes from the hosted route, not the helper's recognizer.
  const operationReadiness = preference.source === 'provider'
    ? operation === 'listen'
      ? snapshot.capabilities?.readiness.find((entry) => entry.operation === 'record')?.state
      : snapshot.activeRecordingCount > 0 || snapshot.activePlaybackCount > 0 ? 'busy' : 'ready'
    : snapshot.capabilities?.readiness.find((entry) => entry.operation === operation)?.state;
  const ready = route.status === 'ready' && !permissionMissing && (!operationReadiness || operationReadiness === 'ready');
  const deviceReason = operationReadiness === 'busy' ? tr('desktop_audio_gate_busy')
    : operationReadiness === 'missing_model' ? tr('desktop_audio_gate_missing_model')
      : operationReadiness === 'needs_permission' ? tr('voice_needs_permission_setup')
        : tr('desktop_audio_gate_unavailable');
  const reasonCode = permissionMissing ? 'permission_denied' : route.reason === 'ready'
    ? operationReadiness === 'busy' ? 'busy' : operationReadiness === 'needs_permission' ? 'permission_denied' : 'systemUnavailable'
    : /offline|language|CompatibleOfflineModel/i.test(route.reason) ? 'missing_model' : route.reason;
  return { visible: true, ready, ...(ready ? {} : {
    reason: permissionMissing ? tr('desktop_audio_gate_microphone') : route.reason === 'ready' ? deviceReason : tr('desktop_audio_gate_unavailable'),
    reasonCode,
  }) };
}

export function audioConversationGate(configuration: AudioConfigurationV4, snapshot: NativeAudioSnapshot): AudioFeatureGate {
  if (configuration.conversation.mode === 'realtime') return { visible: snapshot.realtimeReadiness?.supported === true, ready: snapshot.realtimeReadiness?.ready === true, reason: snapshot.realtimeReadiness?.reason, reasonCode: snapshot.realtimeReadiness?.reason };
  const input = audioFeatureGate(configuration, snapshot, 'listen');
  const output = audioFeatureGate(configuration, snapshot, 'speak');
  return { visible: input.visible && output.visible, ready: input.ready && output.ready, reason: input.reason ?? output.reason, reasonCode: input.reasonCode ?? output.reasonCode };
}

/** A temporary failed attempt does not change the device's declared support. */
export function voiceStateAfterAudioFailure(kind: string): 'idle' | 'denied' | 'unsupported' {
  if (kind === 'permission_denied') return 'denied';
  if (kind === 'unsupported') return 'unsupported';
  return 'idle';
}

export interface AudioSetupGuidance { title: string; description: string; titleKey: string; descriptionKey: string }

function guidance(titleKey: string, descriptionKey: string): AudioSetupGuidance {
  return { title: tr(titleKey), description: tr(descriptionKey), titleKey, descriptionKey };
}

/** Offer an actionable setting, without exposing transport or route internals. */
export function audioSetupGuidance(kind: string, message = ''): AudioSetupGuidance | null {
  if (kind === 'cancelled') return null;
  if (kind === 'permission_denied' || /授权|permission|denied/i.test(message)) return guidance('audio_guide_permission_title', 'desktop_audio_guide_permission_detail');
  if (/foreground/i.test(message)) return guidance('desktop_audio_guide_foreground_title', 'desktop_audio_guide_foreground_detail');
  if (/\b429\b/.test(message)) return guidance('audio_guide_quota_title', 'desktop_audio_guide_quota_detail');
  if (kind === 'no_speech' || /no speech|no-speech/i.test(message)) return guidance('voice_no_speech_recognized', 'desktop_audio_guide_no_speech_detail');
  if (kind === 'unsupported' || message === 'unsupported' || /providerOperationUnsupported|不支持/i.test(message)) return guidance('audio_operation_unsupported', 'desktop_audio_guide_unsupported_detail');
  if (/systemUnavailable|noAvailableSource|missing_model|missingModel|语言|模型/i.test(message)) return guidance('audio_guide_configuration_title', 'desktop_audio_guide_model_detail');
  if (kind === 'busy' || message === 'busy') return guidance('desktop_audio_gate_busy', 'desktop_audio_guide_busy_detail');
  return guidance('audio_guide_retry_title', 'audio_guide_retry_detail');
}
