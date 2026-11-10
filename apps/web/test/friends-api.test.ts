import assert from 'node:assert/strict';
import test from 'node:test';
import type { User } from 'firebase/auth';
import type { Firestore } from 'firebase/firestore';
import { TeamsApiError } from '../../../packages/teams';
import { firestoreBlock, firestoreFriendCard, firestoreFriendName } from '../../../packages/teams/firestore';
import { friendsTransport } from '../../../packages/teams/friends';
import { webFriendsApi } from '../src/friends/api';
import { pairTeamId } from './support/fakeFriends';

// ── The transport ──

test('friendsTransport: the profile is a GET and every action a POST of JSON to /v1/friends, with the bearer token and no cache or redirect', async () => {
  const calls: { url: string; init: RequestInit }[] = [];
  const request = friendsTransport('https://lingxios.net', async () => 'synthetic-token', async (url, init) => { calls.push({ url: String(url), init: init! }); return Response.json({ ok: true }); });
  await request('/profile');
  await request('', { action: 'lookup', handle: 'sam' });
  assert.deepEqual(calls.map(call => [call.url, call.init.method]), [['https://lingxios.net/v1/friends/profile', 'GET'], ['https://lingxios.net/v1/friends', 'POST']]);
  assert.deepEqual(calls[0]!.init.headers, { Authorization: 'Bearer synthetic-token' });
  assert.deepEqual(calls[1]!.init.headers, { Authorization: 'Bearer synthetic-token', 'Content-Type': 'application/json' });
  assert.equal(calls[1]!.init.body, JSON.stringify({ action: 'lookup', handle: 'sam' })); assert.equal(calls[0]!.init.body, undefined);
  for (const call of calls) { assert.equal(call.init.cache, 'no-store'); assert.equal(call.init.redirect, 'error'); assert.ok(call.init.signal); }
});

test('friendsTransport: a refusal is a TeamsApiError with the server\'s code and status, and an answer without one is a service failure', async () => {
  const answer = (response: Response) => friendsTransport('', async () => 't', async () => response)('', { action: 'rotateCode' });
  await assert.rejects(answer(Response.json({ error: { code: 'RATE_LIMITED' } }, { status: 429 })), (error: unknown) => error instanceof TeamsApiError && error.code === 'RATE_LIMITED' && error.status === 429 && error.message === 'RATE_LIMITED');
  await assert.rejects(answer(Response.json({ nothing: true }, { status: 502 })), (error: unknown) => error instanceof TeamsApiError && error.code === 'SERVICE_UNAVAILABLE' && error.status === 502);
  await assert.rejects(answer(Response.json(null, { status: 500 })), /SERVICE_UNAVAILABLE/u);
  await assert.rejects(answer(new Response('<html>', { status: 200 })), SyntaxError, 'an answer that is not JSON is not a result');
});

// ── The Web adapter ──

const user = (uid: string, token = async () => 'synthetic-token') => ({ uid, getIdToken: token }) as unknown as User;

test('the Web adapter sends the profile and the actions as the signed-in account and answers with the result', async context => {
  const urls: string[] = [];
  context.mock.method(globalThis, 'fetch', async (url: string, init: RequestInit) => { urls.push(`${init.method} ${url}`); return Response.json(init.method === 'GET' ? { uid: 'alice', displayName: 'Alice' } : { ok: true }); });
  const api = webFriendsApi(user('alice'), {} as Firestore, () => 'alice');
  assert.deepEqual(await api.profile(), { uid: 'alice', displayName: 'Alice' });
  assert.deepEqual(await api.execute({ action: 'revokeCode' }), { ok: true });
  assert.deepEqual(urls, ['GET /v1/friends/profile', 'POST /v1/friends']);
  api.close();
});

test('the Web adapter does nothing for another account: not before the token, not after the answer, not once closed', async context => {
  let current = 'alice', release: (() => void) | undefined, requests = 0;
  context.mock.method(globalThis, 'fetch', async () => { requests++; return new Promise<Response>(resolve => { release = () => resolve(Response.json({ ok: true })); }); });
  const api = webFriendsApi(user('alice'), {} as Firestore, () => current);
  const late = api.execute({ action: 'unblock', otherUid: 'bob' });
  while (!release) await new Promise(resolve => setTimeout(resolve, 0));
  current = 'carol'; release();
  await assert.rejects(late, /INVALID_SESSION/u, 'the answer belongs to an account that left');
  await assert.rejects(api.profile(), /INVALID_SESSION/u);
  assert.equal(requests, 1, 'and the second account\'s token never goes out under the first');

  current = 'alice';
  let resume!: (token: string) => void;
  const slow = webFriendsApi(user('alice', () => new Promise<string>(resolve => { resume = resolve; })), {} as Firestore, () => current);
  const delayed = slow.execute({ action: 'rotateCode' });
  slow.close(); resume('synthetic-token');
  await assert.rejects(delayed, /INVALID_SESSION/u);
  assert.equal(requests, 1, 'a closed adapter sends nothing');
  assert.throws(() => slow.watchCards(() => {}, () => {}), /INVALID_SESSION/u);
  assert.throws(() => slow.watchBlocks(() => {}, () => {}), /INVALID_SESSION/u);
  assert.equal(await slow.readProfile('bob'), null);
});

test('a friend\'s profile is only read for a well-formed uid', async () => {
  const api = webFriendsApi(user('alice'), {} as Firestore, () => 'alice');
  for (const friend of ['', 'a/b', 'a b', '../x', 'x'.repeat(129)]) assert.equal(await api.readProfile(friend), null, friend);
  api.close();
});

// ── The documents ──

const at = (ms: number) => ({ toDate: () => new Date(ms) });
const pointer = { id: 'm1', seq: 4, at: at(5000), author: { kind: 'user', uid: 'bob' }, preview: 'Hello' };

test('a card becomes plain values: milliseconds, the Teams pointer, counters that are counts', () => {
  assert.deepEqual(firestoreFriendCard('bob', { state: 'friends', displayName: 'Bob', handle: 'bobby', note: '', since: at(1000), teamId: pairTeamId('bob'), generation: 2, lastSeq: 4, lastReadSeq: 1, lastChangeSeq: 6, lastMessage: pointer }), {
    uid: 'bob', state: 'friends', displayName: 'Bob', handle: 'bobby', since: 1000, teamId: pairTeamId('bob'), generation: 2, lastSeq: 4, lastReadSeq: 1, lastChangeSeq: 6,
    lastMessage: { id: 'm1', seq: 4, at: new Date(5000).toISOString(), author: { kind: 'user', uid: 'bob' }, preview: 'Hello' },
  });
  assert.deepEqual(firestoreFriendCard('carol', { state: 'incoming', displayName: 'Carol', note: 'Hi' }), { uid: 'carol', state: 'incoming', displayName: 'Carol', note: 'Hi', since: 0 });
});

test('one bad card never fails the list: a pointer that is not whole counts as absent, a counter that is not a count as missing, and an unusable card is left out', () => {
  const base = { state: 'friends', displayName: 'Bob', since: at(1), teamId: pairTeamId('bob'), generation: 1 };
  assert.equal(firestoreFriendCard('bob', { ...base, lastMessage: { id: 'm1', seq: 4 } })!.lastMessage, undefined, 'a half pointer is no message');
  assert.equal(firestoreFriendCard('bob', { ...base, lastMessage: 'text' })!.lastMessage, undefined);
  assert.equal(firestoreFriendCard('bob', { ...base, lastMessage: { ...pointer, at: null } })!.lastMessage?.at, null, 'a pointer the server has not stamped yet');
  for (const counter of [-1, 1.5, '3', null, Number.NaN, 2 ** 60]) {
    const value = firestoreFriendCard('bob', { ...base, lastSeq: counter, lastReadSeq: counter, lastChangeSeq: counter })!;
    assert.deepEqual([value.lastSeq, value.lastReadSeq, value.lastChangeSeq], [undefined, undefined, undefined], String(counter));
  }
  assert.equal(firestoreFriendCard('bob', { ...base, generation: 0 })!.generation, undefined, 'a generation starts at 1');
  for (const broken of [{ ...base, state: 'strangers' }, { ...base, state: undefined }, { ...base, displayName: 7 }, { ...base, displayName: undefined }]) assert.equal(firestoreFriendCard('bob', broken), undefined);
});

test('a block keeps the name it was made with, and a profile gives only what a friend can change', () => {
  assert.deepEqual(firestoreBlock('dave', { at: at(9000), displayName: 'Dave', handle: 'dave' }), { uid: 'dave', at: 9000, displayName: 'Dave', handle: 'dave' });
  assert.deepEqual(firestoreBlock('erin', { at: at(2000) }), { uid: 'erin', at: 2000, displayName: '' }, 'a block made before names were kept');
  assert.deepEqual(firestoreBlock('fay', {}), { uid: 'fay', at: 0, displayName: '' });
  assert.deepEqual(firestoreFriendName({ displayName: 'Bob B', handle: 'bobby', email: 'b@example.com', findByEmail: true }), { displayName: 'Bob B', handle: 'bobby' });
  assert.deepEqual(firestoreFriendName({ displayName: 3, handle: '' }), {});
  assert.equal(firestoreFriendName(undefined), null);
});
