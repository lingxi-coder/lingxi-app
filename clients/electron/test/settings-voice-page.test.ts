import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { canonicalVoicePreferencesForSave, voicePageModel } from '../src/renderer/components/settings/pages/Voice';
import { defaultNativeAudioSnapshot, type NativeAudioSnapshot } from '../src/shared/nativeAudio';
import { defaultVoicePreferences, type VoicePreferences } from '../src/shared/voicePreferences';

function prefs(overrides: Partial<VoicePreferences> = {}): VoicePreferences {
  return { ...defaultVoicePreferences(), ...overrides };
}

function snapshot(overrides: Partial<NativeAudioSnapshot> = {}): NativeAudioSnapshot {
  return {
    ...defaultNativeAudioSnapshot(),
    helper: { state: 'running' },
    permissions: { microphone: 'granted', speech: 'authorized' },
    localeTag: 'zh-CN',
    recognizerAvailable: true,
    voices: [{
      id: 'system:com.apple.voice.compact.zh-CN.Tingting',
      label: 'Tingting',
      languageTag: 'zh-CN',
      source: 'system',
      familyId: 'system',
      isDefault: true,
    }],
    ...overrides,
  };
}

test('automatic mode reports the native Apple backend without any provider state', () => {
  const model = voicePageModel(prefs(), snapshot({
    recognition: {
      requestedMode: 'automatic', effectiveBackend: 'apple', effectiveLanguage: 'zh-CN', detail: 'macOS Speech is ready.',
    },
  }));
  assert.equal(model.effectiveBackend, 'apple');
  assert.equal(model.effectiveLanguage, 'zh-CN');
  assert.equal(model.recognizerAvailable, true);
  assert.equal(model.notices.some((notice) => /Provider/.test(notice)), false);
});

test('localOnly is always selectable and reports a missing language model honestly', () => {
  const model = voicePageModel(prefs({ recognitionMode: 'localOnly' }), snapshot({ models: [] }));
  assert.equal(model.recognitionOptions.find((option) => option.id === 'localOnly')?.disabled, false);
  assert.equal(model.effectiveBackend, 'unavailable');
  assert.ok(model.notices.some((notice) => /离线识别模型/.test(notice)));
});

test('an installed language model makes localOnly resolve to Sherpa', () => {
  const model = voicePageModel(prefs({ recognitionMode: 'localOnly' }), snapshot({
    models: [{ modelId: 'sherpa.zipformer-zh-14m-mobile', state: { type: 'ready' } }],
  }));
  assert.equal(model.effectiveBackend, 'sherpa');
});

test('requested and effective voice are kept distinct and autoplay remains persisted', () => {
  const model = voicePageModel(prefs({ voiceSelection: 'sherpa:missing:voice', autoPlayReplies: true, rate: 1.5 }), snapshot({
    playback: {
      requestedVoiceSelection: 'sherpa:missing:voice',
      effectiveVoiceId: 'system:com.apple.voice.compact.zh-CN.Tingting',
      effectiveVoiceLabel: 'Tingting',
    },
  }));
  assert.equal(model.voiceSelection, 'sherpa:missing:voice');
  assert.equal(model.effectiveVoiceLabel, 'Tingting');
  assert.equal(model.autoPlayReplies, true);
  assert.equal(model.rate, 1.5);
  assert.ok(model.notices.some((notice) => /替代音色/.test(notice)));
});

test('legacy system voice names canonicalize to stable identifiers on save', () => {
  const current = snapshot();
  assert.equal(
    canonicalVoicePreferencesForSave(prefs({ voiceSelection: 'system:Tingting' }), current).voiceSelection,
    'system:com.apple.voice.compact.zh-CN.Tingting',
  );
  assert.equal(
    canonicalVoicePreferencesForSave(prefs({ voiceSelection: 'system:Missing Voice' }), current).voiceSelection,
    'system:default',
  );
});

test('the page exposes model lifecycle, preview, permissions, and no provider transcription dependency', () => {
  const source = readFileSync(new URL('../src/renderer/components/settings/pages/Voice.tsx', import.meta.url), 'utf8');
  for (const command of ['install_model', 'cancel_model', 'remove_model', 'request_authorization']) {
    assert.match(source, new RegExp(command));
  }
  assert.match(source, /试听/);
  assert.match(source, /speech_recognition/);
  assert.match(source, /系统识别器/);
  assert.match(source, /重试原生服务/);
  assert.doesNotMatch(source, /providerCredentials|transcriptionCapable|activeVoiceProviderId/);
});
