/**
 * The transcript's collapse store.
 *
 * The rule under test is the one the Stage got wrong: item ids restart at `i1`
 * in every session (`conversation.ts` `nextId: 1`), so a map that outlives the
 * session hands session A's choices to session B's unrelated items.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  collapseFor,
  collapseInitial,
  collapseOpen,
  collapseSet,
} from '../src/renderer/components/collapseStore';

test('a choice is remembered within its own session', () => {
  let store = collapseInitial('session-a');
  store = collapseSet(store, 'session-a', 'i3', false);
  assert.equal(collapseOpen(store, 'session-a', 'i3'), false);
  // An item nobody touched keeps no opinion, so the row falls back to its own
  // default rather than to `false`.
  assert.equal(collapseOpen(store, 'session-a', 'i4'), undefined);
});

test('collapse state does NOT bleed into the next session', () => {
  // Session A: the user collapses the third item.
  let store = collapseInitial('session-a');
  store = collapseSet(store, 'session-a', 'i3', false);

  // Session B starts. Its generated ids restart at i1, so `i3` is a completely
  // different block — it must arrive with no inherited choice.
  assert.equal(collapseOpen(store, 'session-b', 'i3'), undefined);
  const fresh = collapseFor(store, 'session-b');
  assert.equal(collapseOpen(fresh, 'session-b', 'i3'), undefined);
  assert.deepEqual(Object.keys(fresh.open), []);
});

test('a write from a new session starts a fresh map instead of merging', () => {
  let store = collapseSet(collapseInitial('session-a'), 'session-a', 'i3', false);
  store = collapseSet(store, 'session-b', 'i1', true);
  assert.equal(store.sessionKey, 'session-b');
  assert.equal(collapseOpen(store, 'session-b', 'i1'), true);
  assert.deepEqual(Object.keys(store.open), ['i1'], 'session A entries must not survive');
});

test('collapseFor keeps identity while the session is unchanged', () => {
  const store = collapseSet(collapseInitial('s'), 's', 'i1', true);
  assert.equal(collapseFor(store, 's'), store, 'no allocation per render');
});

test('an id that collides with Object.prototype is not reported as open', () => {
  const store = collapseInitial('s');
  for (const id of ['constructor', 'toString', 'valueOf', '__proto__']) {
    assert.equal(collapseOpen(store, 's', id), undefined, id);
  }
  const set = collapseSet(store, 's', 'constructor', true);
  assert.equal(collapseOpen(set, 's', 'constructor'), true);
  assert.equal(collapseOpen(set, 's', 'toString'), undefined);
});
