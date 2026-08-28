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

test('only the coding group is layered, and MCP opts out', () => {
  for (const page of SETTINGS_NAV) {
    if (page.group !== '编码') {
      assert.equal(page.layered, false, `${page.id} is outside 编码 and must not be layered`);
    }
  }
  assert.equal(
    SETTINGS_NAV.find((p) => p.id === 'mcp')?.layered, false,
    'MCP has its own three-scope storage and must not reuse the settings layer switcher',
  );
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
