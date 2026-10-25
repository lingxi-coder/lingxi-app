import assert from 'node:assert/strict';
import { test } from 'node:test';
import { audioFeatureGate, audioConversationGate, voiceStateAfterAudioFailure, audioSetupGuidance } from '../src/renderer/audio/audioFeatureGate';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import { defaultNativeAudioSnapshot } from '../src/shared/nativeAudio';
import { tr } from '../src/renderer/i18n/locale';

test('unsupported device operations hide usage entries while configuration remains data', () => {
  const configuration = audioConfigurationDefaults();
  assert.deepEqual(audioFeatureGate(configuration, defaultNativeAudioSnapshot(), 'listen'), { visible: false, ready: false, reason: tr('audio_operation_unsupported'), reasonCode: 'unsupported' });
  assert.equal(configuration.recognition.source, 'automatic');
});

test('recoverable microphone permission keeps input visible and supplies an action reason', () => {
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.capabilities!.supported_operations = ['listen'];
  snapshot.permissions.microphone = 'prompt';
  snapshot.permissions.speech = 'authorized';
  snapshot.recognizerAvailable = true;
  const gate = audioFeatureGate(audioConfigurationDefaults(), snapshot, 'listen');
  assert.equal(gate.visible, true);
  assert.equal(gate.ready, false);
  assert.equal(gate.reason, tr('desktop_audio_gate_microphone'));
  assert.equal(gate.reasonCode, 'permission_denied');
});

test('provider unsupported hides input even when native recognition is ready', () => {
  const config = audioConfigurationDefaults();
  config.recognition.source = 'provider';
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.sessionContext = { sessionId: 's', profileId: 'custom-openai' };
  snapshot.providerCapabilities = [{ profileId: 'custom-openai', providerId: 'openai', kind: 'recognition', supported: false, readiness: 'unsupported', defaultModelId: null, modelIds: [] }];
  assert.equal(audioFeatureGate(config, snapshot, 'listen').visible, false);
});

test('unreachable provider remains visible with configuration feedback and realtime never infers readiness', () => {
  const config = audioConfigurationDefaults();
  config.speech.source = 'provider';
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.sessionContext = { sessionId: 's', profileId: 'p' };
  snapshot.providerCapabilities = [{ profileId: 'p', providerId: 'openai', kind: 'speech', supported: true, readiness: 'unreachable', defaultModelId: 'tts', modelIds: ['tts'] }];
  assert.equal(audioFeatureGate(config, snapshot, 'speak').visible, true);
  assert.equal(audioFeatureGate(config, snapshot, 'speak').reason, tr('desktop_audio_gate_unavailable'));
  assert.equal(audioFeatureGate(config, snapshot, 'speak').reasonCode, 'providerUnreachable');
  config.conversation.mode = 'realtime';
  assert.deepEqual(audioConversationGate(config, snapshot), { visible: false, ready: false, reason: undefined, reasonCode: undefined });
});

test('temporary foreground and service failures allow a later normal retry', () => {
  assert.equal(voiceStateAfterAudioFailure('unavailable'), 'idle');
  assert.equal(voiceStateAfterAudioFailure('busy'), 'idle');
  assert.equal(voiceStateAfterAudioFailure('unsupported'), 'unsupported');
  assert.equal(voiceStateAfterAudioFailure('permission_denied'), 'denied');
});

test('current language readiness overrides a stale available recognizer snapshot', () => {
  const config = audioConfigurationDefaults();
  config.language = 'zh-CN';
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.capabilities!.supported_operations = ['listen'];
  snapshot.capabilities!.readiness = [{ operation: 'listen', state: 'unavailable' }];
  snapshot.permissions.microphone = 'granted';
  snapshot.permissions.speech = 'authorized';
  snapshot.recognizerAvailable = true;
  const gate = audioFeatureGate(config, snapshot, 'listen');
  assert.equal(gate.ready, false);
  assert.equal(gate.reason, tr('desktop_audio_gate_unavailable'));
  assert.equal(gate.reasonCode, 'systemUnavailable');
  assert.equal(audioSetupGuidance('unavailable', gate.reasonCode)?.titleKey, 'audio_guide_configuration_title');
});

test('audio guidance distinguishes configuration, permissions, quota and retryable input failures', () => {
  assert.equal(audioSetupGuidance('cancelled'), null);
  assert.equal(audioSetupGuidance('unsupported')!.description, tr('desktop_audio_guide_unsupported_detail'));
  assert.equal(audioSetupGuidance('permission_denied')!.titleKey, 'audio_guide_permission_title');
  assert.equal(audioSetupGuidance('unavailable', 'systemUnavailable')!.titleKey, 'audio_guide_configuration_title');
  assert.equal(audioSetupGuidance('unavailable', 'No speech detected')!.descriptionKey, 'desktop_audio_guide_no_speech_detail');
  assert.equal(audioSetupGuidance('unavailable', 'audio provider returned HTTP 429')!.descriptionKey, 'desktop_audio_guide_quota_detail');
  assert.doesNotMatch(JSON.stringify(audioSetupGuidance('unavailable', 'private-canary request identifier')), /private-canary/);
});

test('a supported provider recognizer uses native capture readiness independently of Apple Speech', () => {
  const config = audioConfigurationDefaults();
  config.recognition.source = 'provider';
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.permissions.microphone = 'granted';
  snapshot.capabilities!.supported_operations = ['record', 'listen'];
  snapshot.capabilities!.readiness = [{ operation: 'record', state: 'ready' }, { operation: 'listen', state: 'unavailable' }];
  snapshot.sessionContext = { sessionId: 's', profileId: 'p' };
  snapshot.providerCapabilities = [{ profileId: 'p', providerId: 'openai', kind: 'recognition', supported: true, readiness: 'ready', defaultModelId: 'stt', modelIds: ['stt'] }];
  assert.equal(audioFeatureGate(config, snapshot, 'listen').ready, true);
});
