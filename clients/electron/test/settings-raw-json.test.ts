import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  brokenLayerGuidance,
  computeLayerPatch,
  layersDiverge,
  patchTouchesReservedKey,
  rawJsonView,
  saveRefusalMessage,
  saveRefused,
  validateRawLayer,
} from '../src/renderer/components/settings/pages/RawJson';
import {
  ATTEMPT_NOT_DISPATCHED,
  snapshotLandedAfter,
} from '../src/renderer/components/settings/useEngineSettings';
import type { SettingsFile, SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';

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

function file(overrides: Partial<SettingsFile> = {}): SettingsFile {
  return { layer: 'user', path: '/home/x/.lingxi/settings.json', exists: true, parsed: true, ...overrides };
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
// rawJsonView — a BROKEN layer gets a view with NO editable text/save
// affordance at all (the type itself makes "editor that implies saving can
// fix this" unrepresentable); an OK layer gets its OWN raw map, never
// `effective`.
// ---------------------------------------------------------------------------

test('rawJsonView is editable and pretty-prints the layer\'s OWN raw map for a clean file', () => {
  const snapshot = snap({
    effective: { outputStyle: 'from-effective-should-not-appear' },
    layers: {
      user: { outputStyle: 'terse' },
      project: { outputStyle: 'verbose' },
    },
    files: [file({ layer: 'user' })],
  });
  const view = rawJsonView(snapshot, 'user');
  assert.equal(view.mode, 'editable');
  assert.deepEqual(JSON.parse((view as { mode: 'editable'; initialText: string }).initialText), { outputStyle: 'terse' });
});

test('rawJsonView defaults an unset (but not broken) layer to an editable, empty object', () => {
  const view = rawJsonView(snap({ files: [file({ layer: 'local', exists: false })] }), 'local');
  assert.equal(view.mode, 'editable');
  assert.equal((view as { mode: 'editable'; initialText: string }).initialText, '{}');
});

test('rawJsonView with no snapshot at all is still editable, not broken', () => {
  assert.equal(rawJsonView(null, 'user').mode, 'editable');
});

test('rawJsonView is BROKEN for a layer with a parse_error, and carries no initialText field', () => {
  const brokenFile = file({ parsed: false, parse_error: 'trailing comma at line 3' });
  const snapshot = snap({ layers: { user: {} }, files: [brokenFile] }); // build_snapshot's own fallback for a broken file
  const view = rawJsonView(snapshot, 'user');
  assert.equal(view.mode, 'broken');
  assert.deepEqual((view as { mode: 'broken'; file: SettingsFile }).file, brokenFile);
  assert.ok(!('initialText' in view), 'a broken view must not carry any text to edit');
});

// ---------------------------------------------------------------------------
// brokenLayerGuidance — fix round 1, Critical: the page must never claim
// saving repairs a broken file. `apply_patch` (`settings_bridge.rs`) calls
// `read_settings_map` first, which returns `Err` for ANY non-empty content
// that fails to parse — the exact condition a `parse_error` reports — so
// EVERY save on a broken layer is refused before the new content is even
// considered. The guidance must say the refusal is deliberate and name the
// path so the user can fix it in a text editor instead.
// ---------------------------------------------------------------------------

test('brokenLayerGuidance never claims saving will repair or replace the file', () => {
  const notice = brokenLayerGuidance(file({ parsed: false, parse_error: 'trailing comma' }));
  assert.doesNotMatch(notice, /会(用新内容)?(整体)?替换/, 'must not claim a save replaces the broken file');
  assert.doesNotMatch(notice, /保存(合法的)?\s*JSON/, 'must not frame saving as the fix');
});

test('brokenLayerGuidance states the refusal is deliberate and names the file path', () => {
  const notice = brokenLayerGuidance(file({ path: '/home/x/.lingxi/settings.json', parsed: false, parse_error: 'boom' }));
  assert.match(notice, /拒绝/, 'must say the engine refuses the write');
  assert.match(notice, /有意的安全设计|安全设计/, 'must say the refusal is deliberate, not a bug');
  assert.match(notice, /\/home\/x\/\.lingxi\/settings\.json/, 'must name the actual file path');
  assert.match(notice, /文本编辑器/, 'must point the user at fixing it outside this page');
  assert.match(notice, /boom/, 'must surface the actual parse error');
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
// patchTouchesReservedKey / saveRefusalMessage — fix round 1, Important: a
// refusal fallback must not blame `permissions` when the patch never
// touched it.
// ---------------------------------------------------------------------------

test('patchTouchesReservedKey is true only when the patch names a reserved key', () => {
  assert.equal(patchTouchesReservedKey({ outputStyle: 'terse' }), false);
  assert.equal(patchTouchesReservedKey({ permissions: { allow: [] } }), true);
  assert.equal(patchTouchesReservedKey({ permissions: null }), true, 'deleting a reserved key still touches it');
});

test('saveRefusalMessage prefers the real engine error when present, regardless of what was attempted', () => {
  assert.equal(saveRefusalMessage('the actual engine message', true), 'the actual engine message');
  assert.equal(saveRefusalMessage('the actual engine message', false), 'the actual engine message');
});

test('saveRefusalMessage names permissions only when the attempted patch actually touched it', () => {
  assert.match(saveRefusalMessage(null, true), /permissions/);
});

test('saveRefusalMessage does NOT blame permissions when the patch never touched it', () => {
  assert.doesNotMatch(saveRefusalMessage(null, false), /permissions/);
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

// ---------------------------------------------------------------------------
// snapshotLandedAfter / saveRefused — final review, Important + Minor: the
// refusal banner was derived from a promise that resolves BEFORE the
// authoritative state arrives.
//
// `bridge.updateEngineSettings` resolves once `update_settings` and
// `refresh_listings` have been SENT; the `SettingsSnapshot` event lands
// later. So at the instant `saving` flipped false, the layer map on screen
// was still the PRE-save one, and every successful save rendered
// `保存未生效…` for a frame — self-clearing to the eye, announced by a screen
// reader every time.
// ---------------------------------------------------------------------------

test('snapshotLandedAfter is false with no attempt at all', () => {
  assert.equal(snapshotLandedAfter(null, { id: 1 }), false);
});

test('snapshotLandedAfter is false while the attempt is still being dispatched', () => {
  assert.equal(
    snapshotLandedAfter({ snapshotAtDispatch: ATTEMPT_NOT_DISPATCHED }, { id: 1 }),
    false,
    'the commands are not even on the wire yet — nothing can have answered them',
  );
});

test('snapshotLandedAfter is false while the interface still holds the SAME snapshot it dispatched against', () => {
  const beforeSave = { id: 1 };
  assert.equal(snapshotLandedAfter({ snapshotAtDispatch: beforeSave }, beforeSave), false);
});

test('snapshotLandedAfter becomes true only once a DIFFERENT snapshot event has landed', () => {
  const beforeSave = { id: 1 };
  const afterSave = { id: 2 };
  assert.equal(snapshotLandedAfter({ snapshotAtDispatch: beforeSave }, afterSave), true);
});

test('snapshotLandedAfter treats a first-ever snapshot (dispatched against null) as newer', () => {
  assert.equal(snapshotLandedAfter({ snapshotAtDispatch: null }, { id: 1 }), true);
});

test('a successful save is NOT reported as refused before its snapshot arrives', () => {
  const beforeSave = { id: 1 };
  const attempt = { value: { outputStyle: 'verbose' }, snapshotAtDispatch: beforeSave };
  // The pre-save layer map — exactly what the page still holds at the moment
  // `updateEngineSettings` resolves. The value DIVERGES from what was asked
  // for, which is precisely why the old promise-timed check fired here.
  assert.equal(
    saveRefused(attempt, beforeSave, { outputStyle: 'terse' }),
    false,
    'no snapshot newer than the attempt has landed, so this divergence proves nothing yet',
  );
});

test('a successful save is not refused once its snapshot lands', () => {
  const attempt = { value: { outputStyle: 'verbose' }, snapshotAtDispatch: { id: 1 } };
  assert.equal(saveRefused(attempt, { id: 2 }, { outputStyle: 'verbose' }), false);
});

test('a genuinely refused save IS reported, once its snapshot lands', () => {
  const attempt = { value: { permissions: { allow: ['Bash'] } }, snapshotAtDispatch: { id: 1 } };
  assert.equal(
    saveRefused(attempt, { id: 2 }, {}),
    true,
    'the engine answered and the layer still does not hold what was asked for',
  );
});

test('saveRefused is false before any save attempt', () => {
  assert.equal(saveRefused(null, { id: 1 }, { outputStyle: 'terse' }), false);
});

// `computeLayerPatch` turns an explicit `{"model": null}` into
// `patch.model = null`, which `apply_patch` reads as DELETE — so the refreshed
// layer has NO `model` while the attempt still says `model: null`. Compared by
// raw JSON.stringify (`undefined` vs `'null'`) those disagree forever, and the
// refusal banner never cleared on a save that did exactly what was asked.

test('an explicit null and an absent key are the same state, not a permanent refusal', () => {
  assert.equal(
    layersDiverge({ model: null }, {}),
    false,
    'apply_patch DELETES a null-valued key, so the attempt and the refreshed layer agree',
  );
  assert.equal(layersDiverge({}, { model: null }), false, 'and symmetrically');
});

test('saving an explicit null is not reported as refused after the delete lands', () => {
  const attempt = { value: { outputStyle: 'terse', model: null }, snapshotAtDispatch: { id: 1 } };
  assert.equal(
    saveRefused(attempt, { id: 2 }, { outputStyle: 'terse' }),
    false,
    'the engine deleted `model` exactly as asked — the banner must clear, not stick forever',
  );
});

test('null-vs-absent leniency does not swallow a real divergence on the same key', () => {
  assert.equal(layersDiverge({ model: null }, { model: 'opus' }), true);
  assert.equal(layersDiverge({ model: false }, {}), true, 'false is a value, not an absence');
  assert.equal(layersDiverge({ model: 0 }, {}), true);
  assert.equal(layersDiverge({ model: '' }, {}), true);
});
