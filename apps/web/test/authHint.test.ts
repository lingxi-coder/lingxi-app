import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { SESSION_HINT_KEY, setSessionHint } from '../src/app/sessionHint';

const memory = () => { const map = new Map<string, string>(); return { map, getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v), removeItem: (k: string) => void map.delete(k) }; };

test('the session hint is set while signed in and cleared on sign-out', () => {
  const store = memory();
  setSessionHint(true, store);
  assert.equal(store.map.get(SESSION_HINT_KEY), '1');
  setSessionHint(false, store);
  assert.equal(store.map.has(SESSION_HINT_KEY), false);
});

test('denied storage never throws', () => {
  const denied = { setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  assert.doesNotThrow(() => setSessionHint(true, denied));
  assert.doesNotThrow(() => setSessionHint(true, null));
});

// The redirect runs in index.html's inline script, before any module loads; this runs that script as the browser would.
const inline = /<script>([\s\S]*?)<\/script>/u.exec(readFileSync(new URL('../index.html', import.meta.url), 'utf8'))![1]!;
const landOn = (href: string, signedIn: boolean | 'denied'): string | null => {
  const url = new URL(href, 'https://lingxios.net');
  let replaced: string | null = null;
  const store = memory();
  if (signedIn === true) store.setItem(SESSION_HINT_KEY, '1');
  const localStorage = signedIn === 'denied' ? { getItem() { throw new Error('denied'); } } : store;
  vm.runInNewContext(inline, {
    localStorage, URLSearchParams, JSON, document: { documentElement: { dataset: {} } },
    location: { pathname: url.pathname, search: url.search, hash: url.hash, replace: (to: string) => { replaced = to; } },
  });
  return replaced;
};

test('only a fresh signed-in load of the landing page goes to Chat', () => {
  assert.equal(landOn('/', true), '/chat');
  assert.equal(landOn('/?utm=x', true), '/chat?utm=x');
  assert.equal(landOn('/?home', true), null, '?home keeps the landing page');
  assert.equal(landOn('/#landing-scenarios', true), null, 'an anchor on the landing page stays there');
  assert.equal(landOn('/', false), null);
  assert.equal(landOn('/', 'denied'), null);
  assert.equal(landOn('/docs', true), null);
});
