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
    files: [...files], effective: {}, active: {}, provenance: {}, locked: [], layers: {}, ...over,
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
