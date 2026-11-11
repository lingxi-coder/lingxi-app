import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseDeepLink } from '../../../packages/deep-links';
import { friendLink } from '../../../packages/teams/friends';
import { appRoutes, resolveRoute, securityZone } from '../src/app/router';
import { clearAddIntent, readAddIntent, retainAddIntent } from '../src/friends/add-intent';
import { friendViewerPath } from '../src/friends/viewer';

const vectors = JSON.parse(readFileSync(new URL('../../../packages/teams/fixtures/friends-vectors.json', import.meta.url), 'utf8'));
const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-', other = 'zY9-zY9-zY9-zY9-zY9-zY9-zY9-zY9_', url = `https://lingxios.net/friends/add?code=${code}`, DAY = 86400000;
const storage = () => { const values = new Map<string, string>(); return { values, getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } }; };

// ── The friend link kept across sign-in (D8) ──

test('add intent survives the stripped URL, sign-in and a closed tab until it is answered', () => {
  const store = storage();
  assert.equal(readAddIntent(url, store, 1000), code);
  assert.equal(retainAddIntent(code, store, 1000), true);
  assert.equal(readAddIntent('https://lingxios.net/friends', store, 2000), code);
  assert.equal(readAddIntent('https://lingxios.net/friends/add', store, 2000 + 6 * DAY), code, 'still pending on the sixth day');
  clearAddIntent(store, code);
  assert.equal(readAddIntent('https://lingxios.net/friends', store, 3000), '');
});

test('only a friend link is an intent: the address bar of the friends page, the shared link, nothing else', () => {
  const store = storage();
  for (const link of [url, `https://lingxios.net/open/friend/add?code=${code}`, `lingxi://friend/add?code=${code}`]) assert.equal(readAddIntent(link, store, 1000), code, link);
  for (const link of [`https://lingxios.net/teams/join?code=${code}`, `https://evil.invalid/friends/add?code=${code}`, `https://lingxios.net/friends/add?code=${code}&x=1`, `https://lingxios.net/friends/add?code=${code}#more`,
    `https://lingxios.net/friends/add?code=${code.slice(1)}`, 'https://lingxios.net/friends/add', 'https://lingxios.net/friends/requests?code=' + code, 'not a url']) assert.equal(readAddIntent(link, store, 1000), '', link);
});

test('a new link replaces the old one, and a refresh cannot extend a link\'s lifetime', () => {
  const store = storage();
  retainAddIntent(code, store, 1000); const first = [...store.values.values()][0];
  retainAddIntent(code, store, 2000); assert.equal([...store.values.values()][0], first, 'the same link keeps its expiry');
  retainAddIntent(other, store, 3000); assert.equal(readAddIntent('https://lingxios.net/friends', store, 4000), other);
  assert.equal(readAddIntent('https://lingxios.net/friends', store, 3000 + 7 * DAY), '', 'seven days, no more');
});

test('a bad record, an expired or implausibly distant one, and a denied store never create a lookup', () => {
  const store = storage(); retainAddIntent(code, store, 1000); const key = [...store.values.keys()][0]!;
  store.setItem(key, '{bad json'); assert.equal(readAddIntent('https://lingxios.net/friends', store, 2000), '');
  store.setItem(key, JSON.stringify({ code, expiresAt: 1000 + 7 * DAY + 1 })); assert.equal(readAddIntent('https://lingxios.net/friends', store, 1000), ''); assert.equal(store.values.size, 0, 'removed, not renewed');
  store.setItem(key, JSON.stringify({ code: 'short', expiresAt: 5000 })); assert.equal(readAddIntent('https://lingxios.net/friends', store, 1000), '');
  const denied = { getItem(): string | null { throw new Error('denied'); }, setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  assert.equal(readAddIntent(url, denied, 1000), code, 'a link in the address bar still works');
  assert.equal(retainAddIntent(code, denied, 1000), false); assert.doesNotThrow(() => clearAddIntent(denied, code));
  assert.equal(retainAddIntent('wrong-code', store, 1000), false); assert.equal(retainAddIntent('', store, 1000), false);
});

test('answering one link leaves a different, newer one pending', () => {
  const store = storage(); retainAddIntent(code, store, 1000); retainAddIntent(other, store, 2000);
  clearAddIntent(store, code); assert.equal(readAddIntent('https://lingxios.net/friends', store, 3000), other);
  clearAddIntent(store, 'wrong'); assert.equal(readAddIntent('https://lingxios.net/friends', store, 3000), other);
  clearAddIntent(store, other); assert.equal(store.values.size, 0);
});

// ── Routes and links (D1, D17) ──

test('/friends and /friends/add are routes of the Teams security zone and nothing else is', () => {
  for (const route of ['/friends', '/friends/add']) { assert.ok((appRoutes as readonly string[]).includes(route), route); assert.equal(resolveRoute(route), route); assert.equal(securityZone(route), 'teams', route); }
  assert.equal(resolveRoute('/Friends/Add/?code=x'), '/friends/add');
  assert.equal(securityZone('/friends/anything'), 'teams');
  for (const route of ['/friendship', '/friend', '/friendsx', '/team', '/']) assert.equal(securityZone(route), 'site', route);
  assert.equal(resolveRoute('/friends/unknown'), '/', 'only the two routes exist');
});

test('links.webViewer: a canonical friend link opens its page, and one that is not a friend link has no friend page', () => {
  assert.ok(vectors.links.webViewer.length >= 4);
  for (const row of vectors.links.webViewer) {
    const link = friendLink(parseDeepLink(row.link));
    assert.equal(link ? friendViewerPath(link) : null, row.expect, row.link);
  }
});

test('the viewer path carries the code only on the page that consumes it, and escapes what it carries', () => {
  assert.equal(friendViewerPath({ kind: 'requests' }), '/friends');
  assert.equal(friendViewerPath({ kind: 'add', code }), `/friends/add?code=${code}`);
  assert.equal(friendViewerPath({ kind: 'chat', uid: 'a_b-9' }), '/friends?chat=a_b-9');
  assert.equal(friendViewerPath({ kind: 'chat', uid: 'a&b=c' }), '/friends?chat=a%26b%3Dc');
});
