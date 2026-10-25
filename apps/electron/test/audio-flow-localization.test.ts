import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  DEFAULT_VOICE_FLOW_STATE, VoiceFlowController, voiceFlowDetail,
  type VoiceFlowBridge, type VoiceFlowState,
} from '../src/renderer/audio/flow/controller';
import { VoiceFlowPanel } from '../src/renderer/components/voice/VoiceFlowPanel';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, invalidateLanguagePreference, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import type { AudioOperationResultDto } from '@lingxi/bridge-client';

(globalThis as { React?: typeof React }).React = React;

async function withLocale(body: (select: (locale: DesktopLocale) => void) => void | Promise<void>): Promise<void> {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  let selected: DesktopLocale = 'en';
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? selected : null },
  });
  try { await body(locale => { selected = locale; invalidateLanguagePreference(); }); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

function escaped(text: string): string {
  return renderToStaticMarkup(React.createElement(React.Fragment, null, text));
}

test('Flow panel translates controls and saved status for all five languages without replacing raw content', async () => {
  await withLocale(select => {
    for (const locale of DESKTOP_LOCALES) {
      select(locale);
      const render = (state: VoiceFlowState) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
        React.createElement(VoiceFlowPanel, { state, onOrb() {}, onRetry() {}, onOpenSettings() {}, onClose() {} })));
      const html = render(DEFAULT_VOICE_FLOW_STATE);
      for (const key of ['voice_flow_mode_label', 'desktop_flow_idle', 'desktop_flow_orb_start', 'audio_guide_settings', 'voice_close_flow_mode_a11y']) {
        assert.ok(html.includes(escaped(translate(key, locale))), `${locale}: ${key}`);
      }
      const raw = 'User transcript: 心流模式 %1$s <untouched>';
      assert.ok(render({ phase: 'thinking', detail: raw, generation: 1 }).includes(escaped(raw)), locale);
      assert.equal(voiceFlowDetail({ ...DEFAULT_VOICE_FLOW_STATE, detail: raw }), raw, 'external adapters can replace the default detail');
      assert.doesNotMatch(html, /desktop_flow_|voice_flow_mode_label|audio_guide_settings/);
    }
  });
});

test('stored controller prompts change language while transcripts and backend errors keep their original text', async () => {
  await withLocale(async select => {
    let finish!: (result: AudioOperationResultDto) => void;
    const result = new Promise<AudioOperationResultDto>(resolve => { finish = resolve; });
    const bridge: VoiceFlowBridge = {
      sendTrackedPrompt: () => ({ token: { sessionId: 'session', clientTurnId: 'turn', purpose: 'flow' }, queued: Promise.resolve() }),
      subscribeTrackedSpeech: () => () => {},
      cancelTrackedPrompt: async () => {},
    };
    const controller = new VoiceFlowController({
      audio: { execute: async () => ({ result: await result }), finishListen: async () => {}, cancel: async () => {} },
      bridge, getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 1 }),
      timers: { setTimeout: () => 0, clearTimeout() {} }, onStateChange() {},
    });
    const started = controller.start();
    const listening = controller.getState();
    assert.equal(voiceFlowDetail(listening), translate('voice_listening', 'en'));
    select('ja');
    assert.equal(voiceFlowDetail(listening), translate('voice_listening', 'ja'));
    const transcript = '正在聆听… literal %2$s';
    finish({ type: 'transcript', text: transcript });
    await started;
    assert.equal(voiceFlowDetail(controller.getState()), transcript);
    select('ko');
    assert.equal(voiceFlowDetail(controller.getState()), transcript);
    controller.dispose();

    const message = 'Original backend failure: 原文 %1$s';
    const failed = new VoiceFlowController({
      audio: { execute: async () => ({ result: { type: 'failed', error: { kind: 'native_failure', message } } }), finishListen: async () => {}, cancel: async () => {} },
      bridge, getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 1 }),
      timers: { setTimeout: () => 0, clearTimeout() {} }, onStateChange() {},
    });
    await failed.start();
    assert.equal(voiceFlowDetail(failed.getState()), `native_failure: ${message}`);
    select('en');
    assert.equal(voiceFlowDetail(failed.getState()), `native_failure: ${message}`);
    failed.dispose();
  });
});

test('saved cancellation failures retranslate nested app messages and preserve the backend detail', async () => {
  await withLocale(async select => {
    let speech!: Parameters<VoiceFlowBridge['subscribeTrackedSpeech']>[1];
    const token = { sessionId: 'session', clientTurnId: 'turn', purpose: 'flow' as const };
    const backendError = 'Opaque backend failure %2$d <raw>';
    let operations = 0;
    const controller = new VoiceFlowController({
      audio: {
        execute: async () => ({ result: operations++ === 0
          ? { type: 'transcript', text: 'User question' }
          : { type: 'transcript', text: 'Invalid playback result' } }),
        finishListen: async () => {}, cancel: async () => {},
      },
      bridge: {
        sendTrackedPrompt: () => ({ token, queued: Promise.resolve() }),
        subscribeTrackedSpeech: (_token, listener) => { speech = listener; return () => {}; },
        cancelTrackedPrompt: async () => { throw new Error(backendError); },
      },
      getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 1 }),
      timers: { setTimeout: () => 0, clearTimeout() {} }, onStateChange() {},
    });
    await controller.start();
    speech({ type: 'message', token, text: 'A reply.', turnId: 1 });
    await new Promise(resolve => setImmediate(resolve));
    const failure = controller.getState();
    assert.equal(failure.phase, 'failed');
    for (const locale of DESKTOP_LOCALES) {
      select(locale);
      assert.equal(voiceFlowDetail(failure), translate('desktop_flow_stop_previous_with_detail', locale,
        translate('desktop_flow_playback_unconfirmed', locale), backendError));
    }
    controller.dispose();
  });
});
