import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEFAULT_VOICE_SELECTION,
  LANGUAGE_AUTO,
  defaultVoicePreferences,
  normalizeVoiceSelection,
  parseVoicePreferences,
  type VoicePreferences,
} from '../src/renderer/audio/preferences';

// These expectations are lifted directly from iOS's
// `VoicePreferencesSnapshot.normalizeVoiceSelection` (VoiceRuntimeConfiguration.swift)
// and Android's `normalizeVoiceSelection` (VoiceSettingsRepository.kt) —
// all three platforms must agree on the resulting string.
test('voice selection normalizes exactly as iOS and Android do', () => {
  assert.equal(normalizeVoiceSelection(undefined), 'system:default');
  assert.equal(normalizeVoiceSelection(''), 'system:default');
  assert.equal(normalizeVoiceSelection('   '), 'system:default');
  assert.equal(normalizeVoiceSelection('default'), 'system:default');
  assert.equal(normalizeVoiceSelection('  Alex  '), 'system:Alex');
  assert.equal(normalizeVoiceSelection('Alex'), 'system:Alex');
  assert.equal(normalizeVoiceSelection('system:Alex'), 'system:Alex');
  assert.equal(normalizeVoiceSelection('sherpa:vits-zh:0'), 'sherpa:vits-zh:0');
  assert.equal(normalizeVoiceSelection('sherpa:m:v'), 'sherpa:m:v');
  assert.equal(DEFAULT_VOICE_SELECTION, 'system:default');
});

test('rate is clamped to the mobile range, never rejected', () => {
  assert.equal(parseVoicePreferences({ rate: 9 }).rate, 2);
  assert.equal(parseVoicePreferences({ rate: 2.01 }).rate, 2);
  assert.equal(parseVoicePreferences({ rate: 2.0 }).rate, 2);
  assert.equal(parseVoicePreferences({ rate: 0.5 }).rate, 0.5);
  assert.equal(parseVoicePreferences({ rate: 0.49 }).rate, 0.5);
  assert.equal(parseVoicePreferences({ rate: 0.1 }).rate, 0.5);
  assert.equal(parseVoicePreferences({ rate: 1.25 }).rate, 1.25);
});

test('rate defaults to 1.0 when absent or unparseable, not merely clamped', () => {
  assert.equal(parseVoicePreferences({}).rate, 1);
  assert.equal(parseVoicePreferences({ rate: 'fast' }).rate, 1);
  assert.equal(parseVoicePreferences({ rate: null }).rate, 1);
  assert.equal(parseVoicePreferences({ rate: Number.NaN }).rate, 1);
});

test('a fresh install defaults to automatic, matching mobile', () => {
  const fresh: VoicePreferences = parseVoicePreferences({});
  assert.equal(fresh.schemaVersion, 2);
  assert.equal(fresh.recognitionMode, 'automatic');
  assert.equal(fresh.language, LANGUAGE_AUTO);
  assert.equal(fresh.voiceSelection, 'system:default');
  assert.equal(fresh.rate, 1);
  assert.equal(fresh.autoPlayReplies, false);
});

test('schemaVersion is always 2, regardless of what was supplied', () => {
  assert.equal(parseVoicePreferences({ schemaVersion: 1 }).schemaVersion, 2);
  assert.equal(parseVoicePreferences({ schemaVersion: 999 }).schemaVersion, 2);
  assert.equal(parseVoicePreferences(undefined).schemaVersion, 2);
});

test('recognitionMode preserves localOnly and falls back to automatic for everything else, including the Swift case name', () => {
  assert.equal(parseVoicePreferences({ recognitionMode: 'localOnly' }).recognitionMode, 'localOnly');
  assert.equal(parseVoicePreferences({ recognitionMode: 'automatic' }).recognitionMode, 'automatic');
  // 'onDevice' is the Swift *case name*, never a persisted value on either
  // mobile platform — it must NOT be treated as the on-device mode here.
  assert.equal(parseVoicePreferences({ recognitionMode: 'onDevice' }).recognitionMode, 'automatic');
  assert.equal(parseVoicePreferences({ recognitionMode: undefined }).recognitionMode, 'automatic');
  assert.equal(parseVoicePreferences({ recognitionMode: 'telepathy' }).recognitionMode, 'automatic');
  assert.equal(parseVoicePreferences({ recognitionMode: 42 }).recognitionMode, 'automatic');
});

test('language trims and normalizes case-insensitive "auto" to the canonical form', () => {
  assert.equal(parseVoicePreferences({ language: 'AUTO' }).language, 'auto');
  assert.equal(parseVoicePreferences({ language: 'Auto' }).language, 'auto');
  assert.equal(parseVoicePreferences({ language: '  ' }).language, 'auto');
  assert.equal(parseVoicePreferences({ language: '' }).language, 'auto');
  assert.equal(parseVoicePreferences({ language: undefined }).language, 'auto');
  assert.equal(parseVoicePreferences({ language: 'zh-CN' }).language, 'zh-CN');
  assert.equal(parseVoicePreferences({ language: '  zh-CN  ' }).language, 'zh-CN');
});

test('voiceSelection is normalized the same way through the full parser', () => {
  assert.equal(parseVoicePreferences({ voiceSelection: '' }).voiceSelection, 'system:default');
  assert.equal(parseVoicePreferences({ voiceSelection: 'default' }).voiceSelection, 'system:default');
  assert.equal(parseVoicePreferences({ voiceSelection: 'Alex' }).voiceSelection, 'system:Alex');
  assert.equal(parseVoicePreferences({ voiceSelection: 'system:Alex' }).voiceSelection, 'system:Alex');
  assert.equal(parseVoicePreferences({ voiceSelection: 'sherpa:m:v' }).voiceSelection, 'sherpa:m:v');
  assert.equal(parseVoicePreferences({ voiceSelection: undefined }).voiceSelection, 'system:default');
  assert.equal(parseVoicePreferences({ voiceSelection: 123 }).voiceSelection, 'system:default');
});

test('auto-play-replies persists and defaults false', () => {
  assert.equal(defaultVoicePreferences().autoPlayReplies, false);
  assert.equal(parseVoicePreferences({}).autoPlayReplies, false);
  assert.equal(parseVoicePreferences({ autoPlayReplies: true }).autoPlayReplies, true);
  assert.equal(parseVoicePreferences({ autoPlayReplies: false }).autoPlayReplies, false);
  assert.equal(parseVoicePreferences({ autoPlayReplies: 'yes' }).autoPlayReplies, false);
});

test('parseVoicePreferences never throws on hostile input', () => {
  assert.doesNotThrow(() => parseVoicePreferences(undefined));
  assert.doesNotThrow(() => parseVoicePreferences(null));
  assert.doesNotThrow(() => parseVoicePreferences('garbage'));
  assert.doesNotThrow(() => parseVoicePreferences(42));
  assert.doesNotThrow(() => parseVoicePreferences([]));
  assert.doesNotThrow(() => parseVoicePreferences({ recognitionMode: { nested: true } }));
});
