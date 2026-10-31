import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { McpServers } from '../src/renderer/components/settings/pages/McpServers';
import { Skills } from '../src/renderer/components/settings/pages/Skills';
import { Plugins } from '../src/renderer/components/settings/pages/Plugins';
import { permissionModeOptions, PERMISSION_MODE_OPTIONS } from '../src/renderer/model/permissionModes';
import { chatCatalog } from '../src/renderer/i18n/chatCatalog';
import { extensionCatalog } from '../src/renderer/i18n/extensionCatalog';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

function renderInLocale(locale: DesktopLocale, element: React.ReactElement): string {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

test('extension management screens render the selected language and preserve catalog content', () => {
  const props: any = {
    bridge: {
      bootstrap: { settings: {} },
      skillsEvent: { skills: [{ name: 'User skill name', source_dir: '/tmp/user/.lingxi/skills/my-skill' }] },
      pluginCatalogEvent: { catalog_json: JSON.stringify({ installed: [{ id: 'user-plugin', name: 'User plugin name' }], revisions: {} }) },
    },
    snapshot: null, editingLayer: 'user', theme: 'dark', onTheme() {}, onNavigate() {}, onClose() {},
  };
  for (const locale of DESKTOP_LOCALES) {
    const mcp = renderInLocale(locale, React.createElement(McpServers, props));
    assert.ok(mcp.includes(translate('desktop_extension_mcp_description', locale)), locale);
    assert.ok(mcp.includes(renderToStaticMarkup(React.createElement(React.Fragment, null, translate('mcp_scope_vs_layers_note', locale)))), locale);
    const skills = renderInLocale(locale, React.createElement(Skills, props));
    assert.ok(skills.includes(translate('desktop_extension_skill_description', locale)), locale);
    assert.ok(skills.includes('User skill name'));
    const plugins = renderInLocale(locale, React.createElement(Plugins, props));
    assert.ok(plugins.includes(translate('desktop_extension_plugin_description', locale)), locale);
    // Installed plugins are on the personal tab; an explicitly selected
    // plugin still renders its catalog name in its details.
    const pluginDetails = renderInLocale(locale, React.createElement(Plugins, { ...props, initialPluginId: 'user-plugin' }));
    assert.ok(pluginDetails.includes('User plugin name'));
    for (const html of [mcp, skills, plugins]) assert.doesNotMatch(html, /desktop_extension_|settings_title_plugins/);
  }
});

test('permission modes update translated display text without changing command IDs or safety flags', () => {
  for (const locale of DESKTOP_LOCALES) {
    const modes = permissionModeOptions(locale);
    assert.deepEqual(modes.map(({ id, icon, danger }) => ({ id, icon, danger })), PERMISSION_MODE_OPTIONS.map(({ id, icon, danger }) => ({ id, icon, danger })));
    assert.equal(modes[0]!.label, translate('desktop_chat_permission_ask', locale));
    assert.equal(modes.at(-1)!.danger, true);
    assert.doesNotMatch(modes.map(mode => `${mode.label} ${mode.shortLabel} ${mode.description}`).join(' '), /desktop_chat_/);
  }
  assert.notEqual(permissionModeOptions('ja')[0]!.label, permissionModeOptions('en')[0]!.label);
});

test('chat and extension dictionaries provide complete translations with matching format arguments', () => {
  for (const [key, values] of Object.entries({ ...chatCatalog, ...extensionCatalog })) {
    const placeholders = (value: string) => [...value.matchAll(/%\d+\$[sd]/g)].map(match => match[0]).sort();
    for (const locale of DESKTOP_LOCALES) {
      assert.ok(values[locale]?.trim(), `${key}: ${locale}`);
      assert.deepEqual(placeholders(values[locale]!), placeholders(values.en!), `${key}: ${locale}`);
    }
  }
});
