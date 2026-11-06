import assert from 'node:assert/strict';
import { test } from 'node:test';
import { clearInviteIntent, readInviteIntent, retainInviteIntent } from '../src/teams/invite-intent';
const code = 'a'.repeat(32), other = 'b'.repeat(32), url = `https://lingxios.net/teams/join?code=${code}`;
function storage() {
  const values = new Map<string, string>();
  return { values, getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } };
}
test('invite intent survives URL stripping, auth interruption and refresh until explicitly resolved', () => {
  const store = storage(); assert.equal(readInviteIntent(url, store, 1000), code);
  assert.equal(retainInviteIntent(code, store, 1000), true);
  assert.equal(readInviteIntent('https://lingxios.net/teams/join', store, 2000), code);
  clearInviteIntent(store, code); assert.equal(readInviteIntent('https://lingxios.net/teams/join', store, 3000), '');
});
test('new valid invitation replaces the previous one, and refresh cannot extend its lifetime', () => {
  const store = storage(); retainInviteIntent(code, store, 1000); const original = [...store.values.values()][0];
  retainInviteIntent(code, store, 2000); assert.equal([...store.values.values()][0], original);
  retainInviteIntent(other, store, 3000); assert.equal(readInviteIntent('https://lingxios.net/teams', store, 4000), other);
  assert.equal(readInviteIntent('https://lingxios.net/teams', store, 3000 + 7 * 86400000), '');
});
test('malformed state, untrusted URLs and denied browser storage never create an automatic join', () => {
  const store = storage(); retainInviteIntent(code, store, 1000);
  store.setItem([...store.values.keys()][0]!, '{bad json'); assert.equal(readInviteIntent('https://evil.invalid/teams/join?code=' + code, store, 2000), '');
  const denied = { getItem(): string | null { throw new Error('denied'); }, setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  assert.equal(readInviteIntent(url, denied, 1000), code);
  assert.equal(retainInviteIntent(code, denied, 1000), false); assert.doesNotThrow(() => clearInviteIntent(denied, code));
  assert.equal(retainInviteIntent('wrong-code', store, 1000), false);
});

test('expired or implausibly distant persisted invitations are removed rather than renewed', () => {
  const store = storage(); retainInviteIntent(code, store, 1000); const key = [...store.values.keys()][0]!;
  assert.equal(readInviteIntent('https://lingxios.net/teams', store, 1000 + 7 * 86400000), '');
  assert.equal(store.values.size, 0);
  store.setItem(key, JSON.stringify({ code, expiresAt: 1000 + 7 * 86400000 + 1 }));
  assert.equal(readInviteIntent('https://lingxios.net/teams', store, 1000), ''); assert.equal(store.values.size, 0);
});

test('resolving an older or absent intent preserves a different newer invitation', () => {
  const store = storage(); retainInviteIntent(code, store, 1000); retainInviteIntent(other, store, 2000);
  const newer = [...store.values.values()][0];
  clearInviteIntent(store, code); assert.equal([...store.values.values()][0], newer);
  clearInviteIntent(store, ''); assert.equal([...store.values.values()][0], newer);
  clearInviteIntent(store, other); assert.equal(store.values.size, 0);
});
