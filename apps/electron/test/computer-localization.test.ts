import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ComputerAccessRequestDto } from '@lingxi/bridge-client';
import { ComputerAccessPrompt } from '../src/renderer/components/ComputerAccessPrompt';
import { ComputerUse } from '../src/renderer/components/settings/pages/ComputerUse';
import { GoogleChromeSettings } from '../src/renderer/components/settings/pages/ComputerUseBrowser';
import { computerCatalog } from '../src/renderer/i18n/computerCatalog';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const escaped = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));

function renderInLocale(locale: DesktopLocale, element: React.ReactElement): string {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try {
    return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
  } finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

const request: ComputerAccessRequestDto = {
  request_id: 1, reason: 'Keep this user-provided reason intact', apps: [{ label: 'Preview' }],
  tier: 'read', clipboard_read: true, clipboard_write: true, system_key_combos: true,
};

test('read-only computer settings render localized copy and preserve unavailable integration controls', () => {
  for (const locale of DESKTOP_LOCALES) {
    const settings = renderInLocale(locale, React.createElement(ComputerUse));
    assert.ok(settings.includes(`<h1>${translate('desktop_computer_title', locale)}</h1>`), locale);
    assert.ok(settings.includes(escaped(translate('desktop_computer_readonly_any_app_description', locale))), locale);
    assert.ok(settings.includes(escaped(translate('desktop_computer_readonly_permanent_access', locale))), locale);
    const anyAppLabel = translate('desktop_computer_readonly_any_app_label', locale);
    const anyAppToggle = (settings.match(/<button\b[^>]*>/g) ?? []).find(tag => tag.includes(`aria-label="${anyAppLabel}"`));
    assert.ok(anyAppToggle, locale);
    assert.match(anyAppToggle, /disabled/, locale);
    const browser = renderInLocale(locale, React.createElement(GoogleChromeSettings, { onBack: () => {} }));
    assert.ok(browser.includes(translate('desktop_computer_website_permissions', locale)), locale);
    assert.ok(browser.includes(escaped(translate('desktop_computer_readonly_full_cdp_description', locale))), locale);
    assert.ok(browser.includes(escaped(translate('desktop_computer_readonly_browser_settings', locale))), locale);
    assert.match(browser, /<select[^>]*disabled/);
    const prompt = renderInLocale(locale, React.createElement(ComputerAccessPrompt, {
      request, onSubmit: () => {}, onDeny: () => {}, onOpenSystemSettings: () => {},
    }));
    assert.ok(prompt.includes(translate('desktop_computer_allow_prompt', locale, 'Preview')), locale);
    assert.ok(prompt.includes(request.reason), locale);
    assert.ok(prompt.includes(translate('desktop_computer_clipboard_read', locale)), locale);
    assert.doesNotMatch(prompt, /desktop_computer_|%1\$s/);
    if (locale !== 'en') {
      assert.doesNotMatch(settings, /Manage how LingXi uses other applications/);
      assert.doesNotMatch(browser, /Choose default permissions and add exceptions/);
      assert.doesNotMatch(prompt, /Allowing LingXi to use apps/);
    }
  }
});

test('computer approval preserves the HEAD session tier and Apps tier heading', () => {
  for (const locale of DESKTOP_LOCALES) {
    for (const tier of ['read', 'click', 'full'] as const) {
      const prompt = renderInLocale(locale, React.createElement(ComputerAccessPrompt, {
        request: { ...request, tier }, onSubmit: () => {}, onDeny: () => {}, onOpenSystemSettings: () => {},
      }));
      const access = translate(`desktop_computer_${tier}_access`, locale);
      assert.ok(prompt.includes(escaped(translate('desktop_computer_session_tier_scope', locale, access))), `${locale}: ${tier}`);
      const tierLabel = translate(`desktop_computer_tier_${tier}`, locale);
      assert.ok(prompt.includes(escaped(translate('desktop_computer_apps_tier', locale, tierLabel))), `${locale}: ${tier}`);
      assert.doesNotMatch(prompt, /desktop_computer_|%\d+\$[sd]/);
    }
  }
});

test('computer copy supplies all five languages and keeps format arguments consistent', () => {
  for (const [key, values] of Object.entries(computerCatalog)) {
    const placeholders = (value: string) => [...value.matchAll(/%\d+\$[sd]/g)].map(match => match[0]).sort();
    for (const locale of DESKTOP_LOCALES) {
      assert.ok(values[locale]?.trim(), `${key}: ${locale}`);
      assert.deepEqual(placeholders(values[locale]!), placeholders(values.en!), `${key}: ${locale}`);
    }
  }
});
