import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rowState, pendingKeys } from '../src/renderer/components/settings/useEngineSettings';
import type { SettingsFile, SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';

const files: SettingsFile[] = [
  { layer: 'user', path: '/u', exists: true, parsed: true },
  { layer: 'project', path: '/p', exists: true, parsed: true },
  { layer: 'local', path: '/l', exists: true, parsed: true },
];

// Typed against SettingsSnapshot (not cast away) so a fixture whose shape
// drifts from the wire's field names — as `destination`/`writable` once did —
// fails to compile instead of silently missing every `.find`.
function snap(over: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    files: [...files], effective: {}, active: {}, provenance: {}, locked: [], layers: {},
    mergedKeys: [], ...over,
  };
}

test('unset when no layer defines the key', () => {
  assert.deepEqual(rowState(snap(), 'outputStyle', 'user'), { kind: 'unset' });
});

test('set-here when the editing layer supplies the effective value', () => {
  const s = snap({ effective: { outputStyle: 'terse' }, provenance: { outputStyle: 'user' } });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'set-here' });
});

test('overridden when a higher layer wins over the editing layer', () => {
  const s = snap({ effective: { outputStyle: 'loud' }, provenance: { outputStyle: 'project' } });
  assert.deepEqual(
    rowState(s, 'outputStyle', 'user'), { kind: 'overridden', by: 'project' },
    'editing user while project wins is the common case, not an edge case',
  );
});

// Task 17b. The engine deep-merges `hooks` (and every other key in its
// `schema::MERGE_STRATEGIES` table), so when two layers each define part of
// it the effective value is neither layer's. `provenance` still names the
// highest CONTRIBUTOR — here `project` — and rendering that as a badge would
// tell the user the value came from `project` when half of it came from
// `user`. `merged` exists so the UI can say the true thing instead.
test('merged when the value is a cross-layer union, not any one layer', () => {
  const s = snap({
    effective: { hooks: { PreToolUse: {}, PostToolUse: {} } },
    provenance: { hooks: 'project' },
    mergedKeys: ['hooks'],
  });
  assert.deepEqual(
    rowState(s, 'hooks', 'user'), { kind: 'merged', locked: false },
    'a merged key must not resolve to a single-layer state — that badge would be false',
  );
  // The same fixture MINUS `mergedKeys` is the pre-17b behaviour, and it is
  // exactly the lie: `overridden` claims `project` beat `user`, when in fact
  // both contributed. Pinning it here proves the assertion above is not
  // passing for some unrelated reason.
  assert.deepEqual(
    rowState(snap({ effective: s.effective, provenance: s.provenance }), 'hooks', 'user'),
    { kind: 'overridden', by: 'project' },
  );
});

// The engine folds the managed layer through the SAME merger as the file
// layers, so an administrator-pinned key can ALSO be a union. `locked` alone
// would draw the `managed` single-layer badge (`LockedBadge`); `merged` alone
// would lose "you cannot edit this". The state carries both.
test('a merged key that is also policy-pinned reports merged AND locked', () => {
  const s = snap({
    effective: { permissions: { allow: [], deny: [] } },
    provenance: { permissions: 'managed' },
    locked: ['permissions'],
    mergedKeys: ['permissions'],
  });
  assert.deepEqual(rowState(s, 'permissions', 'user'), { kind: 'merged', locked: true });
});

// A key only one layer defines is NOT merged, so its badge stays honest —
// a `mergedKeys` that swallowed every key would make the field useless.
test('a key outside mergedKeys keeps its single-layer state', () => {
  const s = snap({
    effective: { outputStyle: 'loud' },
    provenance: { outputStyle: 'project' },
    mergedKeys: ['hooks'],
  });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'overridden', by: 'project' });
});

// The invariant behind the badge, stated once rather than per fixture: for a
// merged key there is no editing layer from which a single-layer answer is
// true, so `rowState` must never hand one back — not `set-here`, not
// `overridden`, not `inherited`, not `locked` (whose badge is `managed`).
// This is what a future reordering of the checks inside `rowState` would
// break, and a fixture-by-fixture test would not necessarily catch.
test('no editing layer yields a single-layer state for a merged key', () => {
  const s = snap({
    effective: { hooks: {} },
    provenance: { hooks: 'managed' },
    locked: ['hooks'],
    mergedKeys: ['hooks'],
  });
  for (const layer of ['user', 'project', 'local'] as const) {
    assert.equal(
      rowState(s, 'hooks', layer).kind, 'merged',
      `editing ${layer}: a merged value belongs to no single layer, so no single-layer state is true`,
    );
  }
});

test('inherited when the editing layer is higher than the winner', () => {
  const s = snap({ effective: { outputStyle: 'terse' }, provenance: { outputStyle: 'user' } });
  assert.deepEqual(rowState(s, 'outputStyle', 'local'), { kind: 'inherited', from: 'user' });
});

test('locked when the managed layer pins the key', () => {
  const s = snap({ effective: { outputStyle: 'x' }, provenance: { outputStyle: 'managed' }, locked: ['outputStyle'] });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'locked' });
});

test('layer-broken when the editing layer failed to parse', () => {
  const s = snap({
    files: [{ layer: 'user', path: '/u', exists: true, parsed: false, parse_error: 'bad json' },
            files[1], files[2]],
  });
  assert.deepEqual(rowState(s, 'outputStyle', 'user'), { kind: 'layer-broken', error: 'bad json' });
});

test('pending names only the keys the running session actually differs on', () => {
  const differing = snap({ effective: { outputStyle: 'terse' }, active: { outputStyle: 'loud' } });
  assert.deepEqual(
    pendingKeys(differing), ['outputStyle'],
    'a key on disk that differs from what the session loaded is pending',
  );
});

test('pending is empty when disk and session agree (the A/B for the test above)', () => {
  const same = snap({ effective: { outputStyle: 'terse' }, active: { outputStyle: 'terse' } });
  assert.deepEqual(
    pendingKeys(same), [],
    'if this also reported pending, the test above would prove nothing',
  );
});
