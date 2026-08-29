import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  computeLayerPatch,
  initialLayerText,
  layersDiverge,
  validateRawLayer,
} from '../src/renderer/components/settings/pages/RawJson';
import type { SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';

function snap(overrides: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    files: [],
    effective: {},
    active: {},
    provenance: {},
    locked: [],
    layers: {},
    mergedKeys: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// validateRawLayer — brief's Step 1 tests, verbatim (malformed / array /
// valid-object A-B pair).
// ---------------------------------------------------------------------------

test('malformed JSON is refused before it can overwrite a layer', () => {
  const error = validateRawLayer('{ "outputStyle": }');
  assert.ok(error, 'broken JSON must be refused');
  assert.match(error as string, /JSON/i);
});

test('a JSON array is refused — a settings layer must be an object', () => {
  assert.ok(validateRawLayer('[1,2,3]'));
});

test('valid object JSON passes (the A/B for the tests above)', () => {
  assert.equal(validateRawLayer('{ "outputStyle": "terse" }'), null);
});

test('a bare JSON scalar (string/number/boolean) is refused, same as an array', () => {
  assert.ok(validateRawLayer('"terse"'));
  assert.ok(validateRawLayer('42'));
  assert.ok(validateRawLayer('true'));
});

test('JSON null is refused — typeof null === "object" is the classic trap', () => {
  assert.ok(validateRawLayer('null'));
});

test('an empty object is a valid layer', () => {
  assert.equal(validateRawLayer('{}'), null);
});

// ---------------------------------------------------------------------------
// initialLayerText — reads the LAYER'S OWN map, never `effective`; a broken
// layer starts empty rather than showing a misleading "{}".
// ---------------------------------------------------------------------------

test('initialLayerText pretty-prints the editing layer\'s OWN raw map', () => {
  const snapshot = snap({
    effective: { outputStyle: 'from-effective-should-not-appear' },
    layers: {
      user: { outputStyle: 'terse' },
      project: { outputStyle: 'verbose' },
    },
  });
  const text = initialLayerText(snapshot, 'user', false);
  assert.deepEqual(JSON.parse(text), { outputStyle: 'terse' });
});

test('initialLayerText defaults an unset layer to an empty object', () => {
  assert.equal(initialLayerText(snap(), 'local', false), '{}');
});

test('initialLayerText starts a BROKEN layer empty, not "{}"', () => {
  const snapshot = snap({ layers: { user: {} } }); // build_snapshot's own fallback for a broken file
  assert.equal(initialLayerText(snapshot, 'user', true), '');
});

// ---------------------------------------------------------------------------
// computeLayerPatch — the diff that keeps an untouched `permissions` block
// from being re-asserted on every save (which would make this page unable
// to save anything else in a layer that also sets permissions).
// ---------------------------------------------------------------------------

test('computeLayerPatch is empty when nothing changed', () => {
  const layer = { outputStyle: 'terse', permissions: { allow: ['Bash'] } };
  assert.deepEqual(computeLayerPatch(layer, { ...layer }), {});
});

test('computeLayerPatch includes only an ADDED key', () => {
  const patch = computeLayerPatch({ outputStyle: 'terse' }, { outputStyle: 'terse', newKey: 1 });
  assert.deepEqual(patch, { newKey: 1 });
});

test('computeLayerPatch includes only a CHANGED key', () => {
  const patch = computeLayerPatch({ outputStyle: 'terse' }, { outputStyle: 'verbose' });
  assert.deepEqual(patch, { outputStyle: 'verbose' });
});

test('computeLayerPatch reports a REMOVED key as null', () => {
  const patch = computeLayerPatch({ outputStyle: 'terse', dropMe: true }, { outputStyle: 'terse' });
  assert.deepEqual(patch, { dropMe: null });
});

test('computeLayerPatch never re-asserts an UNCHANGED permissions block', () => {
  const permissions = { allow: ['Bash(ls:*)'], deny: [], ask: [], additionalDirectories: [] };
  const patch = computeLayerPatch(
    { permissions, outputStyle: 'terse' },
    { permissions, outputStyle: 'verbose' },
  );
  assert.deepEqual(patch, { outputStyle: 'verbose' }, 'only the actually-changed key belongs in the patch');
  assert.ok(!('permissions' in patch), 'an untouched permissions block must never be re-sent');
});

test('computeLayerPatch DOES include permissions when its value actually changed — the page must not silently drop it', () => {
  const patch = computeLayerPatch(
    { permissions: { allow: [], deny: [], ask: [], additionalDirectories: [] } },
    { permissions: { allow: ['Bash'], deny: [], ask: [], additionalDirectories: [] } },
  );
  assert.ok('permissions' in patch, 'a genuine permissions edit must still be attempted, not quietly stripped');
});

// ---------------------------------------------------------------------------
// layersDiverge — the grounded (not promise-based) refusal check.
// ---------------------------------------------------------------------------

test('layersDiverge is false for two layers with identical top-level values', () => {
  assert.equal(layersDiverge({ a: 1, b: [1, 2] }, { a: 1, b: [1, 2] }), false);
});

test('layersDiverge is true when a key\'s value differs', () => {
  assert.equal(layersDiverge({ permissions: { allow: ['Bash'] } }, { permissions: { allow: [] } }), true);
});

test('layersDiverge is true when one side has an extra key', () => {
  assert.equal(layersDiverge({ a: 1 }, { a: 1, b: 2 }), true);
});
