import { test } from 'node:test';
import assert from 'node:assert/strict';

import { SETTINGS_NAV, searchNav } from '../src/renderer/components/settings/nav';

test('the nav declares all fifteen pages across four groups', () => {
  assert.equal(SETTINGS_NAV.length, 15);
  assert.deepEqual(
    [...new Set(SETTINGS_NAV.map((p) => p.group))],
    ['个人', '模型与服务', '编码', '高级'],
  );
});

test('layered is a data property, not a group property — three pages break the correspondence', () => {
  // layered means "this page's values live in the four settings layers and need
  // the layer switcher to pick a target". Group is a navigation concept and
  // does not determine this. Named here explicitly so a future edit that
  // "fixes" one of these back to match its group is caught, not shipped:
  //   - mcp: inside 编码, but NOT layered (its own three-scope storage, no layers).
  //   - custom-providers: outside 编码, but layered (writes settings.providers/routing).
  //   - raw-json: outside 编码, but layered (its whole job is editing the current layer's file).
  const layeredIds = new Set([
    'permissions', 'tools-agent', 'skills', 'hooks', 'plugins',
    'custom-providers', 'raw-json',
  ]);
  for (const page of SETTINGS_NAV) {
    assert.equal(
      page.layered, layeredIds.has(page.id),
      `${page.id}.layered must be ${layeredIds.has(page.id)}`,
    );
  }
});

test('client-owned pages do not need the engine', () => {
  for (const id of ['appearance', 'projects', 'diagnostics', 'about', 'voice']) {
    assert.equal(
      SETTINGS_NAV.find((p) => p.id === id)?.needsEngine, false,
      `${id} lives in the client and must stay usable with no engine`,
    );
  }
});

test('search finds a page by a settings key it owns', () => {
  const hits = searchNav('outputStyle');
  assert.ok(
    hits.some((p) => p.id === 'tools-agent'),
    'searching a settings key must reach the page that owns it',
  );
});

test('search finds nothing for a key no page declares', () => {
  assert.deepEqual(
    searchNav('zzzzz-not-a-setting'), [],
    'if this returned hits, the search test above would prove nothing',
  );
});

test('voice is declared but not yet built; every other page is', () => {
  for (const page of SETTINGS_NAV) {
    const expected = page.id !== 'voice';
    assert.equal(
      page.implemented, expected,
      `${page.id}.implemented must be ${expected}`,
    );
  }
});
