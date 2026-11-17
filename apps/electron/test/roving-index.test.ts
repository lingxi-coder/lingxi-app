import assert from 'node:assert/strict';
import test from 'node:test';

import { rovingIndex } from '../src/renderer/components/rovingIndex';

test('Home and End go to the first and the last item, wherever the focus is', () => {
  for (const current of [-1, 0, 2, 4]) {
    assert.equal(rovingIndex(5, current, 'Home'), 0, `from ${current}`);
    assert.equal(rovingIndex(5, current, 'End'), 4, `from ${current}`);
  }
});

test('the arrow keys of a column go to the next and the previous item, and stop at the ends', () => {
  assert.equal(rovingIndex(5, 2, 'ArrowDown'), 3);
  assert.equal(rovingIndex(5, 2, 'ArrowUp'), 1);
  assert.equal(rovingIndex(5, 4, 'ArrowDown'), 4, 'the last item has no next one');
  assert.equal(rovingIndex(5, 0, 'ArrowUp'), 0, 'the first item has no previous one');
  assert.equal(rovingIndex(1, 0, 'ArrowDown'), 0);
});

test('a tab strip names its own keys and goes round at the ends', () => {
  const tabs = { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true };
  assert.equal(rovingIndex(9, 3, 'ArrowRight', tabs), 4);
  assert.equal(rovingIndex(9, 3, 'ArrowLeft', tabs), 2);
  assert.equal(rovingIndex(9, 8, 'ArrowRight', tabs), 0, 'after the last comes the first');
  assert.equal(rovingIndex(9, 0, 'ArrowLeft', tabs), 8, 'before the first comes the last');
  assert.equal(rovingIndex(9, 3, 'ArrowDown', tabs), undefined, 'the keys of a column are not the keys of a strip');
  assert.equal(rovingIndex(9, 3, 'ArrowRight'), undefined, 'and the other way round');
});

test('before anything has the focus the first press of next lands on the first item and of previous on the last', () => {
  assert.equal(rovingIndex(4, -1, 'ArrowDown'), 0);
  assert.equal(rovingIndex(4, -1, 'ArrowUp'), 3);
  assert.equal(rovingIndex(4, -1, 'ArrowRight', { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true }), 0);
  assert.equal(rovingIndex(4, -1, 'ArrowLeft', { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true }), 3);
});

test('other keys move nothing, and neither does a list with nothing in it', () => {
  for (const key of ['Enter', ' ', 'a', 'Tab', 'PageDown', 'Escape']) assert.equal(rovingIndex(5, 2, key), undefined, key);
  for (const key of ['Home', 'End', 'ArrowDown', 'ArrowUp']) assert.equal(rovingIndex(0, -1, key), undefined, key);
});
