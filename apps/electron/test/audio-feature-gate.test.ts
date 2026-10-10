import assert from 'node:assert/strict';
import { test } from 'node:test';
import { audioFeatureGate, audioConversationGate } from '../src/renderer/audio/audioFeatureGate';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import { defaultNativeAudioSnapshot } from '../src/shared/nativeAudio';

test('unsupported device operations hide usage entries while configuration remains data', () => {
  const configuration = audioConfigurationDefaults();
  assert.deepEqual(audioFeatureGate(configuration, defaultNativeAudioSnapshot(), 'listen'), { visible: false, ready: false, reason: '当前设备不支持此音频操作。' });
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
  assert.match(gate.reason!, /授权/);
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
  assert.match(audioFeatureGate(config, snapshot, 'speak').reason!, /providerUnreachable/);
  config.conversation.mode = 'realtime';
  assert.deepEqual(audioConversationGate(config, snapshot), { visible: false, ready: false, reason: undefined });
});
