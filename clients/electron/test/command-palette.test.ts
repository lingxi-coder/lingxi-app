import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  commandPaletteEntries,
  filterCommandPaletteEntries,
} from '../src/renderer/components/BetaDesktop';
import { isCommandPaletteShortcut } from '../src/renderer/App';

test('command palette merges engine and desktop commands under one searchable list', () => {
  const entries = commandPaletteEntries([
    { name: 'model', description: 'Switch models', source: 'builtin' },
    { name: 'status', description: 'Show current status', source: 'builtin' },
  ]);

  assert.ok(entries.some((entry) => entry.name === 'model' && entry.source === 'desktop'));
  assert.ok(entries.some((entry) => entry.name === 'status' && entry.source === 'engine'));
  assert.equal(entries.filter((entry) => entry.name === 'model').length, 1);
});

test('command palette filtering matches name and description case-insensitively', () => {
  const entries = [
    { name: 'model', source: 'desktop', description: 'Switch the active model' },
    { name: 'status', source: 'engine', description: 'Show current session status' },
  ] as const;

  assert.deepEqual(filterCommandPaletteEntries(entries, 'MODEL').map((entry) => entry.name), ['model']);
  assert.deepEqual(filterCommandPaletteEntries(entries, 'session').map((entry) => entry.name), ['status']);
  assert.equal(filterCommandPaletteEntries(entries, '').length, 2);
});

test('command palette opens from Command-K and Control-K only', () => {
  const event = (overrides: Partial<KeyboardEvent>) => ({
    altKey: false,
    ctrlKey: false,
    key: 'k',
    metaKey: false,
    shiftKey: false,
    ...overrides,
  }) as KeyboardEvent;
  assert.equal(isCommandPaletteShortcut(event({ metaKey: true })), true);
  assert.equal(isCommandPaletteShortcut(event({ ctrlKey: true })), true);
  assert.equal(isCommandPaletteShortcut(event({ ctrlKey: true, key: 'K' })), true);
  assert.equal(isCommandPaletteShortcut(event({ ctrlKey: true, shiftKey: true })), false);
  assert.equal(isCommandPaletteShortcut(event({ altKey: true, metaKey: true })), false);
  assert.equal(isCommandPaletteShortcut(event({ key: 'p', metaKey: true })), false);
});
