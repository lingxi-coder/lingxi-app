import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  defaultVoicePreferences,
  parseVoicePreferences,
  type VoicePreferences,
} from '../src/shared/voicePreferences';

test('fresh Desktop audio preferences use the v4 independent-source defaults', () => {
  const fresh: VoicePreferences = defaultVoicePreferences();
  assert.deepEqual(fresh, {
    schemaVersion: 4,
    conversation: { mode: 'agent', interaction: 'turn_based', cloud: { binding: 'follow_session', profileId: null, modelId: null }, voice: null },
    recognition: { source: 'automatic', offlineModelId: null, cloud: { binding: 'follow_session', profileId: null, modelId: null } },
    speech: { source: 'automatic', offlineModelId: null, voice: null, cloud: { binding: 'follow_session', profileId: null, modelId: null } },
    language: 'auto',
    rate: 1,
    autoPlayReplies: false,
  });
});

test('unknown source/model/voice requests are preserved as explicit unavailable preferences', () => {
  const value = parseVoicePreferences({
    schemaVersion: 4,
    recognition: { source: 'future-stt', offlineModelId: 'unknown-model' },
    speech: { source: 'offline', offlineModelId: 'missing-tts', voice: { source: 'offline', modelId: 'missing-tts', id: 'ghost' } },
  });
  assert.equal(value.recognition.source, 'future-stt');
  assert.equal(value.recognition.offlineModelId, 'unknown-model');
  assert.equal(value.speech.offlineModelId, 'missing-tts');
  assert.deepEqual(value.speech.voice, { source: 'offline', modelId: 'missing-tts', id: 'ghost' });
});

test('v4 normalization clamps rate and sanitizes hostile outer values without changing source intent', () => {
  assert.equal(parseVoicePreferences({ schemaVersion: 4, rate: 9 }).rate, 2);
  assert.equal(parseVoicePreferences({ schemaVersion: 4, rate: 0.1 }).rate, 0.5);
  assert.doesNotThrow(() => parseVoicePreferences(undefined));
  assert.doesNotThrow(() => parseVoicePreferences(null));
  assert.doesNotThrow(() => parseVoicePreferences('garbage'));
  assert.equal(parseVoicePreferences({ schemaVersion: 4, recognition: { source: 'offline' } }).recognition.source, 'offline');
});


test('old versions and absent-version aliases are ignored rather than migrated', () => {
  for (const input of [{ schemaVersion: 3, recognition: { source: 'offline' }, language: 'fr-FR', rate: 1.75 }, { recognitionMode: 'localOnly', voiceSelection: 'system:Tingting', rate: 0.5 }]) {
    assert.deepEqual(parseVoicePreferences(input), defaultVoicePreferences());
  }
  const current = parseVoicePreferences({ schemaVersion: 4, speech: { source: 'system', voice: 'Tingting' }, voiceSelection: 'system:Tingting' });
  assert.equal(current.speech.voice, null);
});
