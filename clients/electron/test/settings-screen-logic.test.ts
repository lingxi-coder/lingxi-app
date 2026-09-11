import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  groupedNav,
  layerDisabled,
  parseSettingsSnapshot,
  resolveInitialPage,
  SETTINGS_SIDEBAR_TOP_INSET,
} from '../src/renderer/components/settings/SettingsScreen';
import type { SettingsSnapshotEvent } from '../src/renderer/bridge/useBridge';

test('resolveInitialPage opens the provider-credentials deep link when a provider id is given', () => {
  assert.equal(resolveInitialPage(undefined, 'anthropic'), 'provider-credentials');
});

test('resolveInitialPage falls back to the first nav page with no deep link', () => {
  assert.equal(resolveInitialPage(undefined), 'general');
});

test('layerDisabled: user is always editable, project and local need an open project', () => {
  assert.equal(layerDisabled('user', false), false);
  assert.equal(layerDisabled('user', true), false);
  assert.equal(layerDisabled('project', false), true);
  assert.equal(layerDisabled('project', true), false);
  assert.equal(layerDisabled('local', false), true);
  assert.equal(layerDisabled('local', true), false);
});

test('groupedNav lists every group in a fixed order with no query', () => {
  const sections = groupedNav('');
  assert.deepEqual(sections.map((s) => s.group), ['个人', '模型与服务', '编码', '高级']);
  const allIds = sections.flatMap((s) => s.pages.map((p) => p.id));
  assert.equal(allIds.length, 17);
});

test('groupedNav filters to matching pages and drops empty groups', () => {
  const sections = groupedNav('mcp');
  const allIds = sections.flatMap((s) => s.pages.map((p) => p.id));
  assert.deepEqual(allIds, ['mcp']);
});

test('groupedNav with a query matching nothing returns no sections', () => {
  assert.deepEqual(groupedNav('this matches absolutely nothing'), []);
});

test('settings search clears the macOS hidden-inset titlebar controls', () => {
  assert.ok(SETTINGS_SIDEBAR_TOP_INSET >= 40);
});

function rawSnapshot(overrides: Partial<SettingsSnapshotEvent> = {}): SettingsSnapshotEvent {
  return {
    type: 'settings_snapshot',
    effective_json: JSON.stringify({ model: 'opus' }),
    provenance_json: JSON.stringify({ model: 'user' }),
    ...overrides,
  } as SettingsSnapshotEvent;
}

test('parseSettingsSnapshot is null-safe: no event means no snapshot and no error', () => {
  assert.deepEqual(parseSettingsSnapshot(null), { snapshot: null, error: null });
  assert.deepEqual(parseSettingsSnapshot(undefined), { snapshot: null, error: null });
});

test('parseSettingsSnapshot contains settings data without runtime activation state', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot());
  assert.equal(error, null);
  assert.deepEqual(snapshot, {
    effective: { model: 'opus' },
    provenance: { model: 'user' },
    files: [],
    locked: [],
    layers: {},
    mergedKeys: [],
  });
});

// Fix round 1: `layers_json` is additive (§0.10), same as the other three —
// a producer that predates it must decode to an empty `layers` map, not
// `undefined` (which would make every `providersFromLayer`/`routingFromLayer`
// call in `CustomProviders` throw instead of honestly reporting "this layer
// has nothing").
test('parseSettingsSnapshot defaults a missing layers_json to an empty map, not undefined', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot());
  assert.equal(error, null);
  assert.deepEqual(snapshot?.layers, {});
});

test('parseSettingsSnapshot decodes layers_json into a per-layer map when present', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot({
    layers_json: JSON.stringify({
      user: { model: 'opus' },
      local: { providers: { mine: { type: 'openai', models: [{ id: 'm' }] } } },
    }),
  }));
  assert.equal(error, null);
  assert.deepEqual(snapshot?.layers, {
    user: { model: 'opus' },
    local: { providers: { mine: { type: 'openai', models: [{ id: 'm' }] } } },
  });
});

test('parseSettingsSnapshot ignores engine active_json state', () => {
  const { snapshot } = parseSettingsSnapshot(rawSnapshot({
    effective_json: JSON.stringify({ model: 'opus', theme: 'dark' }),
    active_json: JSON.stringify({ model: 'sonnet', theme: 'light' }),
  }));
  assert.equal('active' in snapshot!, false);
});

test('parseSettingsSnapshot decodes the optional fields when present', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot({
    files_json: JSON.stringify([{ layer: 'user', path: '/x', exists: true, parsed: true }]),
    active_json: JSON.stringify({ model: 'sonnet' }),
    locked: ['model'],
    layers_json: JSON.stringify({ user: { model: 'opus' } }),
    merged_keys: ['hooks'],
  }));
  assert.equal(error, null);
  assert.deepEqual(snapshot, {
    effective: { model: 'opus' },
    provenance: { model: 'user' },
    files: [{ layer: 'user', path: '/x', exists: true, parsed: true }],
    locked: ['model'],
    layers: { user: { model: 'opus' } },
    mergedKeys: ['hooks'],
  });
});

// Task 17b: `merged_keys` is additive (§0.10) like the other optional
// fields. An absent one must decode to `[]` — "we don't know of any merged
// key" — and NOT to undefined, which would make `rowState`'s
// `mergedKeys.includes` throw on every row a producer that predates the
// field feeds it.
test('parseSettingsSnapshot defaults a missing merged_keys to an empty list, not undefined', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot());
  assert.equal(error, null);
  assert.deepEqual(snapshot?.mergedKeys, []);
});

test('parseSettingsSnapshot surfaces malformed JSON as an error, not a throw', () => {
  assert.doesNotThrow(() => parseSettingsSnapshot(rawSnapshot({ effective_json: '{not json' })));
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot({ effective_json: '{not json' }));
  assert.equal(snapshot, null);
  assert.ok(error && error.length > 0, 'a malformed payload must produce a non-empty error message');
});

test('parseSettingsSnapshot surfaces a malformed optional field as an error too', () => {
  const { snapshot, error } = parseSettingsSnapshot(rawSnapshot({ files_json: '[not json' }));
  assert.equal(snapshot, null);
  assert.ok(error);
});
