import assert from 'node:assert/strict';
import test from 'node:test';
import { hasSessionHint, landingRedirect, setSessionHint } from '../src/app/sessionHint';

const memory = () => { const map = new Map<string, string>(); return { getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v), removeItem: (k: string) => void map.delete(k) }; };

test('the session hint is set while signed in and cleared on sign-out', () => {
  const store = memory();
  assert.equal(hasSessionHint(store), false);
  setSessionHint(true, store);
  assert.equal(hasSessionHint(store), true);
  setSessionHint(false, store);
  assert.equal(hasSessionHint(store), false);
});

test('denied storage never throws and gives no shortcut', () => {
  const denied = { getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  assert.doesNotThrow(() => setSessionHint(true, denied));
  assert.equal(hasSessionHint(denied), false);
  assert.equal(hasSessionHint(null), false);
});

test('only a fresh signed-in load of the landing page goes to Chat', () => {
  assert.equal(landingRedirect('/', '', true), '/chat');
  assert.equal(landingRedirect('/', '?utm=x', true), '/chat?utm=x');
  assert.equal(landingRedirect('/', '?home', true), null, '?home keeps the landing page');
  assert.equal(landingRedirect('/', '', false), null);
  assert.equal(landingRedirect('/docs', '', true), null);
});
