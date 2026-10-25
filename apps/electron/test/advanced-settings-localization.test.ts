import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { CustomProviders } from '../src/renderer/components/settings/pages/CustomProviders';
import { ProviderCredentials } from '../src/renderer/components/settings/pages/ProviderCredentials';
import { ProviderEditorFields } from '../src/renderer/components/settings/pages/ProviderEditorFields';
import { ProviderProtocol } from '../src/renderer/components/settings/pages/ProviderProtocol';
import { ProviderRegion } from '../src/renderer/components/settings/pages/ProviderRegion';
import { Fusion } from '../src/renderer/components/settings/pages/Fusion';
import { Permissions } from '../src/renderer/components/settings/pages/Permissions';
import { ToolsAgent } from '../src/renderer/components/settings/pages/ToolsAgent';
import { Diagnostics } from '../src/renderer/components/settings/pages/Diagnostics';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

function renderInLocale(locale: DesktopLocale, element: React.ReactElement): string {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

test('advanced settings render all supported languages while preserving profile IDs and user descriptions', () => {
  const snapshot = { files: [], effective: {}, active: {}, provenance: {}, locked: [], layers: { user: {} }, mergedKeys: [] };
  const props: any = {
    bridge: {
      bootstrap: { settings: {}, providerCredentials: [] }, desktop: { currentModel: null, providerModelCatalog: [] },
      agentCatalog: [{ name: 'User Agent', description: 'Keep original 用户说明', tools_allowed: [] }],
    },
    snapshot, editingLayer: 'user', theme: 'dark', onTheme() {}, onNavigate() {}, onClose() {},
  };
  for (const locale of DESKTOP_LOCALES) {
    const pages: Array<[React.ReactElement, string]> = [
      [React.createElement(CustomProviders, props), 'desktop_settings_cb6216aa42'],
      [React.createElement(ProviderCredentials, { ...props, initialProviderId: 'openai-chatgpt' }), 'desktop_settings_1dcf32a354'],
      [React.createElement(ProviderEditorFields, { name: 'User-Profile-用户', onName() {}, draft: { type: 'openai', models: [{ id: 'user-model' }] }, onDraft() {}, apiKey: '', onApiKey() {} }), 'desktop_settings_d1ff8f98bc'],
      [React.createElement(ProviderProtocol, { ...props, providerId: 'openai-chatgpt' }), 'desktop_settings_e06568d15f'],
      [React.createElement(ProviderRegion, props), 'desktop_settings_d50e29c2a9'],
      [React.createElement(Fusion, props), 'desktop_settings_53bc997d63'],
      [React.createElement(Permissions, props), 'desktop_settings_240cf246ac'],
      [React.createElement(ToolsAgent, props), 'desktop_settings_d08bf2421b'],
      [React.createElement(Diagnostics, props), 'desktop_settings_4f89901316'],
    ];
    for (const [page, key] of pages) {
      const html = renderInLocale(locale, page);
      // Render the expected text through React too, so quotes/entities use the
      // same HTML escaping rules as the actual control.
      const expected = renderToStaticMarkup(React.createElement(React.Fragment, null, translate(key, locale)));
      assert.ok(html.includes(expected), `${typeof page.type === 'function' ? page.type.name : page.type}: ${key}: ${locale}`);
      assert.doesNotMatch(html, /desktop_settings_|%[1-9]\$/);
      if (page.type === ProviderEditorFields) assert.ok(html.includes('User-Profile-用户'));
      if (page.type === ToolsAgent) assert.ok(html.includes('Keep original 用户说明'));
    }
  }
});
