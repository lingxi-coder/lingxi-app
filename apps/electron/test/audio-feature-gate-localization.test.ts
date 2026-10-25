import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { audioFeatureGate, audioSetupGuidance } from '../src/renderer/audio/audioFeatureGate';
import { AudioSetupGuide } from '../src/renderer/components/AudioSetupGuide';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import { defaultNativeAudioSnapshot } from '../src/shared/nativeAudio';

(globalThis as { React?: typeof React }).React = React;

function inLocale<T>(locale: DesktopLocale, action: () => T): T {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try { return action(); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

function textHTML(text: string): string {
  return renderToStaticMarkup(React.createElement(React.Fragment, null, text));
}

test('saved audio guidance re-renders in every selected language without reclassifying raw errors', () => {
  const saved = inLocale('zh-Hans', () => audioSetupGuidance('unavailable', 'HTTP 429 private-canary'))!;
  for (const locale of DESKTOP_LOCALES) {
    const html = inLocale(locale, () => renderToStaticMarkup(React.createElement(
      Theme.Provider, { value: tokens(true) }, React.createElement(AudioSetupGuide, {
        guidance: saved, onDismiss() {}, onOpenSettings() {},
      }),
    )));
    assert.ok(html.includes(textHTML(translate(saved.titleKey, locale))), locale);
    assert.ok(html.includes(textHTML(translate(saved.descriptionKey, locale))), locale);
    assert.ok(html.includes(textHTML(translate('audio_guide_settings', locale))), locale);
    assert.ok(html.includes(translate('desktop_audio_guide_dismiss', locale)), locale);
    assert.doesNotMatch(html, /desktop_audio_|private-canary/);
    if (locale === 'en') assert.doesNotMatch(html, /[\p{Script=Han}]/u);
  }
});

test('localized gate explanations retain stable permission, model, and busy guidance categories', () => {
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.capabilities!.supported_operations = ['listen'];
  snapshot.permissions.microphone = 'prompt';
  snapshot.permissions.speech = 'authorized';
  snapshot.recognizerAvailable = true;
  for (const locale of DESKTOP_LOCALES) inLocale(locale, () => {
    const gate = audioFeatureGate(audioConfigurationDefaults(), snapshot, 'listen');
    assert.equal(gate.reason, translate('desktop_audio_gate_microphone', locale));
    assert.equal(gate.reasonCode, 'permission_denied');
    assert.equal(audioSetupGuidance('unavailable', gate.reasonCode)?.titleKey, 'audio_guide_permission_title');
    assert.equal(audioSetupGuidance('unavailable', 'busy')?.descriptionKey, 'desktop_audio_guide_busy_detail');
    assert.equal(audioSetupGuidance('unavailable', 'missing_model')?.titleKey, 'audio_guide_configuration_title');
    assert.equal(audioSetupGuidance('unavailable', 'unsupported')?.descriptionKey, 'desktop_audio_guide_unsupported_detail');
  });
});
