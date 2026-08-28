import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rowState, pendingKeys } from '../src/renderer/components/settings/useEngineSettings';

const files = [
  { destination: 'user', path: '/u', exists: true, writable: true },
  { destination: 'project', path: '/p', exists: true, writable: true },
  { destination: 'local', path: '/l', exists: true, writable: true },
] as const;

function snap(over: Record<string, unknown> = {}) {
  return {
    files: [...files], effective: {}, active: {}, provenance: {}, locked: [], ...over,
  } as never;
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
    files: [{ destination: 'user', path: '/u', exists: true, writable: false, parse_error: 'bad json' },
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
