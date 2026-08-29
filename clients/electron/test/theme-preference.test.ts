import { test } from 'node:test';
import assert from 'node:assert/strict';

import { resolveThemeMode, watchThemePreference, type SystemColorSchemeQuery } from '../src/renderer/theme/tokens';

test('resolveThemeMode passes dark/light through and resolves system from the OS query', () => {
  assert.equal(resolveThemeMode('dark', false), 'dark');
  assert.equal(resolveThemeMode('light', true), 'light');
  assert.equal(resolveThemeMode('system', true), 'dark');
  assert.equal(resolveThemeMode('system', false), 'light');
  assert.equal(resolveThemeMode(undefined, true), 'dark');
  assert.equal(resolveThemeMode(undefined, false), 'light');
});

function fakeMediaQuery(initialMatches: boolean): SystemColorSchemeQuery & {
  fireChange(matches: boolean): void;
  listenerCount(): number;
} {
  let matches = initialMatches;
  const listeners = new Set<() => void>();
  return {
    get matches() { return matches; },
    addEventListener(_type, listener) { listeners.add(listener); },
    removeEventListener(_type, listener) { listeners.delete(listener); },
    fireChange(next) { matches = next; for (const listener of listeners) listener(); },
    listenerCount() { return listeners.size; },
  };
}

test("watchThemePreference resolves once for 'dark'/'light' and never attaches a listener", () => {
  const query = fakeMediaQuery(true);
  const seen: string[] = [];
  const cleanup = watchThemePreference('light', query, (mode) => seen.push(mode));
  assert.deepEqual(seen, ['light']);
  assert.equal(query.listenerCount(), 0);
  cleanup();
});

// Final review, Minor (user-visible): this test used to PIN the no-op as
// intended. It is not: `Appearance.tsx` renders `settings.theme ?? 'system'`
// as the selected pill and `resolveThemeMode` already reads `undefined` as
// "ask the OS", so a fresh install with no persisted preference showed
// 跟随系统 selected while the app stayed on `App.tsx`'s hardcoded `'dark'`
// seed and ignored the OS entirely.
test("watchThemePreference treats an absent preference as 'system' — resolving AND following the OS", () => {
  const query = fakeMediaQuery(false);
  const seen: string[] = [];
  const cleanup = watchThemePreference(undefined, query, (mode) => seen.push(mode));
  assert.deepEqual(
    seen, ['light'],
    'no stored preference means "follow the OS", so it must resolve from the OS immediately',
  );
  assert.equal(
    query.listenerCount(), 1,
    'and keep following it — resolving once at mount would stop tracking the OS',
  );

  query.fireChange(true);
  assert.deepEqual(seen, ['light', 'dark'], 'flipping the OS theme must reach the app');

  cleanup();
  assert.equal(query.listenerCount(), 0, 'cleanup must remove the change listener');
});

test("watchThemePreference follows the OS when the preference is 'system', and stops after cleanup", () => {
  const query = fakeMediaQuery(false);
  const seen: string[] = [];
  const cleanup = watchThemePreference('system', query, (mode) => seen.push(mode));
  assert.deepEqual(seen, ['light'], 'resolves immediately from the current OS setting');
  assert.equal(query.listenerCount(), 1, 'must attach a change listener for system preference');

  query.fireChange(true);
  assert.deepEqual(seen, ['light', 'dark'], 'must follow the OS when it changes while mounted');

  query.fireChange(false);
  assert.deepEqual(seen, ['light', 'dark', 'light']);

  cleanup();
  assert.equal(query.listenerCount(), 0, 'cleanup must remove the change listener');
  query.fireChange(true);
  assert.deepEqual(seen, ['light', 'dark', 'light'], 'no further updates after cleanup');
});
