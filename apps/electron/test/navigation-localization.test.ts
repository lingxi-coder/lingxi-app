import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { registerHooks } from 'node:module';
import { MentionMenu } from '../src/renderer/components/MentionMenu';
import { FileMentionPreview } from '../src/renderer/components/FileMentionPreview';
import { navigationCatalog } from '../src/renderer/i18n/navigationCatalog';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const cssHook = registerHooks({ load(url, context, nextLoad) {
  return url.endsWith('.css') ? { format: 'module', source: 'export {};', shortCircuit: true } : nextLoad(url, context);
} });
const { SideChatPanel } = await import('../src/renderer/components/SideChatPanel');
const { DateTimePicker } = await import('../src/renderer/components/ui/DateTimePicker');
cssHook.deregister();
const noop = () => {};
const escaped = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));
function renderInLocale(locale: DesktopLocale, element: React.ReactElement): string {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null } });
  try { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

test('side chat and reference surfaces translate controls without changing user content', () => {
  const side: any = { id: 'side-1', draft: 'User draft', images: [], permissions: [], questions: [],
    desktop: { models: [], currentModel: 'user-model', conversationControls: undefined }, conversation: { items: [], running: false }, closing: false };
  for (const locale of DESKTOP_LOCALES) {
    const sideHtml = renderInLocale(locale, React.createElement(SideChatPanel, { controller: { open: true, selected: side, sides: [side] } as any }));
    assert.ok(sideHtml.includes(escaped(translate('desktop_navigation_side_number', locale, 1))), locale);
    assert.ok(sideHtml.includes(escaped(translate('desktop_navigation_parent_context', locale))), locale);
    assert.ok(sideHtml.includes('User draft'));
    const mentions = renderInLocale(locale, React.createElement(MentionMenu, {
      entries: [
        { id: 'files', name: 'Files and folders', description: 'Search this workspace', group: 'Add', icon: 'folder', action: 'files' },
        { id: 'user-plugin', name: 'User plugin name', description: 'User plugin description', group: 'Plugins', icon: 'plug' },
      ], selectedIndex: 0, query: '', filesOnly: false, searchInput: React.createRef<HTMLInputElement>(), status: 'ready', truncated: false,
      onQuery: noop, onSelectIndex: noop, onChoose: noop, onKeyDown: noop, onClose: noop, onBack: noop,
    }));
    assert.ok(mentions.includes(escaped(translate('desktop_navigation_search_workspace', locale))), locale);
    assert.ok(mentions.includes('User plugin name'));
    assert.ok(mentions.includes('User plugin description'));
    const preview = renderInLocale(locale, React.createElement(FileMentionPreview, { path: '/tmp/User file.md', bridge: {} as any, onClose: noop }));
    assert.ok(preview.includes(escaped(translate('desktop_navigation_loading_preview', locale))), locale);
    assert.ok(preview.includes('/tmp/User file.md'));
    for (const html of [sideHtml, mentions, preview]) assert.doesNotMatch(html, /desktop_navigation_|%\d+\$[sd]/);
  }
});

test('date picker uses the interface language for dates and navigation controls', () => {
  for (const locale of DESKTOP_LOCALES) {
    const html = renderInLocale(locale, React.createElement(DateTimePicker, { mode: 'datetime-local', value: '2026-01-02T14:30', label: 'User date label', onChange: noop }));
    const date = new Date(2026, 0, 2, 12).toLocaleDateString(locale, { month: 'short', day: 'numeric', year: 'numeric' });
    assert.ok(html.includes(escaped(date)), locale);
    assert.ok(html.includes(escaped(translate('desktop_navigation_previous_month', locale))), locale);
    assert.ok(html.includes('User date label'));
    assert.ok(html.includes('14:30'));
    assert.doesNotMatch(html, /desktop_navigation_/);
  }
});

test('navigation copy includes every language and preserves numbered parameters', () => {
  for (const [key, values] of Object.entries(navigationCatalog)) {
    const parameters = (value: string) => [...value.matchAll(/%\d+\$[sd]/g)].map(match => match[0]).sort();
    for (const locale of DESKTOP_LOCALES) {
      assert.ok(values[locale]?.trim(), `${key}: ${locale}`);
      assert.deepEqual(parameters(values[locale]!), parameters(values.en!), `${key}: ${locale}`);
    }
  }
});
