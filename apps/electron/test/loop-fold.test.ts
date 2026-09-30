import { strict as assert } from 'node:assert';
import test from 'node:test';

import { foldOwners, visibleRows, type FoldableRow } from '../src/renderer/components/loopFold';

/** `w` = a wakeup row (with the streak it folded), `r` = an ordinary row. */
function rows(spec: readonly (readonly [string, number | undefined])[]): FoldableRow[] {
  return spec.map(([id, streak]) => ({
    id,
    type: 'narration',
    ...(streak === undefined ? {} : { loopWakeupStreak: streak }),
  }));
}

const allClosed = () => false;
const allOpen = () => true;

test('nothing folded leaves the array untouched, identity included', () => {
  const items = rows([['w1', 0], ['a', undefined]]);
  assert.equal(visibleRows(items, [], allClosed), items);
});

test('a folded run hides until its fold row is opened', () => {
  // w1 announced a tick; `a`/`b` are what that tick produced; w2 folds it.
  const items = rows([['w1', 0], ['a', undefined], ['b', undefined], ['w2', 1]]);
  const folded = ['w1', 'a', 'b'];

  assert.deepEqual(
    visibleRows(items, folded, allClosed).map((r) => r.id),
    ['w2'],
    'a closed fold row hides the whole quiet group',
  );
  assert.deepEqual(
    visibleRows(items, folded, allOpen).map((r) => r.id),
    ['w1', 'a', 'b', 'w2'],
    'opening it brings the group back',
  );
  assert.deepEqual([...foldOwners(items, folded)], [['w1', 'w2'], ['a', 'w2'], ['b', 'w2']]);
});

/**
 * The streak is cumulative: the third quiet tick reports 3, not 1. So a later
 * fold SUBSUMES the earlier one, and every row in the run — including the
 * wakeup row that used to be the fold row — answers to the newest fold row.
 * Anything else would leave a fold row hidden behind another fold with its own
 * group still attached to it.
 */
test('a later fold subsumes the earlier one', () => {
  const items = rows([['w1', 0], ['a', undefined], ['w2', 1], ['b', undefined], ['w3', 2]]);
  const owners = foldOwners(items, ['w1', 'a', 'w2', 'b']);
  assert.deepEqual(
    [...owners.values()],
    ['w3', 'w3', 'w3', 'w3'],
    'the newest fold row owns the whole run',
  );
});

/**
 * A run with no fold row after it is a fold still in flight. Hiding it would
 * leave rows on screen with nothing to click to get them back.
 */
test('a trailing run with no fold row after it stays visible', () => {
  const items = rows([['w1', 0], ['a', undefined]]);
  assert.deepEqual(
    visibleRows(items, ['w1', 'a'], allClosed).map((r) => r.id),
    ['w1', 'a'],
  );
});

test('an ordinary wakeup (streak 0) is not a fold row', () => {
  const items = rows([['w1', 0], ['a', undefined], ['w2', 0]]);
  assert.equal(foldOwners(items, ['w1', 'a']).size, 0);
});
