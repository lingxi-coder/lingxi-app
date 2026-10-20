import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Plugins } from '../src/renderer/components/settings/pages/Plugins';
import type { PageContentProps } from '../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

function renderDetails(configSchemaJson: string) {
  const id = 'lingxi-token-weather@lingxi-desktop-mods';
  const bridge = {
    pluginAdmin: async () => undefined,
    pluginCatalogEvent: {
      catalog_json: JSON.stringify({
        installed: [{
          id, name: 'lingxi-token-weather', display_name: 'Token Weather (LingXi)',
          version: '0.1.1', config_schema_json: configSchemaJson,
        }],
        revisions: {},
      }),
    },
  } as unknown as PageContentProps['bridge'];
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
    React.createElement(Plugins, {
      bridge, snapshot: null, editingLayer: 'user', initialPluginId: id,
      theme: 'light', onTheme() {}, onNavigate() {}, onClose() {}, onJumpToLayer() {},
    })));
}

test('installed plugin details render when the current protocol has no user_config schema', () => {
  const html = renderDetails('null');
  assert.match(html, /Token Weather \(LingXi\)/);
  assert.match(html, /0\.1\.1/);
  assert.match(html, /Enabled/);
  assert.match(html, /Configuration/);
});

test('installed plugin details still render declared configuration fields', () => {
  const html = renderDetails(JSON.stringify({ fields: {
    units: { type: 'string', title: 'Display units', description: 'Units used by this Mod' },
  } }));
  assert.match(html, /Display units/);
  assert.match(html, /Units used by this Mod/);
  assert.match(html, /units option/);
});
