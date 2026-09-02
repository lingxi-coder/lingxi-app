import { test } from 'node:test';
import assert from 'node:assert/strict';

import { SETTINGS_NAV, searchNav } from '../src/renderer/components/settings/nav';
import { PAGE_CONTENT } from '../src/renderer/components/settings/SettingsScreen';

test('the nav declares all sixteen pages across four groups', () => {
  assert.equal(SETTINGS_NAV.length, 16);
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
  for (const id of ['account', 'appearance', 'projects', 'diagnostics', 'about', 'voice']) {
    assert.equal(
      SETTINGS_NAV.find((p) => p.id === id)?.needsEngine, false,
      `${id} lives in the client and must stay usable with no engine`,
    );
  }
});

test('provider credentials stays visible while the engine is disconnected', () => {
  assert.equal(
    SETTINGS_NAV.find((page) => page.id === 'provider-credentials')?.needsEngine,
    false,
    'the page owns engine recovery and must not be replaced by the generic engine-required placeholder',
  );
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

// Until Task 9 of the desktop-audio-capability plan, `voice` was the one
// `implemented: false` entry left in `SETTINGS_NAV` — every OTHER page had
// already been built by Tasks 15-19. `voice` needed the voice preferences/
// capability-probe/capture/synthesis modules those tasks' own predecessors
// (Tasks 4-8) built first, which did not exist yet when `nav.ts` was
// written. With `Voice.tsx` landed, there is no more honest
// `implemented: false` page left to demonstrate — see
// `settings-screen.test.mjs`'s `page-content` scenario, which now checks
// `voice` renders real content the same way it already checks the other
// fifteen pages.
test('every declared settings page is implemented', () => {
  for (const page of SETTINGS_NAV) {
    assert.equal(page.implemented, true, `${page.id}.implemented must be true`);
  }
});

// Task 20 fix round 1, Important: before this, every `implemented: true`
// page having a `PAGE_CONTENT` entry was true by coincidence — the fourteen
// ids on each side happened to line up. `PAGE_CONTENT` is a
// `Partial<Record<string, ...>>`, so a silently-dropped or typo'd key would
// type-check fine and just fall through to the `not-wired` placeholder at
// runtime. This pins the correspondence directly: every page this shell
// claims is built must actually have a component registered for it, so the
// layer-switcher-over-placeholder state this task closed off cannot come
// back through a future edit that adds a nav entry (or a `PAGE_CONTENT` key)
// without its other half.
test('every implemented nav page has a real PAGE_CONTENT entry — not by coincidence', () => {
  for (const page of SETTINGS_NAV.filter((p) => p.implemented)) {
    assert.ok(
      PAGE_CONTENT[page.id],
      `${page.id} is implemented:true but has no PAGE_CONTENT component — it would silently fall back to the "not-wired" placeholder`,
    );
  }
});
