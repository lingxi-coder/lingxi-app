import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  defaultVoicePreferences,
  isLegacySystemVoiceAlias,
  parseVoicePreferences,
  type VoicePreferences,
} from '../src/shared/voicePreferences';

test('fresh Desktop audio preferences use the v3 independent-source defaults', () => {
  const fresh: VoicePreferences = defaultVoicePreferences();
  assert.deepEqual(fresh, {
    schemaVersion: 3,
    recognition: { source: 'automatic', offlineModelId: null },
    speech: { source: 'automatic', offlineModelId: null, voice: null },
    language: 'auto',
    rate: 1,
    autoPlayReplies: false,
  });
});

test('legacy localOnly recognition migrates to explicit offline without changing speech preference', () => {
  assert.deepEqual(parseVoicePreferences({ recognitionMode: 'localOnly', language: 'ZH-cn', rate: 0.1 }), {
    schemaVersion: 3,
    recognition: { source: 'offline', offlineModelId: null },
    speech: { source: 'automatic', offlineModelId: null, voice: null },
    language: 'ZH-cn',
    rate: 0.5,
    autoPlayReplies: false,
  });
});

test('legacy voice aliases stay explicit for one-time resolution against the helper catalog', () => {
  const system = parseVoicePreferences({ voiceSelection: 'system:Tingting' });
  assert.deepEqual(system.speech, {
    source: 'system', offlineModelId: null, voice: { source: 'system', id: 'Tingting' },
  });
  const sherpa = parseVoicePreferences({ voiceSelection: 'sherpa:vits-zh:0' });
  assert.deepEqual(sherpa.speech, {
    source: 'offline', offlineModelId: 'vits-zh', voice: { source: 'offline', modelId: 'vits-zh', id: '0' },
  });
  assert.equal(isLegacySystemVoiceAlias('system:Tingting'), true);
  assert.equal(isLegacySystemVoiceAlias('system:com.apple.voice.compact.zh-CN.Tingting'), false);
});

test('unknown source/model/voice requests are preserved as explicit unavailable preferences', () => {
  const value = parseVoicePreferences({
    schemaVersion: 3,
    recognition: { source: 'future-stt', offlineModelId: 'unknown-model' },
    speech: { source: 'offline', offlineModelId: 'missing-tts', voice: { source: 'offline', modelId: 'missing-tts', id: 'ghost' } },
  });
  assert.equal(value.recognition.source, 'future-stt');
  assert.equal(value.recognition.offlineModelId, 'unknown-model');
  assert.equal(value.speech.offlineModelId, 'missing-tts');
  assert.deepEqual(value.speech.voice, { source: 'offline', modelId: 'missing-tts', id: 'ghost' });
});

test('v3 normalization clamps rate and sanitizes hostile outer values without changing source intent', () => {
  assert.equal(parseVoicePreferences({ schemaVersion: 3, rate: 9 }).rate, 2);
  assert.equal(parseVoicePreferences({ schemaVersion: 3, rate: 0.1 }).rate, 0.5);
  assert.doesNotThrow(() => parseVoicePreferences(undefined));
  assert.doesNotThrow(() => parseVoicePreferences(null));
  assert.doesNotThrow(() => parseVoicePreferences('garbage'));
  assert.equal(parseVoicePreferences({ schemaVersion: 3, recognition: { source: 'offline' } }).recognition.source, 'offline');
});
