import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  saveVoiceDraftIfUnchanged,
  voicePageModel,
  voicePreviewRequest,
  voiceSelectionFromValue,
  voiceSelectionOptionValue,
} from '../src/renderer/components/settings/pages/Voice';
import { audioConfigurationDefaults, type AudioConfigurationV3 } from '../src/shared/generatedAudioConfiguration';
import { defaultNativeAudioSnapshot, type NativeAudioSnapshot } from '../src/shared/nativeAudio';

function prefs(overrides: Partial<AudioConfigurationV3> = {}): AudioConfigurationV3 {
  return { ...audioConfigurationDefaults(), ...overrides };
}

function snapshot(overrides: Partial<NativeAudioSnapshot> = {}): NativeAudioSnapshot {
  return {
    ...defaultNativeAudioSnapshot(),
    helper: { state: 'running' },
    permissions: { microphone: 'granted', speech: 'authorized' },
    localeTag: 'zh-CN',
    recognizerAvailable: true,
    voices: [{
      id: 'system:com.apple.voice.compact.zh-CN.Tingting', label: 'Tingting', languageTag: 'zh-CN',
      source: 'system', familyId: 'system', isDefault: true,
    }],
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise; });
  return { promise, resolve };
}

test('automatic recognition preview resolves the available system backend and locale', () => {
  const model = voicePageModel(prefs(), snapshot({
    recognition: { requestedMode: 'automatic', effectiveBackend: 'apple', effectiveLanguage: 'zh-CN', detail: 'ready' },
  }));
  assert.equal(model.recognitionRoute.effective?.source, 'system');
  assert.equal(model.recognitionRoute.effective?.modelId, null);
  assert.match(model.actualRecognition, /macOS 系统识别/);
});

test('independent explicit offline sources report missing models without falling back to system', () => {
  const model = voicePageModel(prefs({
    recognition: { source: 'offline', offlineModelId: 'sherpa.moonshine-tiny-en' },
    speech: { source: 'offline', offlineModelId: 'sherpa.melo-zh-en', voice: { source: 'offline', modelId: 'sherpa.melo-zh-en', id: 'ghost' } },
  }), snapshot());
  assert.equal(model.recognitionRoute.status, 'unavailable');
  assert.equal(model.recognitionRoute.effective, null);
  assert.equal(model.speechRoute.status, 'unavailable');
  assert.equal(model.speechRoute.effective, null);
  assert.ok(model.notices.some((notice) => /识别偏好不可用/.test(notice)));
  assert.ok(model.notices.some((notice) => /朗读偏好不可用/.test(notice)));
});

test('voice page shows requested, preview, and actual speech independently', () => {
  const config = prefs({
    speech: { source: 'system', offlineModelId: null, voice: { source: 'system', id: 'com.apple.voice.compact.zh-CN.Tingting' } },
    rate: 1.5,
    autoPlayReplies: true,
  });
  const model = voicePageModel(config, snapshot({
    activity: 'speaking',
    playback: { effectiveVoiceId: 'system:com.apple.voice.compact.zh-CN.Tingting', effectiveVoiceLabel: 'Tingting' },
  }));
  assert.equal(model.speechRoute.requested.voice?.id, 'com.apple.voice.compact.zh-CN.Tingting');
  assert.equal(model.speechRoute.effective?.voiceId, 'com.apple.voice.compact.zh-CN.Tingting');
  assert.equal(model.actualSpeech, 'Tingting');
  assert.equal(config.rate, 1.5);
  assert.equal(config.autoPlayReplies, true);
});

test('offline voice selection value matches the Sherpa catalog option and round-trips', () => {
  const modelId = 'sherpa.melo-zh-en';
  const voiceId = 'ZH-Female-1';
  const voice = { source: 'offline' as const, modelId, id: voiceId };
  const catalogVoice = {
    id: `sherpa:${modelId}:${voiceId}`,
    label: '中文女声',
    languageTag: 'zh-CN',
    source: 'sherpa' as const,
    familyId: modelId,
  };

  const value = voiceSelectionOptionValue(voice);

  assert.equal(value, catalogVoice.id);
  assert.deepEqual(voiceSelectionFromValue(value, [catalogVoice]), voice);
});

test('a voice save leaves later edits dirty until they are saved too', async () => {
  const saveResponse = deferred<void>();
  const submitted = prefs({ rate: 1.1 });
  let editRevision = 4;
  let persisted: AudioConfigurationV3 | null = null;
  const saving = saveVoiceDraftIfUnchanged(
    submitted,
    4,
    () => editRevision,
    async (configuration) => {
      persisted = configuration;
      await saveResponse.promise;
    },
  );

  editRevision += 1;
  saveResponse.resolve();

  assert.equal(await saving, false);
  assert.equal(persisted, submitted);
  assert.equal(editRevision, 5);
});

test('a unique legacy system voice name resolves for display without mutating the saved config', () => {
  const saved = prefs({
    speech: { source: 'system', offlineModelId: null, voice: { source: 'system', id: 'Tingting' } },
  });

  const model = voicePageModel(saved, snapshot());

  assert.equal(model.speechRoute.status, 'ready');
  assert.equal(model.displayConfiguration.speech.voice?.id, 'com.apple.voice.compact.zh-CN.Tingting');
  assert.deepEqual(saved.speech.voice, { source: 'system', id: 'Tingting' });
});

test('ambiguous legacy system voice names remain unresolved instead of selecting an arbitrary voice', () => {
  const saved = prefs({
    speech: { source: 'system', offlineModelId: null, voice: { source: 'system', id: 'Tingting' } },
  });
  const model = voicePageModel(saved, snapshot({
    voices: [
      { id: 'system:com.apple.voice.compact.zh-CN.Tingting', label: 'Tingting', languageTag: 'zh-CN', source: 'system', familyId: 'system' },
      { id: 'system:com.apple.voice.compact.zh-TW.Tingting', label: 'Tingting', languageTag: 'zh-TW', source: 'system', familyId: 'system' },
    ],
  }));

  assert.equal(model.displayConfiguration.speech.voice?.id, 'Tingting');
  assert.equal(model.speechRoute.status, 'unavailable');
});

test('voice preview carries the unsaved speech source, model, and selected voice', () => {
  const draft = prefs({
    language: 'zh-CN',
    rate: 1.35,
    speech: {
      source: 'offline',
      offlineModelId: 'sherpa.melo-zh-en',
      voice: { source: 'offline', modelId: 'sherpa.melo-zh-en', id: 'ZH-Female-1' },
    },
  });

  const request = voicePreviewRequest(draft, 'en-US');

  assert.deepEqual(request.configuration, draft);
  assert.equal(request.operation.type, 'speak');
  assert.equal(request.operation.language, 'zh-CN');
  assert.equal(request.operation.rate, 1.35);
  assert.equal(request.operation.voice, 'sherpa:sherpa.melo-zh-en:ZH-Female-1');
});

test('voice preview carries an unsaved offline route when no specific voice is selected', () => {
  const draft = prefs({ speech: { source: 'offline', offlineModelId: 'sherpa.melo-zh-en', voice: null } });

  const request = voicePreviewRequest(draft, 'en-US');

  assert.equal(request.operation.voice, undefined);
  assert.deepEqual(request.configuration.speech, draft.speech);
});

test('the page saves explicit v3 preferences and executes model/audio work through host routes', () => {
  const source = readFileSync(new URL('../src/renderer/components/settings/pages/Voice.tsx', import.meta.url), 'utf8');
  for (const command of ['install_model', 'cancel_model', 'remove_model', 'request_authorization']) assert.match(source, new RegExp(command));
  assert.match(source, /audioExecute/);
  assert.match(source, /audioExecute\(operation, undefined, configurationOverride\)/);
  assert.match(source, /voicePreviewRequest\(displayDraft, snapshot\.localeTag/);
  assert.match(source, /voiceSelectionOptionValue\(displayDraft\.speech\.voice, displayDraft\.speech\.offlineModelId\)/);
  assert.match(source, /saveVoiceDraftIfUnchanged\(/);
  assert.match(source, /draftEditRevision\.current \+= 1/);
  assert.match(source, /if \(unchanged\) setDirty\(false\)/);
  assert.match(source, /voiceRevision/);
  assert.match(source, /catch \(cause\)/);
  assert.match(source, /bridge\.setVoicePreferences\(configuration,/);
  assert.match(source, /保存语音偏好/);
  assert.match(source, /实际：/);
  assert.doesNotMatch(source, /speechSynthesis|navigator\.mediaDevices/);
});
