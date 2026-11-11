import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { assertFails, assertSucceeds, initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { collection, doc, getDoc, getDocs } from 'firebase/firestore';
import { z } from 'zod';
import { FRIEND_LIMIT, FriendsStore, LOOKUP_LIMIT, LOOKUP_WINDOW_MS, OFFER_GRACE_MS, OFFER_LIMIT, OFFER_TTL_MS, OUTGOING_LIMIT, friendNotifier, pairKey, pairTeamId } from '../src/friends.mjs';
import { PAIR_CONVERSATION_ID, TEAM_MEMBER_LIMIT, TeamsStore, USER_TEAM_LIMIT, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { createTeamConversation } from '../src/team-conversations.mjs';
import { onTeamMessageCreated } from '../src/team-message-push.mjs';

const projectId = 'demo-lingxi-teams-t0', options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 120000 };
const alice = { uid: 'alice', name: 'Alice' }, bob = { uid: 'bob', name: 'Bob' }, carol = { uid: 'carol', name: 'Carol' }, dave = { uid: 'dave' };
const sha = value => createHash('sha256').update(value).digest('hex');
let app, db, environment, friends, teams, messages, now, events, accounts, auth, serial = 0;
const mail = person => `${person.uid}@example.com`;

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'friends-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore(); now = Date.now(); events = []; accounts = new Map();
  // Admin Auth stand-in: an address maps to an account, as getUserByEmail does. Everyone has a verified one.
  for (const person of [alice, bob, carol, dave]) accounts.set(mail(person), { uid: person.uid, emailVerified: true, ...(person.name ? { displayName: person.name } : {}) });
  auth = { async getUserByEmail(email) {
    const found = accounts.get(email);
    if (!found) throw Object.assign(new Error('There is no user record'), { code: 'auth/user-not-found' });
    return found;
  } };
  friends = new FriendsStore(db, () => now, { auth, notify: async event => { events.push(event); } });
  teams = new TeamsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
});
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

const act = (actor, body) => friends.execute(actor, body);
const rid = () => `request-${String(++serial).padStart(6, '0')}`;
const data = async path => (await db.doc(path).get()).data();
const ids = async path => (await db.collection(path).get()).docs.map(doc => doc.id).sort();
const rejects = (promise, status, code) => assert.rejects(promise, { status, code });
const link = async user => (await friends.profile(user)).friendCode.code;
// Friends through the friend link (instant), or through request and accept.
async function linked(owner, visitor) { return act(visitor, { action: 'redeem', code: await link(owner), requestId: rid() }); }
async function befriend(requester, accepter) {
  await friends.profile(accepter);
  await request(requester, accepter);
  return act(accepter, { action: 'accept', otherUid: requester.uid });
}
// A request names its target by proof. The default proof is the target's verified email.
const request = (from, to, extra = {}) => act(from, { action: 'request', via: 'email', email: mail(to), requestId: rid(), ...extra });
const teamsIn = async () => (await db.collection('teams').get()).docs.filter(doc => doc.id.startsWith('f_')).map(doc => doc.id);
const activePairTeams = async () => (await db.collection('teams').where('state', '==', 'active').get()).docs.map(doc => doc.id).filter(id => id.startsWith('f_')).sort();
// The friendship document, both cards and the pair team must always tell the same story.
async function consistent(a, b) {
  const [x, y] = [a, b].map(user => user.uid);
  const [f, ca, cb] = await Promise.all([data(`friendships/${pairKey(x, y)}`), data(`users/${x}/friends/${y}`), data(`users/${y}/friends/${x}`)]);
  if (f?.state === 'friends') {
    assert.equal(ca.state, 'friends'); assert.equal(cb.state, 'friends'); assert.equal(ca.teamId, f.teamId); assert.equal(cb.teamId, f.teamId);
    assert.equal(f.teamId, pairTeamId(x, y, f.epoch));
    const team = await data(`teams/${f.teamId}`);
    assert.equal(team.state, 'active'); assert.equal(team.kind, 'pair'); assert.equal(team.memberCount, 2);
    assert.deepEqual(await ids(`teams/${f.teamId}/members`), [x, y].sort());
    return 'friends';
  }
  if (f?.state === 'pending') {
    const [sent, received] = f.requester === x ? [ca, cb] : [cb, ca];
    assert.equal(sent.state, 'outgoing'); assert.equal(received.state, 'incoming');
    return 'pending';
  }
  assert.equal(ca, undefined); assert.equal(cb, undefined);
  if (f?.epoch) {
    const old = pairTeamId(x, y, f.epoch);
    assert.equal((await data(`teams/${old}`)).state, 'deleted'); assert.deepEqual(await ids(`teams/${old}/members`), []);
  }
  return f ? 'ended' : 'none';
}

test('profile: first use creates the profile and one friend code; reading again changes nothing', options, async () => {
  const first = await friends.profile(alice);
  assert.deepEqual(Object.keys(first).sort(), ['displayName', 'findByEmail', 'findByHandle', 'friendCode', 'uid']);
  assert.deepEqual([first.uid, first.displayName, first.findByHandle, first.findByEmail], ['alice', 'Alice', true, true]);
  assert.match(first.friendCode.code, /^[A-Za-z0-9_-]{32}$/u);
  assert.equal(first.friendCode.link, `https://lingxios.net/open/friend/add?code=${first.friendCode.code}`);
  const stored = await data('profiles/alice'), entry = await data(`friendCodes/${sha(first.friendCode.code)}`);
  assert.equal(stored.friendCodeId, sha(first.friendCode.code));
  assert.ok(!JSON.stringify(stored).includes(first.friendCode.code), 'the raw code never sits in a client-readable document');
  assert.deepEqual([entry.uid, entry.code, entry.revoked], ['alice', first.friendCode.code, false]);
  const before = (await db.doc('profiles/alice').get()).updateTime.toMillis();
  now += 5000;
  assert.deepEqual(await friends.profile(alice), first);
  assert.deepEqual(await act(alice, { action: 'profile' }), first);
  assert.equal((await db.doc('profiles/alice').get()).updateTime.toMillis(), before, 'a plain read writes nothing');
  assert.equal((await db.collection('friendCodes').get()).size, 1);
  // The default name comes from the token, falling back to Member.
  assert.equal((await friends.profile(dave)).displayName, 'Member');
  assert.equal((await friends.profile({ uid: 'long', name: `  ${'n'.repeat(100)} ` })).displayName, 'n'.repeat(80));
});

test('profile: name and discoverability edits stick; the handle is case-folded and swapped with its lock', options, async () => {
  await friends.profile(alice); await friends.profile(bob);
  const edited = await act(alice, { action: 'profile', displayName: '  Alice W  ', findByHandle: false, findByEmail: false, handle: 'Alice_W' });
  assert.deepEqual([edited.displayName, edited.handle, edited.findByHandle, edited.findByEmail], ['Alice W', 'Alice_W', false, false]);
  assert.deepEqual(await data('handles/alice_w'), { uid: 'alice' });
  const stored = await data('profiles/alice');
  assert.deepEqual([stored.handle, stored.handleLower], ['Alice_W', 'alice_w']);
  await rejects(act(bob, { action: 'profile', handle: 'ALICE_w' }), 409, 'HANDLE_TAKEN');
  assert.equal((await data('profiles/bob')).handle, undefined);
  await act(alice, { action: 'profile', handle: 'alice_2' });
  assert.equal((await db.doc('handles/alice_w').get()).exists, false);
  assert.deepEqual(await data('handles/alice_2'), { uid: 'alice' });
  await act(alice, { action: 'profile', handle: 'ALICE_2' }); // only the display case changes
  assert.equal((await data('profiles/alice')).handle, 'ALICE_2');
  assert.deepEqual(await ids('handles'), ['alice_2']);
  await act(bob, { action: 'profile', handle: 'alice_w' }); // released names are free again
  assert.deepEqual(await ids('handles'), ['alice_2', 'alice_w']);
  assert.equal((await data('handles/alice_w')).uid, 'bob');
  const same = await act(alice, { action: 'profile', handle: 'alice_2' });
  assert.equal(same.handle, 'alice_2');
  assert.equal((await friends.profile(alice)).friendCode.code, edited.friendCode.code, 'editing never changes the friend code');
});

test('profile: users racing for one handle — exactly one wins and nobody else holds a lock', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  const results = await Promise.allSettled([alice, bob, carol].map((user, index) => act(user, { action: 'profile', handle: index % 2 ? 'Contested' : 'contested' })));
  assert.equal(results.filter(result => result.status === 'fulfilled').length, 1);
  for (const failure of results.filter(result => result.status === 'rejected')) assert.equal(failure.reason.code, 'HANDLE_TAKEN');
  const winner = [alice, bob, carol][results.findIndex(result => result.status === 'fulfilled')];
  assert.deepEqual(await data('handles/contested'), { uid: winner.uid });
  const holders = (await db.collection('profiles').get()).docs.filter(doc => doc.data().handleLower === 'contested');
  assert.deepEqual(holders.map(doc => doc.id), [winner.uid]);
  assert.deepEqual(await ids('handles'), ['contested']);
  // A first call racing itself still ends with one profile and one active code.
  const twice = await Promise.all([friends.profile(dave), friends.profile(dave), friends.profile(dave)]);
  assert.equal(new Set(twice.map(value => value.friendCode.code)).size, 1);
  assert.deepEqual((await db.collection('friendCodes').where('uid', '==', 'dave').get()).docs.map(doc => doc.data().revoked), [false]);
});

test('friend code: rotating revokes the old code in the same transaction, revoking leaves none and is not undone by reading', options, async () => {
  const first = (await friends.profile(alice)).friendCode.code;
  await friends.profile(bob);
  const rotated = await act(alice, { action: 'rotateCode' });
  assert.notEqual(rotated.code, first);
  assert.equal(rotated.link, `https://lingxios.net/open/friend/add?code=${rotated.code}`);
  assert.equal((await data(`friendCodes/${sha(first)}`)).revoked, true);
  assert.equal((await data(`friendCodes/${sha(rotated.code)}`)).revoked, false);
  assert.equal((await data('profiles/alice')).friendCodeId, sha(rotated.code));
  assert.equal((await friends.profile(alice)).friendCode.code, rotated.code);
  await rejects(act(bob, { action: 'lookup', code: first }), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'redeem', code: first, requestId: rid() }), 404, 'NOT_FOUND');
  assert.equal((await act(bob, { action: 'lookup', code: rotated.code })).uid, 'alice');
  assert.deepEqual(await act(alice, { action: 'revokeCode' }), { ok: true });
  assert.deepEqual(await act(alice, { action: 'revokeCode' }), { ok: true });
  assert.equal((await friends.profile(alice)).friendCode, null);
  assert.equal((await data('profiles/alice')).friendCodeId, undefined);
  await rejects(act(bob, { action: 'redeem', code: rotated.code, requestId: rid() }), 404, 'NOT_FOUND');
  const again = await act(alice, { action: 'rotateCode' });
  assert.equal((await friends.profile(alice)).friendCode.code, again.code);
  assert.deepEqual((await db.collection('friendCodes').where('uid', '==', 'alice').get()).docs.filter(doc => !doc.data().revoked).map(doc => doc.id), [sha(again.code)]);
  // Rotating or revoking before any profile exists still works for a brand-new user.
  assert.match((await act(carol, { action: 'rotateCode' })).code, /^[A-Za-z0-9_-]{32}$/u);
  assert.deepEqual(await act(dave, { action: 'revokeCode' }), { ok: true });
});

test('lookup: handle, friend code and team member each return a minimal card with the caller\'s relation', options, async () => {
  await friends.profile(alice); await friends.profile(bob);
  await act(alice, { action: 'profile', handle: 'Alice_W' });
  const card = await act(bob, { action: 'lookup', handle: ' alice_w'.trim().toUpperCase() });
  assert.deepEqual(card, { uid: 'alice', displayName: 'Alice', handle: 'Alice_W', relation: 'none' });
  assert.deepEqual(await act(bob, { action: 'lookup', code: await link(alice) }), card);
  assert.deepEqual(Object.keys(await act(alice, { action: 'lookup', code: await link(bob) })).sort(), ['displayName', 'relation', 'uid']);
  await request(bob, alice);
  assert.equal((await act(alice, { action: 'lookup', code: await link(bob) })).relation, 'incoming');
  assert.equal((await act(bob, { action: 'lookup', handle: 'alice_w' })).relation, 'outgoing');
  await act(alice, { action: 'accept', otherUid: 'bob' });
  assert.equal((await act(bob, { action: 'lookup', handle: 'alice_w' })).relation, 'friends');
  // The team path needs two current members; no profile, handle or flag is required.
  await teams.execute(alice, { action: 'create', teamId: 'lookup-team', name: 'Lookup' });
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'lookup-team', requestId: 'invite01', role: 'member', maxUses: 5, ttlSeconds: 3600 });
  await teams.execute(carol, { action: 'join', code, requestId: 'join-carol' });
  assert.deepEqual(await act(alice, { action: 'lookup', teamId: 'lookup-team', uid: 'carol' }), { uid: 'carol', displayName: 'Carol', relation: 'none' });
  await act(carol, { action: 'profile', findByHandle: false, findByEmail: false, displayName: 'Carol C' });
  assert.deepEqual(await act(alice, { action: 'lookup', teamId: 'lookup-team', uid: 'carol' }), { uid: 'carol', displayName: 'Carol C', relation: 'none' });
  await rejects(act(alice, { action: 'lookup', teamId: 'lookup-team', uid: 'bob' }), 404, 'NOT_FOUND'); // Bob is not a member
  await rejects(act(bob, { action: 'lookup', teamId: 'lookup-team', uid: 'alice' }), 404, 'NOT_FOUND'); // neither is the caller
  await rejects(act(alice, { action: 'lookup', teamId: 'lookup-team', uid: 'nobody' }), 404, 'NOT_FOUND');
  await rejects(act(alice, { action: 'lookup', teamId: 'missing-team', uid: 'carol' }), 404, 'NOT_FOUND');
  await teams.execute(alice, { action: 'delete', teamId: 'lookup-team' });
  await rejects(act(alice, { action: 'lookup', teamId: 'lookup-team', uid: 'carol' }), 404, 'NOT_FOUND');
});

test('lookup: missing, hidden, unverified, self and blocked targets are one uniform NOT_FOUND', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  await act(alice, { action: 'profile', handle: 'alice_w' });
  await act(carol, { action: 'profile', handle: 'carol_c', findByHandle: false, findByEmail: false });
  accounts.set('alice@example.com', { uid: 'alice', emailVerified: true });
  accounts.set('carol@example.com', { uid: 'carol', emailVerified: true });
  accounts.set('unverified@example.com', { uid: 'bob', emailVerified: false });
  const found = await act(bob, { action: 'lookup', email: ' Alice@Example.COM ' });
  assert.deepEqual(found, { uid: 'alice', displayName: 'Alice', handle: 'alice_w', relation: 'none' });
  const code = await link(alice);
  const misses = [
    { action: 'lookup', handle: 'nobody_here' }, { action: 'lookup', handle: 'carol_c' }, // unknown / findByHandle off
    { action: 'lookup', email: 'ghost@example.com' }, { action: 'lookup', email: 'carol@example.com' }, // unknown / findByEmail off
    { action: 'lookup', email: 'unverified@example.com' }, { action: 'lookup', handle: 'dave_d' }, // unverified / a handle nobody holds
    { action: 'lookup', code: 'z'.repeat(32) }, { action: 'lookup', teamId: 'nothing01', uid: 'carol' },
  ];
  const errors = [];
  for (const input of misses) {
    await assert.rejects(act(bob, input), error => { errors.push(JSON.stringify([error.constructor.name, error.status, error.code, error.message])); return true; });
  }
  for (const input of [{ action: 'lookup', handle: 'alice_w' }, { action: 'lookup', email: 'alice@example.com' }, { action: 'lookup', code }]) {
    await assert.rejects(act(alice, input), error => { errors.push(JSON.stringify([error.constructor.name, error.status, error.code, error.message])); return true; }); // self
  }
  await act(carol, { action: 'block', otherUid: 'bob' });
  await act(carol, { action: 'profile', findByHandle: true });
  await assert.rejects(act(bob, { action: 'lookup', handle: 'carol_c' }), error => { errors.push(JSON.stringify([error.constructor.name, error.status, error.code, error.message])); return true; }); // blocked by Carol
  await act(carol, { action: 'unblock', otherUid: 'bob' });
  assert.equal((await act(bob, { action: 'lookup', handle: 'carol_c' })).uid, 'carol');
  await act(bob, { action: 'block', otherUid: 'carol' });
  await assert.rejects(act(carol, { action: 'lookup', code: await link(bob) }), error => { errors.push(JSON.stringify([error.constructor.name, error.status, error.code, error.message])); return true; }); // blocked by me
  await act(bob, { action: 'block', otherUid: 'alice' });
  await assert.rejects(act(alice, { action: 'lookup', code: await link(bob) }), error => { errors.push(JSON.stringify([error.constructor.name, error.status, error.code, error.message])); return true; }); // blocked the other way
  assert.equal(new Set(errors).size, 1, errors.join('\n'));
  assert.equal(errors[0], JSON.stringify(['FriendsError', 404, 'NOT_FOUND', 'NOT_FOUND']));
  // An Auth failure other than "no such user" is a real error, not a quiet miss.
  const outage = new FriendsStore(db, () => now, { auth: { getUserByEmail: async () => { throw Object.assign(new Error('unavailable'), { code: 'auth/internal-error' }); } } });
  await assert.rejects(outage.execute(bob, { action: 'lookup', email: 'alice@example.com' }), { code: 'auth/internal-error' });
});

test('lookup: 30 per 10 minutes per user in a sliding window; misses count, a refused call is not recorded, requests have their own bucket', options, async () => {
  await friends.profile(alice);
  for (let i = 0; i < LOOKUP_LIMIT; i++) {
    now += 1000;
    await assert.rejects(act(bob, { action: 'lookup', handle: `missing_${i}` }), { code: 'NOT_FOUND' });
  }
  await rejects(act(bob, { action: 'lookup', code: await link(alice) }), 429, 'RATE_LIMITED');
  await rejects(act(bob, { action: 'lookup', handle: 'missing_x' }), 429, 'RATE_LIMITED');
  assert.equal((await data(`friendLimits/${sha('lookup:bob')}`)).sentAt.length, LOOKUP_LIMIT);
  assert.equal((await act(carol, { action: 'lookup', code: await link(alice) })).uid, 'alice'); // limits are per user
  assert.equal((await request(bob, alice, { via: 'email' })).state, 'outgoing'); // requests do not spend the lookup budget
  await friends.prune(); // nothing has aged out yet
  assert.deepEqual(await ids('friendLimits'), [sha('lookup:bob'), sha('lookup:carol'), sha('request:bob')].sort());
  // The oldest timestamps age out one by one: after 10 minutes from the first lookup exactly one slot opens.
  now = (await data(`friendLimits/${sha('lookup:bob')}`)).sentAt[0] + LOOKUP_WINDOW_MS + 1;
  assert.equal((await act(bob, { action: 'lookup', code: await link(alice) })).uid, 'alice');
  await rejects(act(bob, { action: 'lookup', code: await link(alice) }), 429, 'RATE_LIMITED');
  now += LOOKUP_WINDOW_MS + 1;
  assert.equal((await act(bob, { action: 'lookup', code: await link(alice) })).uid, 'alice');
  assert.equal((await data(`friendLimits/${sha('lookup:bob')}`)).sentAt.length, 1);
  // Closed windows are pruned; the open one stays until it closes too.
  await friends.prune();
  assert.deepEqual(await ids('friendLimits'), [sha('lookup:bob')]);
  now += LOOKUP_WINDOW_MS + 1;
  await friends.prune();
  assert.deepEqual(await ids('friendLimits'), []);
});

test('request: records one pending friendship with both cards and one push; repeating it creates nothing', options, async () => {
  await friends.profile(alice); await friends.profile(bob);
  const first = rid(), ask = (extra = {}) => act(bob, { action: 'request', via: 'email', email: mail(alice), requestId: first, ...extra });
  assert.deepEqual(await ask({ note: '  Met at the meetup  ' }), { uid: 'alice', state: 'outgoing' });
  const outgoing = await data('users/bob/friends/alice'), incoming = await data('users/alice/friends/bob');
  assert.deepEqual([outgoing.state, outgoing.displayName, outgoing.note, incoming.state, incoming.displayName, incoming.note],
    ['outgoing', 'Alice', 'Met at the meetup', 'incoming', 'Bob', 'Met at the meetup']);
  assert.equal(outgoing.teamId, undefined);
  const f = await data(`friendships/${pairKey('alice', 'bob')}`);
  assert.deepEqual([f.state, f.requester, f.epoch, f.source, f.note, f.uids], ['pending', 'bob', 0, 'email', 'Met at the meetup', ['alice', 'bob']]);
  assert.deepEqual(await teamsIn(), []);
  assert.deepEqual(events.map(event => [event.kind, event.to, event.from, event.name]), [['request', 'alice', 'bob', 'Bob']]);
  // The same id again, and a new id for the same target, change nothing and push nothing more.
  assert.deepEqual(await ask({ note: 'Met at the meetup' }), { uid: 'alice', state: 'outgoing' });
  assert.deepEqual(await request(bob, alice, { note: 'again' }), { uid: 'alice', state: 'outgoing' });
  assert.equal(events.length, 1);
  assert.equal((await data('users/alice/friends/bob')).note, 'Met at the meetup');
  assert.equal((await db.collection('users/bob/friendRequests').get()).size, 2);
  // An id means one payload, proof included. A replay answers for the user the first call resolved, with no new discovery.
  await rejects(ask({ note: 'Different' }), 409, 'REQUEST_ID_CONFLICT');
  await rejects(ask({ email: mail(carol) }), 409, 'REQUEST_ID_CONFLICT');
  await rejects(ask({ via: 'handle', email: undefined, handle: 'alice_w' }), 409, 'REQUEST_ID_CONFLICT');
  await act(alice, { action: 'profile', findByEmail: false });
  assert.deepEqual(await ask({ note: 'Met at the meetup' }), { uid: 'alice', state: 'outgoing' });
  assert.equal(await consistent(alice, bob), 'pending');
});

test('request: the proof names the target — a handle, an email or a shared team — and is refused as NOT_FOUND exactly where lookup would show nothing', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  const asks = (via, proof) => act(bob, { action: 'request', via, requestId: rid(), ...proof });
  const reset = async () => { await act(alice, { action: 'block', otherUid: 'bob' }); await act(alice, { action: 'unblock', otherUid: 'bob' }); };
  await rejects(asks('handle', { handle: 'alice_w' }), 404, 'NOT_FOUND'); // no handle yet
  await act(alice, { action: 'profile', handle: 'Alice_W' });
  assert.deepEqual(await asks('handle', { handle: 'ALICE_W' }), { uid: 'alice', state: 'outgoing' }); // case-folded
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).source, 'handle');
  await reset();
  await act(alice, { action: 'profile', findByHandle: false });
  await rejects(asks('handle', { handle: 'alice_w' }), 404, 'NOT_FOUND');
  assert.deepEqual(await asks('email', { email: ' Alice@Example.COM ' }), { uid: 'alice', state: 'outgoing' });
  await reset();
  await act(alice, { action: 'profile', findByEmail: false });
  await rejects(asks('email', { email: mail(alice) }), 404, 'NOT_FOUND');
  await rejects(asks('email', { email: 'ghost@example.com' }), 404, 'NOT_FOUND'); // nobody owns it
  await rejects(asks('email', { email: mail(bob) }), 404, 'NOT_FOUND'); // yourself
  await rejects(asks('handle', { handle: 'nobody_here' }), 404, 'NOT_FOUND');
  accounts.set('unverified@example.com', { uid: 'carol', emailVerified: false });
  await rejects(asks('email', { email: 'unverified@example.com' }), 404, 'NOT_FOUND'); // an address anyone could have registered
  // A team proof needs the named team to be active with both as current members; hidden flags do not matter there.
  await teams.execute(bob, { action: 'create', teamId: 'ask-team01', name: 'Ask' });
  const { code } = await teams.execute(bob, { action: 'invite', teamId: 'ask-team01', requestId: 'invite01', role: 'member', maxUses: 5, ttlSeconds: 3600 });
  await teams.execute(alice, { action: 'join', code, requestId: 'join-alice' });
  await teams.execute(dave, { action: 'join', code, requestId: 'join-dave' });
  await rejects(asks('team', { teamId: 'ask-team01', uid: 'carol' }), 404, 'NOT_FOUND'); // not a member
  await rejects(asks('team', { teamId: 'ask-team01', uid: 'nobody' }), 404, 'NOT_FOUND');
  await rejects(asks('team', { teamId: 'ask-team01', uid: 'bob' }), 404, 'NOT_FOUND'); // yourself
  await rejects(asks('team', { teamId: 'missing-team', uid: 'alice' }), 404, 'NOT_FOUND');
  assert.deepEqual(await asks('team', { teamId: 'ask-team01', uid: 'alice' }), { uid: 'alice', state: 'outgoing' });
  assert.deepEqual(await asks('team', { teamId: 'ask-team01', uid: 'dave' }), { uid: 'dave', state: 'outgoing' }); // no profile: the card name comes from the team
  assert.equal((await data('users/bob/friends/dave')).displayName, 'Member');
  assert.equal((await data('users/dave/friends/bob')).displayName, 'Bob');
  assert.equal((await data(`friendships/${pairKey('bob', 'dave')}`)).source, 'team');
  await teams.execute(bob, { action: 'delete', teamId: 'ask-team01' });
  await act(alice, { action: 'cancel', otherUid: 'bob' }); await act(bob, { action: 'cancel', otherUid: 'alice' });
  await rejects(asks('team', { teamId: 'ask-team01', uid: 'alice' }), 404, 'NOT_FOUND'); // the shared team is gone
  // Whatever was refused left nothing behind: no card, no friendship, no receipt for the refused ids.
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal(await consistent(bob, carol), 'none');
  assert.equal((await db.collection('users/bob/friendRequests').get()).size, 4);
});

test('request: a uid is not a proof — requests without a handle, an email or a shared team are rejected before anything is read or spent', options, async () => {
  for (const user of [alice, bob]) await friends.profile(user);
  const bare = [
    { targetUid: 'alice', via: 'email' }, { targetUid: 'alice', via: 'handle' }, { targetUid: 'alice', via: 'team' },
    { uid: 'alice', via: 'email' }, { uid: 'alice', via: 'handle' }, { uid: 'alice', via: 'team' }, { uid: 'alice', via: 'team', teamId: undefined },
    { via: 'email' }, { via: 'handle' }, { via: 'team' }, { via: 'team', teamId: 'team-0001' },
    { via: 'email', email: mail(alice), uid: 'alice' }, { via: 'handle', handle: 'alice_w', uid: 'alice' }, { via: 'email', email: mail(alice), handle: 'alice_w' },
    { via: 'email', email: mail(alice), targetUid: 'alice' }, { via: 'team', teamId: 'team-0001', uid: 'alice', email: mail(alice) },
  ];
  for (const body of bare) await assert.rejects(act(bob, { action: 'request', requestId: rid(), ...body }), z.ZodError, JSON.stringify(body));
  // Over HTTP it is the ordinary 400, and nothing was recorded, not even the rate-limit window.
  const handler = createTeamsHandler({ store: teams, friends, createCustomToken: async () => '', verifyToken: async () => bob });
  const response = await handler(new Request('https://test.invalid/v1/friends', { method: 'POST', headers: { Authorization: 'Bearer token' },
    body: JSON.stringify({ action: 'request', targetUid: 'alice', requestId: rid(), via: 'email' }) }));
  assert.deepEqual([response.status, await response.json()], [400, { error: { code: 'INVALID_INPUT' } }]);
  assert.equal(await consistent(alice, bob), 'none');
  assert.deepEqual(await ids('friendLimits'), []);
  assert.deepEqual(await ids('users/bob/friendRequests'), []);
  assert.deepEqual(await ids('users/alice/friends'), []);
  assert.deepEqual(events, []);
});

test('request: knowing someone\'s uid is not enough — the wrong handle or email for it is NOT_FOUND, or reaches only whoever really owns it', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  await act(alice, { action: 'profile', handle: 'alice_w' }); await act(carol, { action: 'profile', handle: 'carol_c' });
  // Bob learned Alice's uid through a team, then left it; the uid alone gets him nowhere.
  await teams.execute(alice, { action: 'create', teamId: 'former-team', name: 'Former' });
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'former-team', requestId: 'invite01', role: 'member', maxUses: 5, ttlSeconds: 3600 });
  await teams.execute(bob, { action: 'join', code, requestId: 'join-bob' });
  assert.equal((await teams.detail('bob', 'former-team')).members.some(member => member.uid === 'alice'), true);
  await teams.execute(bob, { action: 'leave', teamId: 'former-team', generation: 1 });
  const asks = (via, proof) => act(bob, { action: 'request', via, requestId: rid(), ...proof });
  await rejects(asks('team', { teamId: 'former-team', uid: 'alice' }), 404, 'NOT_FOUND'); // no longer a shared team
  await rejects(asks('handle', { handle: 'alice_x' }), 404, 'NOT_FOUND'); // one letter off
  await rejects(asks('handle', { handle: 'alice' }), 404, 'NOT_FOUND');
  await rejects(asks('email', { email: 'alice@example.org' }), 404, 'NOT_FOUND');
  await rejects(asks('email', { email: 'alice.w@example.com' }), 404, 'NOT_FOUND');
  assert.equal(await consistent(alice, bob), 'none');
  assert.deepEqual(await ids('users/alice/friends'), []);
  // Someone else's handle or email reaches that someone else, and only them: nothing is created for Alice.
  assert.deepEqual(await asks('handle', { handle: 'carol_c' }), { uid: 'carol', state: 'outgoing' });
  assert.deepEqual(await asks('email', { email: mail(carol) }), { uid: 'carol', state: 'outgoing' });
  assert.equal(await consistent(alice, bob), 'none');
  assert.equal(await consistent(bob, carol), 'pending');
  assert.deepEqual(await ids('users/alice/friends'), []);
  assert.equal((await db.doc('users/bob/friends/alice').get()).exists, false);
  // The right proof still works.
  assert.deepEqual(await asks('handle', { handle: 'alice_w' }), { uid: 'alice', state: 'outgoing' });
});

test('request: a pending request from the target still becomes an accept, but only through a proof that resolves to them', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  await act(alice, { action: 'profile', handle: 'alice_w' });
  await request(alice, bob); // Alice asked Bob
  const asks = (via, proof) => act(bob, { action: 'request', via, requestId: rid(), ...proof });
  // A proof that resolves to nobody, or to somebody else, never completes it.
  await rejects(asks('handle', { handle: 'alice_x' }), 404, 'NOT_FOUND');
  await rejects(asks('email', { email: 'ghost@example.com' }), 404, 'NOT_FOUND');
  await rejects(asks('team', { teamId: 'missing-team', uid: 'alice' }), 404, 'NOT_FOUND');
  assert.equal(await consistent(alice, bob), 'pending');
  assert.equal((await asks('email', { email: mail(carol) })).uid, 'carol');
  assert.equal(await consistent(alice, bob), 'pending');
  // Even for a request that is Bob's accept, the target must be findable by the way he names her: she hid her handle.
  await act(alice, { action: 'profile', findByHandle: false });
  await rejects(asks('handle', { handle: 'alice_w' }), 404, 'NOT_FOUND');
  assert.equal(await consistent(alice, bob), 'pending');
  const done = await asks('email', { email: mail(alice) });
  assert.deepEqual(done, { uid: 'alice', state: 'friends', teamId: pairTeamId('alice', 'bob', 1) });
  assert.equal(await consistent(alice, bob), 'friends');
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).requester, 'alice');
  assert.deepEqual(events.map(event => [event.kind, event.to]).slice(-1), [['accepted', 'alice']]);
});

test('email discovery: a verified account that never opened Friends is found, named from Auth, and can be asked; handle and code still need a profile', options, async () => {
  await friends.profile(bob);
  const erin = { uid: 'erin', name: 'Erin' }, frank = { uid: 'frank' };
  accounts.set('erin@example.com', { uid: 'erin', emailVerified: true, displayName: '  Erin from Auth  ' });
  accounts.set('frank@example.com', { uid: 'frank', emailVerified: true });
  accounts.set('grace@example.com', { uid: 'grace', emailVerified: false, displayName: 'Grace' });
  accounts.set('long@example.com', { uid: 'longname', emailVerified: true, displayName: 'n'.repeat(120) });
  assert.equal((await db.doc('profiles/erin').get()).exists, false);
  // Lookup: the Auth display name stands in; no handle exists to show; the flags have never been switched off.
  assert.deepEqual(await act(bob, { action: 'lookup', email: 'ERIN@example.com' }), { uid: 'erin', displayName: 'Erin from Auth', relation: 'none' });
  assert.deepEqual(await act(bob, { action: 'lookup', email: 'frank@example.com' }), { uid: 'frank', displayName: 'Member', relation: 'none' });
  assert.equal((await act(bob, { action: 'lookup', email: 'long@example.com' })).displayName, 'n'.repeat(80));
  await rejects(act(bob, { action: 'lookup', email: 'grace@example.com' }), 404, 'NOT_FOUND'); // unverified
  // Request: the stored card carries the same name, and nobody's profile is created by being asked.
  assert.deepEqual(await request(bob, erin), { uid: 'erin', state: 'outgoing' });
  assert.equal((await data('users/bob/friends/erin')).displayName, 'Erin from Auth');
  assert.equal((await data('users/erin/friends/bob')).displayName, 'Bob');
  assert.deepEqual(await request(bob, frank), { uid: 'frank', state: 'outgoing' });
  assert.equal((await data('users/bob/friends/frank')).displayName, 'Member');
  assert.equal((await db.doc('profiles/erin').get()).exists, false);
  assert.equal((await db.doc('profiles/frank').get()).exists, false);
  assert.deepEqual(events.map(event => [event.kind, event.to]), [['request', 'erin'], ['request', 'frank']]);
  // Erin accepts without ever opening Friends; her own token name now names her on Bob's card.
  assert.equal((await act(erin, { action: 'accept', otherUid: 'bob' })).state, 'friends');
  assert.equal(await consistent(erin, bob), 'friends');
  assert.equal((await data('users/bob/friends/erin')).displayName, 'Erin');
  // The handle and code routes still require a profile: orphan locks and codes for an account without one find nobody.
  await db.doc('handles/orphan_h').set({ uid: 'frank' });
  await db.doc(`friendCodes/${sha('f'.repeat(32))}`).set({ uid: 'frank', code: 'f'.repeat(32), createdAt: new Date(now), revoked: false });
  await rejects(act(bob, { action: 'lookup', handle: 'orphan_h' }), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'lookup', code: 'f'.repeat(32) }), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'request', via: 'handle', handle: 'orphan_h', requestId: rid() }), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'redeem', code: 'f'.repeat(32), requestId: rid() }), 404, 'NOT_FOUND');
  // Opening Friends makes the flags real: turned off, the account is hidden again.
  await friends.profile(frank);
  assert.equal((await act(bob, { action: 'lookup', email: 'frank@example.com' })).displayName, 'Member');
  await act(bob, { action: 'cancel', otherUid: 'frank' });
  await act(frank, { action: 'profile', findByEmail: false });
  await rejects(act(bob, { action: 'lookup', email: 'frank@example.com' }), 404, 'NOT_FOUND');
  await rejects(request(bob, frank), 404, 'NOT_FOUND');
  // An account whose uid cannot be a friend (the hashing separator) is not found rather than half-handled.
  accounts.set('odd@example.com', { uid: 'od|d', emailVerified: true });
  await rejects(act(bob, { action: 'lookup', email: 'odd@example.com' }), 404, 'NOT_FOUND');
});

test('request: at most 50 outgoing requests, 500 friends; a mutual request and cancelling still fit', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  const seed = db.batch();
  for (let i = 0; i < OUTGOING_LIMIT; i++) seed.set(db.doc(`users/bob/friends/seed${String(i).padStart(3, '0')}`), { state: 'outgoing', displayName: 'Seed' });
  await seed.commit();
  await rejects(request(bob, alice), 409, 'REQUEST_LIMIT');
  assert.equal(await consistent(alice, bob), 'none');
  await request(alice, bob); // an incoming request is not outgoing: Bob's own request completes it
  assert.equal((await request(bob, alice)).state, 'friends');
  await db.doc('users/bob/friends/seed000').delete();
  assert.equal((await request(bob, carol)).state, 'outgoing');
  await rejects(act(bob, { action: 'request', via: 'handle', handle: 'nobody_here', requestId: rid() }), 404, 'NOT_FOUND'); // the limit never hides the uniform miss either
  await rejects(request(bob, dave), 409, 'REQUEST_LIMIT'); // dave never opened Friends; an email still finds him
  await act(bob, { action: 'cancel', otherUid: 'carol' });
  assert.equal((await request(bob, dave)).state, 'outgoing');
  // Friend limit: 500 on either side stops accepting, redeeming and a mutual request.
  const crowd = db.batch();
  for (let i = 0; i < FRIEND_LIMIT; i++) crowd.set(db.doc(`users/carol/friends/mate${String(i).padStart(3, '0')}`), { state: 'friends', displayName: 'Mate' });
  await crowd.commit();
  await act(dave, { action: 'accept', otherUid: 'bob' });
  assert.equal(await consistent(bob, dave), 'friends');
  await friends.profile(carol); await request(alice, carol); await request(dave, carol);
  await rejects(act(carol, { action: 'accept', otherUid: 'alice' }), 409, 'FRIEND_LIMIT');
  await rejects(act(alice, { action: 'redeem', code: await link(carol), requestId: rid() }), 409, 'FRIEND_LIMIT');
  await rejects(act(carol, { action: 'redeem', code: await link(alice), requestId: rid() }), 409, 'FRIEND_LIMIT');
  assert.equal(await consistent(alice, carol), 'pending'); // nothing changed
  await db.doc('users/carol/friends/mate000').delete();
  assert.equal((await act(carol, { action: 'accept', otherUid: 'alice' })).state, 'friends');
  await rejects(act(carol, { action: 'accept', otherUid: 'dave' }), 409, 'FRIEND_LIMIT');
});

test('accept: creates the pair team, its single dm, both cards and the friendship in one step — and is safe to repeat', options, async () => {
  await friends.profile(alice); await friends.profile(bob);
  await request(bob, alice, { note: 'hello' });
  now += 1000;
  const accepted = await act(alice, { action: 'accept', otherUid: 'bob' });
  const teamId = `f_${sha('alice|bob|1').slice(0, 40)}`;
  assert.deepEqual(accepted, { state: 'friends', teamId });
  const team = await data(`teams/${teamId}`);
  assert.deepEqual([team.kind, team.state, team.memberCount, team.createdBy, team.name], ['pair', 'active', 2, 'alice', 'Friends']);
  for (const user of [alice, bob]) {
    const member = await data(`teams/${teamId}/members/${user.uid}`);
    assert.deepEqual([member.displayName, member.role, member.generation], [user.name, 'member', 1]);
    assert.equal(member.joinedAt.toMillis(), now);
    assert.deepEqual(await data(`teams/${teamId}/membershipEpochs/${user.uid}`), { generation: 1 });
  }
  assert.deepEqual(await ids(`teams/${teamId}/conversations`), [PAIR_CONVERSATION_ID]);
  const conversation = await data(`teams/${teamId}/conversations/${PAIR_CONVERSATION_ID}`);
  assert.deepEqual([conversation.kind, conversation.participants, conversation.memberUids, conversation.createdBy, conversation.lastSeq, conversation.lastMessage],
    ['dm', ['user:alice', 'user:bob'], ['alice', 'bob'], 'alice', 0, null]);
  // The record is byte-for-byte what the trusted conversation constructor writes in an ordinary team.
  await teams.execute(alice, { action: 'create', teamId: 'plain-team', name: 'Plain' });
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'plain-team', requestId: 'invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(bob, { action: 'join', code, requestId: 'join-bob' });
  const plain = await createTeamConversation(db, { teamId: 'plain-team', conversationId: 'plain-chat', callerUid: 'alice', callerGeneration: 1, kind: 'dm', title: 'Friends', participants: ['user:alice', 'user:bob'] });
  assert.deepEqual(conversation, plain);
  for (const [owner, peer, name] of [['alice', 'bob', 'Bob'], ['bob', 'alice', 'Alice']]) {
    const card = await data(`users/${owner}/friends/${peer}`);
    assert.deepEqual(Object.keys(card).sort(), ['displayName', 'generation', 'since', 'state', 'teamId', 'updatedAt']);
    assert.deepEqual([card.state, card.displayName, card.teamId, card.generation, card.since.toMillis()], ['friends', name, teamId, 1, now]);
  }
  const f = await data(`friendships/${pairKey('alice', 'bob')}`);
  assert.deepEqual([f.state, f.epoch, f.teamId, f.requester, f.source, f.note], ['friends', 1, teamId, 'bob', 'email', undefined]);
  assert.deepEqual(events.map(event => event.kind), ['request', 'accepted']);
  assert.deepEqual([events[1].to, events[1].from, events[1].name], ['bob', 'alice', 'Alice']);
  // The pair team is invisible to the team list and its index, and does not count toward the limit.
  for (const user of ['alice', 'bob']) assert.deepEqual(await ids(`users/${user}/teams`), ['plain-team']);
  assert.deepEqual((await teams.list('alice')).teams.map(item => item.id), ['plain-team']);
  const detail = await teams.detail('bob', teamId);
  assert.deepEqual([detail.team.kind, detail.team.name, detail.team.memberCount, detail.team.role, detail.team.generation, detail.invites], ['pair', 'Friends', 2, 'member', 1, []]);
  assert.deepEqual(detail.members.map(member => [member.uid, member.displayName, member.role, member.generation]).sort(), [['alice', 'Alice', 'member', 1], ['bob', 'Bob', 'member', 1]]);
  await rejects(teams.detail('carol', teamId), 403, 'MEMBERSHIP_REQUIRED');
  // Repeats change nothing.
  assert.deepEqual(await act(alice, { action: 'accept', otherUid: 'bob' }), accepted);
  assert.deepEqual(await act(bob, { action: 'accept', otherUid: 'alice' }), accepted); // already friends: the other side's late accept
  assert.equal((await request(alice, bob)).teamId, teamId);
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).epoch, 1);
  assert.equal(events.length, 2);
  assert.deepEqual(await teamsIn(), [teamId]);
  assert.equal(await consistent(alice, bob), 'friends');
});

test('accept: only the receiver of a live request can accept it', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  await rejects(act(alice, { action: 'accept', otherUid: 'bob' }), 404, 'REQUEST_UNAVAILABLE'); // nothing pending
  await request(bob, alice);
  await rejects(act(bob, { action: 'accept', otherUid: 'alice' }), 404, 'REQUEST_UNAVAILABLE'); // the requester cannot accept their own
  await rejects(act(carol, { action: 'accept', otherUid: 'bob' }), 404, 'REQUEST_UNAVAILABLE'); // a third party
  await rejects(act(alice, { action: 'accept', otherUid: 'alice' }), 400, 'INVALID_INPUT');
  assert.equal(await consistent(alice, bob), 'pending');
  assert.deepEqual(await teamsIn(), []);
});

test('decline and cancel end a pending request on both sides; repeats and the wrong direction are harmless; asking again is allowed', options, async () => {
  for (const user of [alice, bob]) await friends.profile(user);
  const first = rid(), ask = () => act(bob, { action: 'request', via: 'email', email: mail(alice), requestId: first });
  await ask();
  await act(bob, { action: 'decline', otherUid: 'alice' }); // wrong direction: Bob asked
  await act(alice, { action: 'cancel', otherUid: 'bob' }); // wrong direction: Alice received
  assert.equal(await consistent(alice, bob), 'pending');
  assert.deepEqual(await act(alice, { action: 'decline', otherUid: 'bob' }), { ok: true });
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).note, undefined);
  assert.deepEqual(await act(alice, { action: 'decline', otherUid: 'bob' }), { ok: true });
  assert.deepEqual(await act(bob, { action: 'cancel', otherUid: 'alice' }), { ok: true });
  await rejects(act(alice, { action: 'accept', otherUid: 'bob' }), 404, 'REQUEST_UNAVAILABLE');
  await rejects(ask(), 409, 'STALE_REQUEST'); // an old id creates nothing
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal((await request(bob, alice)).state, 'outgoing'); // a new id may ask again
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).state, 'pending');
  assert.deepEqual(await act(bob, { action: 'cancel', otherUid: 'alice' }), { ok: true });
  assert.equal(await consistent(alice, bob), 'ended');
  await rejects(act(alice, { action: 'accept', otherUid: 'bob' }), 404, 'REQUEST_UNAVAILABLE');
  await rejects(act(alice, { action: 'decline', otherUid: 'alice' }), 400, 'INVALID_INPUT');
  assert.deepEqual(await teamsIn(), []);
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).epoch, 0, 'no friendship ever existed');
});

test('mutual requests become one friendship — sequentially and when sent at the same moment', options, async () => {
  for (const user of [alice, bob, carol, dave]) await friends.profile(user);
  assert.equal((await request(bob, alice)).state, 'outgoing');
  const joined = await request(alice, bob);
  assert.equal(joined.state, 'friends');
  assert.equal(joined.teamId, pairTeamId('alice', 'bob', 1));
  assert.deepEqual(events.map(event => [event.kind, event.to]), [['request', 'alice'], ['accepted', 'bob']]);
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).requester, 'bob');
  assert.equal(await consistent(alice, bob), 'friends');
  for (const [round, [x, y]] of [[carol, dave], [dave, carol]].entries()) {
    const results = await Promise.all([request(x, y), request(y, x)]);
    // Whoever commits second completes the friendship; the one that committed first has only asked.
    assert.ok(results.every(result => ['friends', 'outgoing'].includes(result.state)) && results.some(result => result.state === 'friends'), JSON.stringify(results));
    assert.equal(await consistent(carol, dave), 'friends');
    assert.equal((await data(`friendships/${pairKey('carol', 'dave')}`)).epoch, round + 1);
    assert.deepEqual(await activePairTeams(), [pairTeamId('alice', 'bob', 1), pairTeamId('carol', 'dave', round + 1)].sort());
    await act(carol, { action: 'remove', otherUid: 'dave' });
    assert.equal(await consistent(carol, dave), 'ended');
  }
});

test('races: accept against cancel, a repeated accept, block and decline always leave cards, friendship and team consistent', options, async () => {
  for (const user of [alice, bob, carol, dave]) await friends.profile(user);
  const accept = (to, from) => act(to, { action: 'accept', otherUid: from.uid });
  // The emulator serializes contending transactions slowly, so each kind of race runs once.
  const races = [
    { from: bob, to: alice, allowed: ['friends', 'ended'], run: (from, to) => [accept(to, from), act(from, { action: 'cancel', otherUid: to.uid })] },
    { from: carol, to: alice, allowed: ['friends'], run: (from, to) => [accept(to, from), accept(to, from), accept(to, from)],
      check: settled => {
        assert.ok(settled.every(result => result.status === 'fulfilled'), 'a repeated accept is answered, not refused');
        assert.equal(new Set(settled.map(result => result.value.teamId)).size, 1);
      } },
    { from: dave, to: alice, allowed: ['ended'], run: (from, to) => [accept(to, from), act(from, { action: 'block', otherUid: to.uid })] },
    { from: bob, to: carol, allowed: ['friends', 'ended'], run: (from, to) => [accept(to, from), act(to, { action: 'decline', otherUid: from.uid })] },
  ];
  for (const { from, to, allowed, run, check } of races) {
    await request(from, to);
    const settled = await Promise.allSettled(run(from, to));
    for (const result of settled) if (result.status === 'rejected') assert.ok([404, 409].includes(result.reason.status), result.reason.code);
    assert.ok(allowed.includes(await consistent(from, to)), `${from.uid} -> ${to.uid}`);
    check?.(settled);
  }
  assert.equal((await data(`friendships/${pairKey('alice', 'carol')}`)).epoch, 1, 'one request is one friendship, however often it is accepted');
  assert.deepEqual(await ids('users/dave/blocks'), ['alice']); // the block belongs to whoever pressed it
  assert.deepEqual(await ids('users/alice/blocks'), []);
  // Whatever the interleaving, the active pair teams are exactly the teams of the live friendships.
  assert.deepEqual(await activePairTeams(), (await db.collection('friendships').where('state', '==', 'friends').get()).docs.map(doc => doc.data().teamId).sort());
});

test('redeem: a friend link is instant, owner-consented, replay-safe, and replaces any pending request', options, async () => {
  await friends.profile(alice); await friends.profile(bob); await friends.profile(carol);
  const owner = await link(alice), first = rid();
  const result = await act(bob, { action: 'redeem', code: owner, requestId: first });
  assert.deepEqual(result, { state: 'friends', uid: 'alice', teamId: pairTeamId('alice', 'bob', 1) });
  const f = await data(`friendships/${pairKey('alice', 'bob')}`);
  assert.deepEqual([f.state, f.requester, f.source, f.epoch], ['friends', 'bob', 'code', 1]);
  assert.equal(await consistent(alice, bob), 'friends');
  assert.deepEqual(events.map(event => [event.kind, event.to, event.from]), [['linked', 'alice', 'bob']]);
  // Replays: the same id returns the same answer and nothing else happens; another payload under that id conflicts.
  assert.deepEqual(await act(bob, { action: 'redeem', code: owner, requestId: first }), result);
  assert.deepEqual(await act(bob, { action: 'redeem', code: owner, requestId: rid() }), result); // already friends
  await rejects(act(bob, { action: 'redeem', code: await link(carol), requestId: first }), 409, 'REQUEST_ID_CONFLICT');
  assert.equal(events.length, 1);
  assert.equal(await consistent(alice, bob), 'friends');
  // Nothing to redeem: unknown, own, malformed.
  await rejects(act(bob, { action: 'redeem', code: 'z'.repeat(32), requestId: rid() }), 404, 'NOT_FOUND');
  await rejects(act(carol, { action: 'redeem', code: await link(carol), requestId: rid() }), 404, 'NOT_FOUND');
  await assert.rejects(act(bob, { action: 'redeem', code: 'short', requestId: rid() }), z.ZodError);
  // The link replaces a pending request in either direction, keeping who asked first.
  await request(bob, carol);
  assert.equal(await consistent(bob, carol), 'pending');
  assert.equal((await act(bob, { action: 'redeem', code: await link(carol), requestId: rid() })).state, 'friends');
  assert.equal(await consistent(bob, carol), 'friends');
  const pairing = await data(`friendships/${pairKey('bob', 'carol')}`);
  assert.deepEqual([pairing.requester, pairing.source, pairing.epoch], ['bob', 'code', 1]);
  await act(carol, { action: 'remove', otherUid: 'bob' });
  await request(carol, bob);
  assert.equal((await act(bob, { action: 'redeem', code: await link(carol), requestId: rid() })).teamId, pairTeamId('bob', 'carol', 2));
  assert.equal(await consistent(bob, carol), 'friends');
  assert.equal((await data(`friendships/${pairKey('bob', 'carol')}`)).requester, 'carol');
  // Ending the friendship makes an old redeem id stale; the link itself still works with a new id and starts a new epoch.
  await act(alice, { action: 'remove', otherUid: 'bob' });
  await rejects(act(bob, { action: 'redeem', code: owner, requestId: first }), 409, 'STALE_REQUEST');
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal((await act(bob, { action: 'redeem', code: owner, requestId: rid() })).teamId, pairTeamId('alice', 'bob', 2));
  assert.equal(await consistent(alice, bob), 'friends');
});

test('redeem: a repeated redeem, a second visitor and the owner\'s own request racing on one link end in one consistent friendship each', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  const owner = await link(alice), same = rid();
  const [first, again, other, asked] = await Promise.all([act(bob, { action: 'redeem', code: owner, requestId: same }), act(bob, { action: 'redeem', code: owner, requestId: same }),
    act(carol, { action: 'redeem', code: owner, requestId: rid() }), request(alice, bob)]);
  assert.deepEqual([first, again], [first, first]); // the same id answers alike
  assert.ok([first, other].every(result => result.state === 'friends'));
  // The owner's request lands before the redeem (then it only asked and the redeem completed it) or after it (already friends).
  assert.ok(['outgoing', 'friends'].includes(asked.state), asked.state);
  assert.deepEqual(await teamsIn(), [pairTeamId('alice', 'bob', 1), pairTeamId('alice', 'carol', 1)].sort());
  assert.equal(await consistent(alice, bob), 'friends'); assert.equal(await consistent(alice, carol), 'friends');
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).epoch, 1);
  assert.equal(events.filter(event => event.kind === 'linked').length, 2, 'one announcement per new friend, none for the replay');
});

test('remove: the pair team is deleted with the friendship, access to its chat ends for both, and a re-friend starts a fresh team and chat', options, async () => {
  await befriend(bob, alice);
  const old = pairTeamId('alice', 'bob', 1), scope = { teamId: old, generation: 1, conversationId: PAIR_CONVERSATION_ID };
  const send = (actor, id, text) => messages.execute(actor, { action: 'mutate', ...scope, messageId: id, changeId: `c-${id}`, operation: 'create', text });
  await send(alice, 'message01', 'hello Bob'); await send(bob, 'message02', 'hello Alice');
  const asBob = environment.authenticatedContext('bob').firestore();
  const path = `teams/${old}/conversations/${PAIR_CONVERSATION_ID}`;
  await assertSucceeds(getDoc(doc(asBob, `${path}/messages/message01`)));
  await assertSucceeds(getDocs(collection(asBob, `${path}/messages`)));
  now += 1000;
  assert.deepEqual(await act(alice, { action: 'remove', otherUid: 'bob' }), { ok: true });
  const tombstone = await data(`teams/${old}`);
  assert.deepEqual([tombstone.state, tombstone.memberCount, tombstone.kind, tombstone.deletedAt.toMillis()], ['deleted', 0, 'pair', now]);
  assert.deepEqual(await ids(`teams/${old}/members`), []);
  assert.deepEqual(await data(`teams/${old}/membershipEpochs/alice`), { generation: 1 });
  assert.deepEqual(await ids(`teams/${old}/conversations/${PAIR_CONVERSATION_ID}/messages`), ['message01', 'message02']); // retained, unreachable
  const f = await data(`friendships/${pairKey('alice', 'bob')}`);
  assert.deepEqual([f.state, f.epoch, f.teamId], ['ended', 1, undefined]);
  assert.equal(await consistent(alice, bob), 'ended');
  for (const user of ['alice', 'bob']) {
    const client = environment.authenticatedContext(user).firestore();
    await assertFails(getDoc(doc(client, `teams/${old}`)));
    await assertFails(getDocs(collection(client, `teams/${old}/members`)));
    await assertFails(getDoc(doc(client, path)));
    await assertFails(getDoc(doc(client, `${path}/messages/message01`)));
    await assertFails(getDocs(collection(client, `${path}/messages`)));
    await assertFails(getDocs(collection(client, `${path}/changes`)));
    await assertFails(getDoc(doc(client, `${path}/reads/${user}`)));
  }
  await rejects(send(bob, 'message03', 'still there?'), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(send(alice, 'message03', 'still there?'), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(messages.execute(alice, { action: 'list', teamId: old, generation: 1 }), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(teams.detail('bob', old), 403, 'MEMBERSHIP_REQUIRED');
  // Repeats and non-friends are no-ops.
  assert.deepEqual(await act(alice, { action: 'remove', otherUid: 'bob' }), { ok: true });
  assert.deepEqual(await act(bob, { action: 'remove', otherUid: 'alice' }), { ok: true });
  assert.deepEqual(await act(bob, { action: 'remove', otherUid: 'carol' }), { ok: true });
  await rejects(act(bob, { action: 'remove', otherUid: 'bob' }), 400, 'INVALID_INPUT');
  assert.equal((await data(`teams/${old}`)).deletedAt.toMillis(), now, 'a repeat does not touch the tombstone');
  // A new friendship is a new team with an empty chat; the old tombstone is never reused.
  await befriend(alice, bob);
  const fresh = pairTeamId('alice', 'bob', 2);
  assert.notEqual(fresh, old);
  assert.equal((await data(`friendships/${pairKey('alice', 'bob')}`)).epoch, 2);
  assert.equal(await consistent(alice, bob), 'friends');
  assert.equal((await data(`teams/${old}`)).state, 'deleted');
  assert.equal((await data(`teams/${fresh}/conversations/${PAIR_CONVERSATION_ID}`)).lastSeq, 0);
  assert.deepEqual(await ids(`teams/${fresh}/conversations/${PAIR_CONVERSATION_ID}/messages`), []);
  const client = environment.authenticatedContext('alice').firestore();
  await assertSucceeds(getDoc(doc(client, `teams/${fresh}`)));
  await assertFails(getDoc(doc(client, `${path}/messages/message01`)));
  assert.deepEqual((await teamsIn()).sort(), [old, fresh].sort());
});

test('block: ends the friendship or request both ways, hides each side from the other, and unblock only lifts the block', options, async () => {
  for (const user of [alice, bob, carol, dave]) await friends.profile(user);
  await befriend(bob, alice);
  now += 1000;
  assert.deepEqual(await act(alice, { action: 'block', otherUid: 'bob' }), { ok: true });
  assert.deepEqual(Object.keys(await data('users/alice/blocks/bob')).sort(), ['at', 'displayName']);
  assert.equal((await data('users/alice/blocks/bob')).at.toMillis(), now);
  assert.equal((await data('users/alice/blocks/bob')).displayName, 'Bob');
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal((await data(pairTeamId('alice', 'bob', 1) && `teams/${pairTeamId('alice', 'bob', 1)}`)).state, 'deleted');
  assert.equal((await db.doc('users/bob/blocks/alice').get()).exists, false);
  // The blocked side cannot find, ask, redeem or reach the blocker, and learns nothing that differs from "no such user".
  await rejects(act(bob, { action: 'lookup', code: await link(alice) }), 404, 'NOT_FOUND');
  await rejects(request(bob, alice), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'redeem', code: await link(alice), requestId: rid() }), 404, 'NOT_FOUND');
  await rejects(act(bob, { action: 'accept', otherUid: 'alice' }), 404, 'REQUEST_UNAVAILABLE');
  // The blocker cannot either, until they unblock.
  await rejects(request(alice, bob), 404, 'NOT_FOUND');
  await rejects(act(alice, { action: 'redeem', code: await link(bob), requestId: rid() }), 404, 'NOT_FOUND');
  assert.equal(await consistent(alice, bob), 'ended');
  // Blocking again changes nothing, including the timestamp.
  now += 5000;
  assert.deepEqual(await act(alice, { action: 'block', otherUid: 'bob' }), { ok: true });
  assert.equal((await data('users/alice/blocks/bob')).at.toMillis(), now - 5000);
  // Unblock removes only the block: no friendship comes back, and either side may ask afresh.
  assert.deepEqual(await act(alice, { action: 'unblock', otherUid: 'bob' }), { ok: true });
  assert.deepEqual(await act(alice, { action: 'unblock', otherUid: 'bob' }), { ok: true });
  assert.deepEqual(await act(alice, { action: 'unblock', otherUid: 'carol' }), { ok: true });
  assert.equal((await db.doc('users/alice/blocks/bob').get()).exists, false);
  assert.equal(await consistent(alice, bob), 'ended');
  assert.equal((await request(bob, alice)).state, 'outgoing');
  // A pending request dies with a block from either end, and a stranger can be blocked beforehand.
  await act(alice, { action: 'block', otherUid: 'bob' });
  assert.equal(await consistent(alice, bob), 'ended');
  await act(alice, { action: 'unblock', otherUid: 'bob' });
  await request(alice, bob);
  await act(alice, { action: 'block', otherUid: 'bob' });
  assert.equal(await consistent(alice, bob), 'ended');
  await act(carol, { action: 'block', otherUid: 'dave' });
  await rejects(request(dave, carol), 404, 'NOT_FOUND');
  assert.equal(await consistent(carol, dave), 'none');
  await rejects(act(carol, { action: 'block', otherUid: 'carol' }), 400, 'INVALID_INPUT');
  // Mutual blocks: lifting one leaves the other in force.
  await act(alice, { action: 'unblock', otherUid: 'bob' });
  await act(bob, { action: 'block', otherUid: 'alice' }); await act(alice, { action: 'block', otherUid: 'bob' });
  await act(alice, { action: 'unblock', otherUid: 'bob' });
  await rejects(request(alice, bob), 404, 'NOT_FOUND');
  await act(bob, { action: 'unblock', otherUid: 'alice' });
  assert.equal((await request(alice, bob)).state, 'outgoing');
});

test('block: the block keeps the person\'s name for the Blocked list — profile first, else the card, else Member — and a repeat keeps the first', options, async () => {
  for (const user of [alice, bob, carol]) await friends.profile(user);
  const entry = async other => { const { at, ...rest } = await data(`users/alice/blocks/${other}`); assert.ok(at.toMillis() > 0); return rest; };
  // A friend: the name and handle their profile has, in the document the owner reads on any device.
  await act(bob, { action: 'profile', handle: 'Bob_B' });
  await befriend(alice, bob);
  await act(alice, { action: 'block', otherUid: 'bob' });
  assert.deepEqual(await entry('bob'), { displayName: 'Bob', handle: 'Bob_B' });
  // A repeat, even after a rename, keeps the first entry whole.
  await act(bob, { action: 'profile', displayName: 'Robert' });
  await act(alice, { action: 'block', otherUid: 'bob' });
  assert.deepEqual(await entry('bob'), { displayName: 'Bob', handle: 'Bob_B' });
  // The profile wins over the name on the card from when the request was made.
  await request(carol, alice);
  assert.equal((await data('users/alice/friends/carol')).displayName, 'Carol');
  await act(carol, { action: 'profile', displayName: 'Caro' });
  await act(alice, { action: 'block', otherUid: 'carol' });
  assert.deepEqual(await entry('carol'), { displayName: 'Caro' });
  // No profile: the card the owner already had names them. Here an incoming request from an account that never opened Friends.
  const erin = { uid: 'erin', name: 'Erin' };
  accounts.set(mail(erin), { uid: 'erin', emailVerified: true });
  await act(erin, { action: 'request', via: 'email', email: mail(alice), requestId: rid() });
  assert.equal((await db.doc('profiles/erin').get()).exists, false);
  await act(alice, { action: 'block', otherUid: 'erin' });
  assert.deepEqual(await entry('erin'), { displayName: 'Erin' });
  // Neither profile nor card: the same fallback as every other card, and a uid nobody has is accepted as before.
  await act(alice, { action: 'block', otherUid: 'dave' });
  await act(alice, { action: 'block', otherUid: 'nobody-here' });
  assert.deepEqual(await entry('dave'), { displayName: 'Member' });
  assert.deepEqual(await entry('nobody-here'), { displayName: 'Member' });
  // The owner reads it; nobody else does.
  const mine = environment.authenticatedContext('alice').firestore();
  assert.equal((await assertSucceeds(getDoc(doc(mine, 'users/alice/blocks/bob')))).data().displayName, 'Bob');
  await assertFails(getDoc(doc(environment.authenticatedContext('bob').firestore(), 'users/alice/blocks/bob')));
  // Lifting the block removes the entry with its name; blocking again takes the name as it is then.
  await act(alice, { action: 'unblock', otherUid: 'bob' });
  assert.equal((await db.doc('users/alice/blocks/bob').get()).exists, false);
  await act(alice, { action: 'block', otherUid: 'bob' });
  assert.deepEqual(await entry('bob'), { displayName: 'Robert', handle: 'Bob_B' });
});

// ─── Team offers (F4) ───────────────────────────────────────────────────────────────────────────────────────────────
const DAY = 24 * 3600 * 1000;
const offerOf = (from, teamId, to, extra = {}) => act(from, { action: 'offerTeam', teamId, friendUid: to.uid, requestId: rid(), ...extra });
const accepts = (to, offerId, extra = {}) => act(to, { action: 'acceptOffer', offerId, requestId: rid(), ...extra });
const makeTeam = (owner, teamId, name = 'Alpha') => teams.execute(owner, { action: 'create', teamId, name });
const offersOf = uid => ids(`users/${uid}/teamOffers`);
const offerPushes = () => events.filter(event => event.kind === 'offer');
// Joins `member` to `teamId` through an invite link from `owner`.
async function admit(owner, member, teamId, role = 'member') {
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: rid(), role, maxUses: 5, ttlSeconds: 3600 });
  return teams.execute(member, { action: 'join', code, requestId: rid() });
}

test('offerTeam: a manager offers a team as one document in the friend\'s own collection — one push, a replay-safe receipt, and a repeat refreshes instead of adding', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha');
  const requestId = rid(), command = { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId };
  const first = await act(alice, command);
  assert.deepEqual(first, { offerId: 'team-alpha', expiresAt: new Date(now + OFFER_TTL_MS).toISOString() });
  const stored = await data('users/bob/teamOffers/team-alpha');
  assert.deepEqual(Object.keys(stored).sort(), ['expiresAt', 'fromName', 'fromUid', 'role', 'teamId', 'teamName']);
  assert.deepEqual([stored.teamId, stored.teamName, stored.fromUid, stored.fromName, stored.role, stored.expiresAt.toMillis()], ['team-alpha', 'Alpha', 'alice', 'Alice', 'member', now + OFFER_TTL_MS]);
  assert.deepEqual(await offersOf('alice'), [], 'the sender holds nothing');
  assert.deepEqual(offerPushes(), [{ kind: 'offer', to: 'bob', from: 'alice', name: 'Alice', offerId: 'team-alpha', key: `${pairKey('alice', 'bob')}:${requestId}` }]);
  // An offer grants nothing: the friend is not a member and has no team.
  assert.equal((await db.doc('teams/team-alpha/members/bob').get()).exists, false);
  assert.deepEqual((await teams.list('bob')).teams, []);
  const receipt = await data(`users/alice/friendRequests/${requestId}`);
  assert.deepEqual(Object.keys(receipt).sort(), ['at', 'digest', 'expiresAt', 'offerId', 'otherUid']);
  assert.deepEqual([receipt.otherUid, receipt.offerId, receipt.expiresAt.toMillis()], ['bob', 'team-alpha', now + OFFER_TTL_MS]);
  // A replay answers with the first result, writes nothing and pushes nothing, even later; another request under the same id is a conflict.
  now += 2 * DAY;
  assert.deepEqual(await act(alice, command), first);
  assert.equal(offerPushes().length, 1);
  assert.equal((await data('users/bob/teamOffers/team-alpha')).expiresAt.toMillis(), stored.expiresAt.toMillis());
  await makeTeam(alice, 'team-beta', 'Beta');
  await rejects(act(alice, { ...command, teamId: 'team-beta' }), 409, 'REQUEST_ID_CONFLICT');
  await rejects(act(alice, { ...command, friendUid: 'carol' }), 409, 'REQUEST_ID_CONFLICT');
  await rejects(act(alice, { action: 'request', via: 'email', email: mail(carol), requestId }), 409, 'REQUEST_ID_CONFLICT');
  // A new request while the offer is live refreshes it in place (a new expiry, the same document) and stays silent.
  const refreshed = await offerOf(alice, 'team-alpha', bob);
  assert.equal(refreshed.expiresAt, new Date(now + OFFER_TTL_MS).toISOString());
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
  assert.equal((await data('users/bob/teamOffers/team-alpha')).expiresAt.toMillis(), now + OFFER_TTL_MS);
  assert.equal(offerPushes().length, 1);
  // Once it has expired it is still listed (until pruned); offering again revives the same document and tells the friend again.
  now += OFFER_TTL_MS + 1;
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
  await offerOf(alice, 'team-alpha', bob);
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
  assert.equal(offerPushes().length, 2);
  // Another team is another document.
  await offerOf(alice, 'team-beta', bob);
  assert.deepEqual(await offersOf('bob'), ['team-alpha', 'team-beta']);
  assert.equal(offerPushes().length, 3);
  assert.equal(await consistent(alice, bob), 'friends');
});

test('offerTeam: only an owner or admin of an ordinary active team may offer, to a current friend who is not a member — each refusal has its own code and writes nothing', options, async () => {
  await befriend(bob, alice); await linked(carol, bob); await friends.profile(dave);
  await makeTeam(alice, 'team-alpha'); await admit(alice, bob, 'team-alpha');
  // A plain member cannot, and neither can a stranger to the team; an admin can.
  await rejects(offerOf(bob, 'team-alpha', carol), 403, 'ROLE_REQUIRED');
  await rejects(offerOf(carol, 'team-alpha', bob), 403, 'MEMBERSHIP_REQUIRED');
  await teams.execute(alice, { action: 'role', teamId: 'team-alpha', uid: 'bob', generation: 1, role: 'admin' });
  assert.equal((await offerOf(bob, 'team-alpha', carol)).offerId, 'team-alpha');
  assert.deepEqual(await offersOf('carol'), ['team-alpha']);
  assert.equal((await data('users/carol/teamOffers/team-alpha')).fromName, 'Bob');
  // The friend must be a friend: a stranger, a pending request and an unknown account are one answer.
  await rejects(offerOf(alice, 'team-alpha', dave), 403, 'FRIEND_REQUIRED');
  await request(dave, alice);
  await rejects(offerOf(alice, 'team-alpha', dave), 403, 'FRIEND_REQUIRED');
  await rejects(offerOf(alice, 'team-alpha', { uid: 'nobody-here' }), 403, 'FRIEND_REQUIRED');
  // A member, a full team, a pair team, a missing and a deleted team, oneself.
  await rejects(offerOf(alice, 'team-alpha', bob), 409, 'ALREADY_MEMBER');
  await db.doc('teams/team-alpha').update({ memberCount: TEAM_MEMBER_LIMIT });
  await rejects(offerOf(bob, 'team-alpha', carol), 409, 'MEMBER_LIMIT');
  await db.doc('teams/team-alpha').update({ memberCount: 2 });
  await rejects(offerOf(alice, pairTeamId('alice', 'bob', 1), carol), 400, 'PAIR_TEAM');
  await rejects(offerOf(carol, pairTeamId('alice', 'bob', 1), bob), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(offerOf(alice, 'team-missing', carol), 403, 'MEMBERSHIP_REQUIRED');
  await makeTeam(alice, 'team-gone', 'Gone'); await teams.execute(alice, { action: 'delete', teamId: 'team-gone' });
  await rejects(offerOf(alice, 'team-gone', carol), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(act(alice, { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'alice', requestId: rid() }), 400, 'INVALID_INPUT');
  // A blocked friend is no friend; the block took their offers with it.
  await act(carol, { action: 'block', otherUid: 'bob' });
  await rejects(offerOf(bob, 'team-alpha', carol), 403, 'FRIEND_REQUIRED');
  assert.deepEqual(await offersOf('carol'), []);
  // Only the two successful offers left a receipt or an offer, and the pair team stayed whole.
  assert.deepEqual(await offersOf('dave'), []); assert.deepEqual(await offersOf('bob'), []); assert.deepEqual(await offersOf('alice'), []);
  assert.equal((await db.collection('users/alice/friendRequests').get()).docs.filter(doc => doc.data().offerId).length, 0);
  assert.equal((await db.collection('users/bob/friendRequests').get()).docs.filter(doc => doc.data().offerId).length, 1);
  assert.equal(await consistent(alice, bob), 'friends');
});

test('offerTeam: a friend holds at most 20 live offers (a refresh of a live one still fits, an expired one does not count) and offering spends the request budget', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha'); await makeTeam(alice, 'team-beta', 'Beta');
  // Eighteen offers from other teams are already live; two from this friend fill the last places.
  const stub = number => ({ teamId: `stub-offer-${number}`, teamName: `Stub ${number}`, fromUid: 'carol', fromName: 'Carol', role: 'member', expiresAt: new Date(now + 30 * DAY) });
  for (let i = 0; i < OFFER_LIMIT - 2; i++) await db.doc(`users/bob/teamOffers/stub-offer-${i}`).set(stub(i));
  await offerOf(alice, 'team-alpha', bob);
  await offerOf(alice, 'team-beta', bob);
  assert.equal((await offersOf('bob')).length, OFFER_LIMIT);
  await makeTeam(alice, 'team-gamma', 'Gamma');
  await rejects(offerOf(alice, 'team-gamma', bob), 409, 'OFFER_LIMIT');
  assert.equal((await db.doc('users/bob/teamOffers/team-gamma').get()).exists, false);
  // Refreshing a live offer adds no place.
  assert.equal((await offerOf(alice, 'team-alpha', bob)).offerId, 'team-alpha');
  // Declining frees one; an expired offer does not count against the limit, and reviving it needs a free place.
  await act(bob, { action: 'declineOffer', offerId: 'stub-offer-0' });
  assert.equal((await offerOf(alice, 'team-gamma', bob)).offerId, 'team-gamma');
  await db.doc('users/bob/teamOffers/team-alpha').update({ expiresAt: new Date(now - 1000) }); // live: 19
  assert.equal((await offerOf(alice, 'team-alpha', bob)).offerId, 'team-alpha'); // live: 20 again
  await db.doc('users/bob/teamOffers/team-beta').update({ expiresAt: new Date(now - 1000) }); // live: 19
  await db.doc('users/bob/teamOffers/stub-offer-extra').set(stub('extra')); // live: 20
  await rejects(offerOf(alice, 'team-beta', bob), 409, 'OFFER_LIMIT');
});

test('offerTeam: offers, requests and redeems share one budget of 30 per 10 minutes, refused offers included', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha');
  for (let i = 0; i < LOOKUP_LIMIT - 1; i++) await rejects(offerOf(alice, 'team-alpha', dave), 403, 'FRIEND_REQUIRED');
  // Alice has spent 29 of her 30 on refused offers; the friendship was Bob's request and Alice's accept, which costs her nothing.
  await offerOf(alice, 'team-alpha', bob);
  await rejects(offerOf(alice, 'team-alpha', bob), 429, 'RATE_LIMITED');
  await rejects(request(alice, carol), 429, 'RATE_LIMITED');
  now += LOOKUP_WINDOW_MS + 1;
  assert.equal((await offerOf(alice, 'team-alpha', bob)).offerId, 'team-alpha');
});

test('acceptOffer: the friend joins through the same core as an invite link — a new generation, the team index, the member count and the signal — and the offer is gone', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha'); await offerOf(alice, 'team-alpha', bob);
  const requestId = rid(), command = { action: 'acceptOffer', offerId: 'team-alpha', requestId };
  assert.deepEqual(await act(bob, command), { teamId: 'team-alpha', generation: 1, alreadyMember: false });
  const member = await data('teams/team-alpha/members/bob');
  assert.deepEqual([member.displayName, member.role, member.generation], ['Bob', 'member', 1]);
  assert.deepEqual(await data('teams/team-alpha/membershipEpochs/bob'), { generation: 1 });
  const index = await data('users/bob/teams/team-alpha');
  assert.deepEqual([index.role, index.generation], ['member', 1]);
  assert.equal((await data('teams/team-alpha')).memberCount, 2);
  assert.ok((await data('teams/team-alpha/signals/membership')).updatedAt);
  assert.deepEqual(await offersOf('bob'), []);
  assert.deepEqual(await data(`users/bob/teamJoinRequests/${requestId}`), { teamId: 'team-alpha', offerId: 'team-alpha', generation: 1 });
  assert.deepEqual((await teams.list('bob')).teams.map(team => [team.id, team.role, team.generation, team.memberCount]), [['team-alpha', 'member', 1, 2]]);
  // A replay is the same answer and changes nothing; another offer or a link join under the same id conflicts.
  assert.deepEqual(await act(bob, command), { teamId: 'team-alpha', generation: 1, alreadyMember: true });
  assert.equal((await data('teams/team-alpha')).memberCount, 2);
  await rejects(act(bob, { ...command, offerId: 'team-other1' }), 409, 'REQUEST_ID_CONFLICT');
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'team-alpha', requestId: 'invite-01', role: 'member', maxUses: 2, ttlSeconds: 3600 });
  await rejects(teams.execute(bob, { action: 'join', code, requestId }), 409, 'REQUEST_ID_CONFLICT');
  // After leaving, the old receipt no longer answers for the membership; a new offer joins with the next generation.
  await teams.execute(bob, { action: 'leave', teamId: 'team-alpha', generation: 1 });
  await rejects(act(bob, command), 409, 'STALE_MEMBERSHIP');
  await offerOf(alice, 'team-alpha', bob);
  assert.deepEqual(await accepts(bob, 'team-alpha'), { teamId: 'team-alpha', generation: 2, alreadyMember: false });
  assert.equal((await data('teams/team-alpha')).memberCount, 2);
  assert.equal(await consistent(alice, bob), 'friends');
});

test('acceptOffer: a refusal the friend can fix keeps the offer — a full team, the 20-team limit — and joining by any other route answers it', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha'); await offerOf(alice, 'team-alpha', bob);
  await db.doc('teams/team-alpha').update({ memberCount: TEAM_MEMBER_LIMIT });
  await rejects(accepts(bob, 'team-alpha'), 409, 'MEMBER_LIMIT');
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
  await db.doc('teams/team-alpha').update({ memberCount: 1 });
  for (let i = 0; i < USER_TEAM_LIMIT; i++) await makeTeam(bob, `bobs-team-${String(i).padStart(2, '0')}`, `Bob ${i}`);
  await rejects(accepts(bob, 'team-alpha'), 409, 'TEAM_LIMIT');
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
  assert.equal((await db.doc('teams/team-alpha/members/bob').get()).exists, false);
  await teams.execute(bob, { action: 'delete', teamId: 'bobs-team-00' });
  assert.equal((await accepts(bob, 'team-alpha')).alreadyMember, false);
  assert.equal((await teams.list('bob')).teams.length, USER_TEAM_LIMIT);
  // Another route into the team answers the offer: an invite link takes it away, and accepting it afterwards finds nothing.
  await makeTeam(alice, 'team-beta', 'Beta'); await offerOf(alice, 'team-beta', bob);
  await teams.execute(bob, { action: 'leave', teamId: 'team-alpha', generation: 1 });
  assert.deepEqual(await offersOf('bob'), ['team-beta']);
  await admit(alice, bob, 'team-beta');
  assert.deepEqual(await offersOf('bob'), []);
  await rejects(accepts(bob, 'team-beta'), 404, 'OFFER_UNAVAILABLE');
});

test('acceptOffer: an offer that cannot be honoured any more is deleted and answers OFFER_UNAVAILABLE — expired, team deleted, sender demoted, removed or gone — or is simply gone when declined', options, async () => {
  await befriend(bob, alice); await linked(carol, bob);
  const cases = [
    ['expired', async () => { now += OFFER_TTL_MS; }],
    ['team deleted', async id => { await teams.execute(alice, { action: 'delete', teamId: id }); }],
    ['sender demoted', async id => { await teams.execute(alice, { action: 'role', teamId: id, uid: 'carol', generation: 1, role: 'member' }); }],
    ['sender removed', async id => { await teams.execute(alice, { action: 'remove', teamId: id, uid: 'carol', generation: 1 }); }],
    ['sender left', async id => { await teams.execute(carol, { action: 'leave', teamId: id, generation: 1 }); }],
  ];
  for (const [index, [name, prepare]] of cases.entries()) {
    const teamId = `stale-team-${index}`;
    await makeTeam(alice, teamId, `Stale ${index}`); await admit(alice, carol, teamId, 'admin');
    await offerOf(carol, teamId, bob);
    await prepare(teamId);
    const count = (await data(`teams/${teamId}`)).memberCount;
    await rejects(accepts(bob, teamId), 404, 'OFFER_UNAVAILABLE');
    assert.deepEqual(await offersOf('bob'), [], name);
    assert.equal((await db.doc(`teams/${teamId}/members/bob`).get()).exists, false, name);
    assert.equal((await db.doc(`users/bob/teams/${teamId}`).get()).exists, false, name);
    assert.equal((await data(`teams/${teamId}`)).memberCount, count, name);
    await rejects(accepts(bob, teamId), 404, 'OFFER_UNAVAILABLE'); // it is gone for good
  }
  // The boundary: an offer is live until its expiry instant and expired from it.
  await makeTeam(alice, 'edge-team-1', 'Edge'); await offerOf(alice, 'edge-team-1', bob);
  now += OFFER_TTL_MS - 1;
  assert.equal((await accepts(bob, 'edge-team-1')).alreadyMember, false);
  await makeTeam(alice, 'edge-team-2', 'Edge two'); await offerOf(alice, 'edge-team-2', bob);
  now += OFFER_TTL_MS;
  await rejects(accepts(bob, 'edge-team-2'), 404, 'OFFER_UNAVAILABLE');
  // Declining deletes the offer; a repeat, an unknown offer and the sender's own view are harmless or empty.
  await makeTeam(alice, 'team-alpha'); await offerOf(alice, 'team-alpha', bob);
  assert.deepEqual(await act(bob, { action: 'declineOffer', offerId: 'team-alpha' }), { ok: true });
  assert.deepEqual(await act(bob, { action: 'declineOffer', offerId: 'team-alpha' }), { ok: true });
  assert.deepEqual(await act(bob, { action: 'declineOffer', offerId: 'never-offered' }), { ok: true });
  await rejects(accepts(bob, 'team-alpha'), 404, 'OFFER_UNAVAILABLE');
  assert.equal((await db.doc('teams/team-alpha/members/bob').get()).exists, false);
  // An offer cannot be accepted by anyone but its recipient: Carol has none under that id.
  await offerOf(alice, 'team-alpha', bob);
  await rejects(accepts(carol, 'team-alpha'), 404, 'OFFER_UNAVAILABLE');
  assert.deepEqual(await offersOf('bob'), ['team-alpha']);
});

test('acceptOffer: a double tap, two taps with different ids, and an accept racing an unfriend each end in one consistent state', options, async () => {
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha'); await offerOf(alice, 'team-alpha', bob);
  const same = rid();
  const taps = await Promise.allSettled([1, 2, 3, 4].map(() => act(bob, { action: 'acceptOffer', offerId: 'team-alpha', requestId: same })));
  assert.ok(taps.every(tap => tap.status === 'fulfilled'), 'a repeated tap is answered, not refused');
  assert.equal(taps.filter(tap => tap.value.alreadyMember === false).length, 1);
  assert.deepEqual([...new Set(taps.map(tap => tap.value.generation))], [1]);
  assert.equal((await data('teams/team-alpha')).memberCount, 2);
  await makeTeam(alice, 'team-beta', 'Beta'); await offerOf(alice, 'team-beta', bob);
  const two = await Promise.allSettled([accepts(bob, 'team-beta'), accepts(bob, 'team-beta')]);
  assert.equal(two.filter(result => result.status === 'fulfilled').length, 1);
  for (const result of two) if (result.status === 'rejected') assert.equal(result.reason.code, 'OFFER_UNAVAILABLE');
  assert.equal((await data('teams/team-beta')).memberCount, 2);
  // Either the friend joined first and keeps the team, or the offer was gone: never an offer for an ended friendship, never half a membership.
  await makeTeam(alice, 'team-gamma', 'Gamma'); await offerOf(alice, 'team-gamma', bob);
  const raced = await Promise.allSettled([accepts(bob, 'team-gamma'), act(alice, { action: 'remove', otherUid: 'bob' })]);
  assert.equal(raced[1].status, 'fulfilled');
  if (raced[0].status === 'rejected') assert.equal(raced[0].reason.code, 'OFFER_UNAVAILABLE');
  assert.deepEqual(await offersOf('bob'), []);
  assert.equal(await consistent(alice, bob), 'ended');
  const joined = (await db.doc('teams/team-gamma/members/bob').get()).exists;
  assert.equal(joined, raced[0].status === 'fulfilled');
  assert.equal((await db.doc('users/bob/teams/team-gamma').get()).exists, joined);
  assert.equal((await data('teams/team-gamma')).memberCount, joined ? 2 : 1);
});

test('remove and block delete the offers between the two friends, in both directions and only theirs — an offer a third friend refreshed stays until that friendship ends too', options, async () => {
  await befriend(bob, alice); await linked(carol, alice); await linked(carol, bob);
  await makeTeam(alice, 'alice-team1', 'A1'); await makeTeam(alice, 'alice-team2', 'A2'); await makeTeam(bob, 'bob-team01', 'B1'); await makeTeam(carol, 'carol-team1', 'C1');
  await admit(alice, carol, 'alice-team1', 'admin');
  const state = async () => Object.fromEntries(await Promise.all(['alice', 'bob', 'carol'].map(async uid =>
    [uid, (await db.collection(`users/${uid}/teamOffers`).get()).docs.map(doc => `${doc.id}<-${doc.data().fromUid}`).sort()])));
  await offerOf(alice, 'alice-team1', bob); await offerOf(alice, 'alice-team2', carol); await offerOf(bob, 'bob-team01', alice);
  await offerOf(carol, 'carol-team1', alice); await offerOf(carol, 'carol-team1', bob); await offerOf(bob, 'bob-team01', carol);
  await offerOf(carol, 'alice-team1', bob); // Carol, an admin of Alice's team, refreshes the live offer: it is Carol's now
  assert.deepEqual(await state(), { alice: ['bob-team01<-bob', 'carol-team1<-carol'], bob: ['alice-team1<-carol', 'carol-team1<-carol'], carol: ['alice-team2<-alice', 'bob-team01<-bob'] });
  await act(alice, { action: 'remove', otherUid: 'bob' });
  assert.deepEqual(await state(), { alice: ['carol-team1<-carol'], bob: ['alice-team1<-carol', 'carol-team1<-carol'], carol: ['alice-team2<-alice', 'bob-team01<-bob'] });
  await act(alice, { action: 'block', otherUid: 'carol' });
  assert.deepEqual(await state(), { alice: [], bob: ['alice-team1<-carol', 'carol-team1<-carol'], carol: ['bob-team01<-bob'] });
  await act(bob, { action: 'block', otherUid: 'carol' });
  assert.deepEqual(await state(), { alice: [], bob: [], carol: [] });
  // A pending request or a decline owns no offers, so it deletes none.
  await db.doc('users/dave/teamOffers/dave-offer1').set({ teamId: 'dave-offer1', teamName: 'D', fromUid: 'alice', fromName: 'Alice', role: 'member', expiresAt: new Date(now + DAY) });
  await request(dave, alice); await act(alice, { action: 'decline', otherUid: 'dave' });
  assert.deepEqual(await offersOf('dave'), ['dave-offer1']);
});

test('prune: an offer is deleted a week after it expired — not before, in as many pages as it takes — and the live ones stay', options, async () => {
  const offer = (expiresAt, fromUid = 'carol') => ({ teamId: 'any-team-01', teamName: 'Any', fromUid, fromName: 'Carol', role: 'member', expiresAt: new Date(expiresAt) });
  await db.doc('users/bob/teamOffers/live-offer1').set(offer(now + DAY));
  await db.doc('users/bob/teamOffers/just-expired').set(offer(now - DAY));
  await db.doc('users/bob/teamOffers/almost-gone').set(offer(now - OFFER_GRACE_MS + 1));
  await db.doc('users/bob/teamOffers/at-boundary').set(offer(now - OFFER_GRACE_MS));
  await db.doc('users/alice/teamOffers/long-gone-1').set(offer(now - OFFER_GRACE_MS - 1));
  const writer = db.bulkWriter();
  for (let i = 0; i < 230; i++) writer.set(db.doc(`users/user-${i % 23}/teamOffers/old-offer-${i}`), offer(now - 30 * DAY));
  await writer.close();
  await db.doc('friendLimits/spent').set({ sentAt: [], expiresAt: new Date(now - 1000) });
  await friends.prune();
  assert.deepEqual(await offersOf('bob'), ['almost-gone', 'just-expired', 'live-offer1']);
  assert.deepEqual(await offersOf('alice'), []);
  assert.equal((await db.collectionGroup('teamOffers').get()).size, 3);
  assert.equal((await db.doc('friendLimits/spent').get()).exists, false);
  // Pruning is idempotent.
  await friends.prune();
  assert.equal((await db.collectionGroup('teamOffers').get()).size, 3);
});

test('push: a team offer notifies the friend once through the real notifier — the sender\'s name, generic text, the requests route, no team name — and a repeat, a decline or a broken provider changes nothing', options, async () => {
  const sent = [], expiry = { toMillis: () => now + 3600000 };
  const device = (uid, locale) => ({ id: `device-${uid}`, uid, revision: 1, fcmInstallationId: `fid-${uid}`, expiresAt: expiry, locale, appId: 'com.lingxi.code', platform: 'ios' });
  const devices = [device('alice', 'en-US'), device('bob', 'zh-CN')];
  const pushStore = { async active(uid) { return devices.filter(item => item.uid === uid); }, device(id) { return { async get() { return { data: () => devices.find(item => item.id === id) }; } }; } };
  let broken = false;
  const live = friendNotifier({ db, pushStore, messaging: { async send(value) { if (broken) throw new Error('provider down'); sent.push(value); } }, clock: () => now });
  friends = new FriendsStore(db, () => now, { auth, notify: live });
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha', 'Secret Project'); await makeTeam(alice, 'team-beta', 'Beta');
  const before = sent.length;
  await offerOf(alice, 'team-alpha', bob);
  assert.equal(sent.length, before + 1);
  const message = sent.at(-1);
  assert.deepEqual([message.data.recipientUid, message.data.route, message.data.kind, message.data.title, message.data.body], ['bob', 'friend/requests', 'friend_offer', 'Alice', '邀请你加入团队']);
  assert.ok(!JSON.stringify(message).includes('Secret Project'));
  await offerOf(alice, 'team-alpha', bob); // a repeat while live
  assert.equal(sent.length, before + 1);
  await act(bob, { action: 'declineOffer', offerId: 'team-alpha' });
  await live({ kind: 'offer', to: 'bob', from: 'alice', name: 'Alice', offerId: 'team-alpha', key: 'late' }); // a late push for a declined offer
  assert.equal(sent.length, before + 1);
  broken = true;
  assert.equal((await offerOf(alice, 'team-beta', bob)).offerId, 'team-beta');
  assert.deepEqual(await offersOf('bob'), ['team-beta']);
  friends = new FriendsStore(db, () => now, { auth, notify: async () => { throw new Error('notifier bug'); } });
  assert.equal((await offerOf(alice, 'team-alpha', bob)).offerId, 'team-alpha');
});

test('HTTP: team offers use the Friends route and the Teams error shape end to end', options, async () => {
  const users = { 'token-alice': alice, 'token-bob': bob };
  const handler = createTeamsHandler({ store: teams, friends, origins: ['https://lingxios.net'], createCustomToken: async () => '',
    verifyToken: async token => users[token] ?? Promise.reject(new Error('invalid')) });
  const call = (token, body) => handler(new Request('https://test.invalid/v1/friends', { method: 'POST', headers: { Authorization: `Bearer ${token}`, Origin: 'https://lingxios.net' }, body: JSON.stringify(body) }));
  const reply = async response => [response.status, await response.json()];
  await befriend(bob, alice); await makeTeam(alice, 'team-alpha');
  assert.deepEqual(await reply(await call('token-alice', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId: 'http-offer-01' })), [200, { offerId: 'team-alpha', expiresAt: new Date(now + OFFER_TTL_MS).toISOString() }]);
  assert.deepEqual(await reply(await call('token-alice', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'carol', requestId: 'http-offer-02' })), [403, { error: { code: 'FRIEND_REQUIRED' } }]);
  assert.deepEqual(await reply(await call('token-bob', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'alice', requestId: 'http-offer-03' })), [403, { error: { code: 'MEMBERSHIP_REQUIRED' } }]);
  assert.deepEqual(await reply(await call('token-alice', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId: 'http-offer-04', role: 'admin' })), [400, { error: { code: 'INVALID_INPUT' } }]);
  assert.deepEqual(await reply(await call('token-bob', { action: 'acceptOffer', offerId: 'team-alpha', requestId: 'http-accept-1' })), [200, { teamId: 'team-alpha', generation: 1, alreadyMember: false }]);
  assert.deepEqual(await reply(await call('token-bob', { action: 'acceptOffer', offerId: 'team-alpha', requestId: 'http-accept-2' })), [404, { error: { code: 'OFFER_UNAVAILABLE' } }]);
  assert.deepEqual(await reply(await call('token-bob', { action: 'declineOffer', offerId: 'team-alpha' })), [200, { ok: true }]);
  assert.deepEqual(await reply(await call('token-alice', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId: 'http-offer-05' })), [409, { error: { code: 'ALREADY_MEMBER' } }]);
});

test('pair teams: every Teams action rejects them with PAIR_TEAM, they are never listed or counted, and the id space is reserved', options, async () => {
  await befriend(bob, alice);
  const teamId = pairTeamId('alice', 'bob', 1);
  const attempts = [
    { action: 'invite', teamId, requestId: 'invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 },
    { action: 'revokeInvite', teamId, inviteId: 'a'.repeat(64) }, { action: 'leave', teamId, generation: 1 },
    { action: 'remove', teamId, uid: 'bob', generation: 1 }, { action: 'role', teamId, uid: 'bob', generation: 1, role: 'admin' },
    { action: 'delete', teamId }, { action: 'create', teamId, name: 'Friends' }, { action: 'create', teamId: 'f_unclaimed_pair_id', name: 'Anything' },
  ];
  for (const actor of [alice, bob]) for (const attempt of attempts) await rejects(teams.execute(actor, attempt), 400, 'PAIR_TEAM');
  // An outsider learns nothing: the membership check comes first.
  await rejects(teams.execute(carol, { action: 'delete', teamId }), 403, 'MEMBERSHIP_REQUIRED');
  await rejects(teams.execute(carol, { action: 'remove', teamId, uid: 'bob', generation: 1 }), 403, 'MEMBERSHIP_REQUIRED');
  // A forged invite that points at a pair team cannot be used to join it.
  await db.doc(`teamInvites/${sha('forged-code')}`).set({ teamId });
  await db.doc(`teams/${teamId}/invites/${sha('forged-code')}`).set({ role: 'member', createdBy: 'alice', createdAt: new Date(now), expiresAt: new Date(now + 3600000), maxUses: 5, uses: 0, revoked: false });
  await rejects(teams.execute(carol, { action: 'join', code: 'forged-code'.padEnd(32, 'x'), requestId: 'join-carol' }), 404, 'INVITE_UNAVAILABLE');
  await db.doc(`teamInvites/${sha('f'.repeat(32))}`).set({ teamId });
  await db.doc(`teams/${teamId}/invites/${sha('f'.repeat(32))}`).set({ role: 'member', createdBy: 'alice', createdAt: new Date(now), expiresAt: new Date(now + 3600000), maxUses: 5, uses: 0, revoked: false });
  await rejects(teams.execute(carol, { action: 'join', code: 'f'.repeat(32), requestId: 'join-carol' }), 400, 'PAIR_TEAM');
  assert.deepEqual(await ids(`teams/${teamId}/members`), ['alice', 'bob']);
  // No client conversation can be added, through the store or the HTTP route; the backend's own dm is the only one.
  const scope = { teamId, generation: 1 };
  for (const kind of ['dm', 'group', 'channel']) {
    await rejects(messages.execute(alice, { action: 'create', ...scope, conversationId: `extra-${kind}`, kind, title: 'Extra', participantUids: ['alice', 'bob'] }), 400, 'PAIR_TEAM');
  }
  await rejects(messages.execute(alice, { action: 'create', ...scope, conversationId: PAIR_CONVERSATION_ID, kind: 'dm', title: 'Friends', participantUids: ['alice', 'bob'] }), 400, 'PAIR_TEAM');
  await rejects(messages.execute(carol, { action: 'create', ...scope, conversationId: 'extra-dm', kind: 'dm', title: 'Extra', participantUids: ['carol', 'alice'] }), 403, 'MEMBERSHIP_REQUIRED');
  assert.deepEqual(await ids(`teams/${teamId}/conversations`), [PAIR_CONVERSATION_ID]);
  const handler = createTeamsHandler({ store: teams, friends, verifyToken: async token => ({ alice, bob, carol }[token] ?? Promise.reject(new Error('bad'))), createCustomToken: async () => '' });
  const call = (token, path, method, body) => handler(new Request(`https://test.invalid${path}`, { method, headers: { Authorization: `Bearer ${token}` }, ...(body ? { body: JSON.stringify(body) } : {}) }));
  const viaHttp = await call('alice', '/v1/teams/messages', 'POST', { action: 'create', ...scope, conversationId: 'extra-http', kind: 'group', title: 'Extra', participantUids: ['alice', 'bob'] });
  assert.deepEqual([viaHttp.status, await viaHttp.json()], [400, { error: { code: 'PAIR_TEAM' } }]);
  const viaTeams = await call('bob', '/v1/teams', 'POST', { action: 'delete', teamId });
  assert.deepEqual([viaTeams.status, await viaTeams.json()], [400, { error: { code: 'PAIR_TEAM' } }]);
  // The list never shows it, detail does, and the user's ordinary-team allowance is untouched.
  for (const token of ['alice', 'bob']) assert.deepEqual(await (await call(token, '/v1/teams', 'GET')).json(), { teams: [] });
  const detail = await call('bob', `/v1/teams/${teamId}`, 'GET');
  assert.equal(detail.status, 200);
  assert.equal((await detail.json()).team.kind, 'pair');
  for (let i = 0; i < USER_TEAM_LIMIT; i++) await teams.execute(alice, { action: 'create', teamId: `real-team-${String(i).padStart(2, '0')}`, name: `Team ${i}` });
  await rejects(teams.execute(alice, { action: 'create', teamId: 'real-team-over', name: 'Over' }), 409, 'TEAM_LIMIT');
  assert.equal((await teams.list('alice')).teams.length, USER_TEAM_LIMIT);
  // With a full team allowance, friends can still be made, and a user with many friends can still create teams.
  assert.equal((await act(alice, { action: 'redeem', code: await link(carol), requestId: rid() })).state, 'friends');
  assert.equal((await teams.list('alice')).teams.length, USER_TEAM_LIMIT);
  assert.equal((await teams.list('alice')).teams.some(item => item.kind), false);
  await act(bob, { action: 'redeem', code: await link(carol), requestId: rid() });
  for (let i = 0; i < USER_TEAM_LIMIT; i++) await teams.execute(bob, { action: 'create', teamId: `bobs-team-${String(i).padStart(2, '0')}`, name: `Bob ${i}` });
  assert.equal((await teams.list('bob')).teams.length, USER_TEAM_LIMIT);
});

test('pair teams: member messaging and read marks work unchanged inside a friendship', options, async () => {
  await befriend(bob, alice);
  const teamId = pairTeamId('alice', 'bob', 1), scope = { teamId, generation: 1, conversationId: PAIR_CONVERSATION_ID };
  const mutate = (actor, id, extra = {}) => messages.execute(actor, { action: 'mutate', ...scope, messageId: id, changeId: `c-${id}`, operation: 'create', text: `text ${id}`, ...extra });
  await mutate(alice, 'message01'); await mutate(bob, 'message02', { replyTo: 'message01' });
  const page = await messages.execute(bob, { action: 'sync', ...scope, after: 0 });
  assert.deepEqual([page.conversation.kind, page.conversation.lastSeq, page.messages.map(item => item.id).sort()], ['dm', 2, ['message01', 'message02']]);
  assert.equal((await messages.execute(bob, { action: 'read', ...scope, lastReadSeq: 2 })).lastReadSeq, 2);
  const listed = (await messages.execute(alice, { action: 'list', teamId, generation: 1 })).conversations;
  assert.deepEqual(listed.map(item => [item.id, item.kind, item.lastSeq, item.lastReadSeq]), [[PAIR_CONVERSATION_ID, 'dm', 2, 0]]);
  await rejects(mutate(carol, 'message03'), 403, 'MEMBERSHIP_REQUIRED');
});

test('shared Bots in a friendship (F3): published for all, the friend lists it and chats with it, the host answers, pausing stops it, and unfriending ends every part', options, async () => {
  await befriend(bob, alice);
  const teamId = pairTeamId('alice', 'bob', 1), bots = new SharedBotsStore(db, () => now), host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
  const command = (actor, action, payload, scope = teamId) => bots.execute(actor, { schemaVersion: 1, commandId: `command_${++serial}`, teamId: scope, generation: 1, action, payload });
  const publication = (expectedRevision = 0) => ({ sharedBotId: 'sharedbot_01', expectedRevision, name: 'Travel assistant', description: 'Shared notes', usePolicy: { mode: 'all' },
    deployment: { ...host, sharedBotId: 'sharedbot_01', revision: expectedRevision + 1, owner: { uid: 'alice', generation: 1 }, instanceId: 'instance_01', definitionDigest: 'a'.repeat(64),
      capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2, capabilities: { durableWait: false, writeOperations: false, delegation: false }, enabled: true },
    inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }], quotas: { team: 100, member: 20, bot: 100 } });
  const cardBefore = await data('users/bob/friends/alice');
  // Publishing for everyone in the friendship reaches the friend; the Bot is usable once the host's heartbeat says it is ready.
  assert.equal((await command(alice, 'publishSharedBot', publication())).revision, 1);
  assert.deepEqual((await command(bob, 'listSharedBots', {})).bots.map(bot => [bot.sharedBotId, bot.permitted, bot.ready, bot.maintainer.uid, bot.instanceId]), [['sharedbot_01', true, false, 'alice', undefined]]);
  await command(alice, 'heartbeatDeployment', { ...host, ready: true });
  assert.deepEqual((await command(bob, 'listSharedBots', {})).bots.map(bot => [bot.permitted, bot.ready, bot.state]), [[true, true, 'published']]);
  assert.equal((await command(alice, 'listSharedBots', {})).bots[0].instanceId, 'instance_01', 'only the maintainer sees the instance');
  await rejects(command(carol, 'listSharedBots', {}), 403, 'MEMBERSHIP_REQUIRED');
  // A friendship holds Bot DMs and its friend chat, never a group or a channel.
  for (const actor of [alice, bob]) for (const kind of ['group', 'channel']) {
    await rejects(command(actor, 'createConversation', { conversationId: `extra_${kind}_${actor.uid}`, kind, title: 'Extra', participantUids: ['alice', 'bob'], sharedBotIds: ['sharedbot_01'] }), 400, 'PAIR_TEAM');
  }
  const chat = await command(bob, 'createConversation', { conversationId: 'bot_chat_01', kind: 'bot_dm', title: 'Travel', participantUids: ['bob'], sharedBotIds: ['sharedbot_01'] });
  assert.deepEqual(chat.readers, ['alice', 'bob'], 'the maintainer can read the friend\'s chats with the Bot');
  assert.deepEqual(await ids(`teams/${teamId}/conversations`), ['bot_chat_01', PAIR_CONVERSATION_ID]);
  for (const actor of [alice, bob]) assert.deepEqual((await messages.execute(actor, { action: 'list', teamId, generation: 1 })).conversations.map(item => [item.id, item.kind]), [['bot_chat_01', 'bot_dm'], [PAIR_CONVERSATION_ID, 'dm']]);
  // The friend asks; the maintainer's host picks the task up, runs it and answers in the same conversation.
  const submitted = await command(bob, 'submit', { conversationId: 'bot_chat_01', messageId: 'pair_message_1', changeId: 'pair_change_01', text: 'What should we pack?', targets: ['sharedbot_01'] });
  assert.equal(submitted.tasks.length, 1);
  assert.deepEqual((await command(alice, 'listHostTasks', host)).tasks.map(task => [task.taskId, task.state]), [[submitted.tasks[0].taskId, 'queued']]);
  const started = await command(alice, 'startTask', { ...host, taskId: submitted.tasks[0].taskId, expectedLastChangeSeq: submitted.changeSeq });
  assert.equal(started.execute, true);
  const done = await command(alice, 'completeTask', { ...host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: 'Pack your passports.', finishedAt: now,
    inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, document: { title: 'Packing list', body: 'Pack your passports.' }, usage: { status: 'missing' } });
  assert.equal(done.state, 'completed');
  const chatPath = `teams/${teamId}/conversations/bot_chat_01`;
  assert.deepEqual((await db.collection(`${chatPath}/messages`).get()).docs.map(item => item.data().author.kind).sort(), ['shared_bot', 'user']);
  assert.equal((await command(bob, 'readArtifact', { artifactId: done.artifactId })).document.title, 'Packing list');
  // The friend chat and the friend's card do not move for any of it (the summary triggers watch the literal friend_chat path).
  const friendChat = await data(`teams/${teamId}/conversations/${PAIR_CONVERSATION_ID}`);
  assert.deepEqual([friendChat.lastSeq, friendChat.lastMessage], [0, null]);
  assert.deepEqual(await data('users/bob/friends/alice'), cardBefore);
  // Push: both directions open the friend chat with the other friend, titled by the author (the Bot's name for its answer).
  const sent = [], expiry = { toMillis: () => now + 3600000 };
  const device = uid => ({ id: `device-${uid}`, uid, revision: 1, fcmInstallationId: `fid-${uid}`, expiresAt: expiry, locale: 'en-US', appId: 'com.lingxi.code', platform: 'android' });
  const devices = [device('alice'), device('bob')];
  const pushStore = { async active(uid) { return devices.filter(item => item.uid === uid); }, device(id) { return { async get() { return { data: () => devices.find(item => item.id === id) }; } }; } };
  const push = async messageId => onTeamMessageCreated({ db, pushStore, messaging: { async send(value) { sent.push(value); } }, teamId, conversationId: 'bot_chat_01', messageId,
    original: (await db.doc(`${chatPath}/messages/${messageId}`).get()).data(), clock: () => now });
  await push('pair_message_1'); await push(done.replyId);
  assert.deepEqual(sent.map(value => [value.data.recipientUid, value.data.route, value.data.kind, value.data.title]),
    [['alice', 'friend/chat?uid=bob', 'friend_message', 'Bob'], ['bob', 'friend/chat?uid=alice', 'friend_message', 'Travel assistant']]);
  // Client reads follow the Team rules: both friends read the Bot chat, nobody else does.
  const read = (uid, path) => getDocs(collection(environment.authenticatedContext(uid).firestore(), path));
  assert.equal((await assertSucceeds(read('bob', `${chatPath}/messages`))).size, 2);
  assert.equal((await assertSucceeds(read('alice', `${chatPath}/messages`))).size, 2);
  await assertFails(read('carol', `${chatPath}/messages`));
  // Stopping the sharing is a pause: the friend no longer sees or uses the Bot, the history stays, and publishing again restores it.
  assert.equal((await command(alice, 'pauseSharedBot', { sharedBotId: 'sharedbot_01', expectedRevision: 1 })).revision, 2);
  assert.deepEqual((await command(bob, 'listSharedBots', {})).bots, []);
  assert.equal((await command(alice, 'listSharedBots', {})).bots[0].state, 'paused');
  await rejects(command(bob, 'submit', { conversationId: 'bot_chat_01', messageId: 'pair_message_2', changeId: 'pair_change_02', text: 'Still there?', targets: ['sharedbot_01'] }), 403, 'BOT_USE_REQUIRED');
  assert.equal((await assertSucceeds(read('bob', `${chatPath}/messages`))).size, 2);
  assert.equal((await command(alice, 'publishSharedBot', publication(2))).revision, 3);
  await command(alice, 'heartbeatDeployment', { ...host, ready: true });
  assert.deepEqual((await command(bob, 'listSharedBots', {})).bots.map(bot => [bot.permitted, bot.ready]), [[true, true]]);
  // Unfriending ends the Bot, its chat and the host's access, for both.
  await act(alice, { action: 'remove', otherUid: 'bob' });
  const again = { conversationId: 'bot_chat_01', messageId: 'pair_message_3', changeId: 'pair_change_03', text: 'More?', targets: ['sharedbot_01'] };
  for (const [actor, action, payload] of [[bob, 'listSharedBots', {}], [bob, 'submit', again], [alice, 'listSharedBots', {}],
    [alice, 'heartbeatDeployment', { ...host, ready: true }], [alice, 'listHostTasks', host]]) await rejects(command(actor, action, payload), 403, 'MEMBERSHIP_REQUIRED');
  for (const uid of ['alice', 'bob']) {
    await assertFails(read(uid, `${chatPath}/messages`));
    await assertFails(getDoc(doc(environment.authenticatedContext(uid).firestore(), chatPath)));
  }
  // A new friendship is a new team: none of the old Bots or chats come back.
  await befriend(alice, bob);
  const fresh = pairTeamId('alice', 'bob', 2);
  assert.deepEqual((await command(bob, 'listSharedBots', {}, fresh)).bots, []);
  assert.deepEqual(await ids(`teams/${fresh}/conversations`), [PAIR_CONVERSATION_ID]);
  await rejects(command(bob, 'submit', again, fresh), 403, 'CONVERSATION_REQUIRED');
});

test('push: a message in a pair team opens the friend chat with the sender; Team messages keep the Team inbox', options, async () => {
  await befriend(bob, alice);
  const teamId = pairTeamId('alice', 'bob', 1), scope = { teamId, generation: 1, conversationId: PAIR_CONVERSATION_ID };
  const expiry = { toMillis: () => now + 3600000 }, sent = [];
  const device = (uid, locale) => ({ id: `device-${uid}`, uid, revision: 1, fcmInstallationId: `fid-${uid}`, expiresAt: expiry, locale, appId: 'com.lingxi.code', platform: 'android' });
  const devices = [device('alice', 'en-US'), device('bob', 'zh-Hans'), device('carol', 'en')];
  const pushStore = { async active(uid) { return devices.filter(item => item.uid === uid); }, device(id) { return { async get() { return { data: () => devices.find(item => item.id === id) }; } }; } };
  const input = (messageId, original, extra = {}) => ({ db, pushStore, messaging: { async send(value) { sent.push(value); } }, teamId, conversationId: PAIR_CONVERSATION_ID, messageId, original, clock: () => now, ...extra });
  const post = async (actor, id) => {
    await messages.execute(actor, { action: 'mutate', ...scope, messageId: id, changeId: `c-${id}`, operation: 'create', text: `Private text of ${id}` });
    return (await db.doc(`teams/${teamId}/conversations/${PAIR_CONVERSATION_ID}/messages/${id}`).get()).data();
  };
  await onTeamMessageCreated(input('message01', await post(alice, 'message01')));
  assert.equal(sent.length, 1);
  assert.deepEqual([sent[0].data.recipientUid, sent[0].data.route, sent[0].data.kind, sent[0].data.title, sent[0].data.body],
    ['bob', 'friend/chat?uid=alice', 'friend_message', 'Alice', '好友有新消息']);
  assert.ok(!JSON.stringify(sent).includes('Private text'));
  await onTeamMessageCreated(input('message02', await post(bob, 'message02')));
  assert.deepEqual([sent[1].data.recipientUid, sent[1].data.route, sent[1].data.title, sent[1].data.body], ['alice', 'friend/chat?uid=bob', 'Bob', 'New message from a friend']);
  assert.equal(sent.length, 2);
  // An ordinary team message is unchanged.
  await teams.execute(alice, { action: 'create', teamId: 'plain-team', name: 'Plain' });
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'plain-team', requestId: 'invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(bob, { action: 'join', code, requestId: 'join-bob' });
  await messages.execute(alice, { action: 'create', teamId: 'plain-team', generation: 1, conversationId: 'plain-chat', kind: 'dm', title: 'Plain', participantUids: ['alice', 'bob'] });
  await messages.execute(alice, { action: 'mutate', teamId: 'plain-team', generation: 1, conversationId: 'plain-chat', messageId: 'plain-01', changeId: 'c-plain-01', operation: 'create', text: 'Team text' });
  const plain = (await db.doc('teams/plain-team/conversations/plain-chat/messages/plain-01').get()).data();
  await onTeamMessageCreated(input('plain-01', plain, { teamId: 'plain-team', conversationId: 'plain-chat' }));
  assert.deepEqual([sent[2].data.recipientUid, sent[2].data.route, sent[2].data.kind, sent[2].data.body], ['bob', 'team/inbox', 'team_message', '团队有新消息']);
  // After the friendship ends nothing is delivered for it.
  const late = await post(alice, 'message03');
  await act(bob, { action: 'remove', otherUid: 'alice' });
  await onTeamMessageCreated(input('message03', late));
  assert.equal(sent.length, 3);
});

test('push: requests and new friends notify through the real notifier — generic text, friend route — and a failed push never fails the command', options, async () => {
  const sent = [], expiry = { toMillis: () => now + 3600000 };
  const device = (uid, locale) => ({ id: `device-${uid}`, uid, revision: 1, fcmInstallationId: `fid-${uid}`, expiresAt: expiry, locale, appId: 'com.lingxi.code', platform: 'ios' });
  const devices = [device('alice', 'en-US'), device('bob', 'zh-CN')];
  const pushStore = { async active(uid) { return devices.filter(item => item.uid === uid); }, device(id) { return { async get() { return { data: () => devices.find(item => item.id === id) }; } }; } };
  let broken = false;
  const live = friendNotifier({ db, pushStore, messaging: { async send(value) { if (broken) throw new Error('provider down'); sent.push(value); } }, clock: () => now });
  friends = new FriendsStore(db, () => now, { auth, notify: live });
  await friends.profile(alice); await friends.profile(bob);
  await request(bob, alice, { note: 'secret hello' });
  assert.equal(sent.length, 1);
  assert.deepEqual([sent[0].data.recipientUid, sent[0].data.route, sent[0].data.kind, sent[0].data.title, sent[0].data.body],
    ['alice', 'friend/requests', 'friend_request', 'Bob', 'wants to add you as a friend']);
  assert.ok(!JSON.stringify(sent).includes('secret hello'));
  await act(alice, { action: 'accept', otherUid: 'bob' });
  assert.deepEqual([sent[1].data.recipientUid, sent[1].data.route, sent[1].data.kind, sent[1].data.title, sent[1].data.body],
    ['bob', 'friend/requests', 'friend_accepted', 'Alice', '已接受你的好友请求']);
  await act(alice, { action: 'remove', otherUid: 'bob' });
  const code = await link(alice);
  await act(bob, { action: 'redeem', code, requestId: rid() });
  assert.deepEqual([sent[2].data.recipientUid, sent[2].data.kind, sent[2].data.body], ['alice', 'friend_linked', 'is now your friend']);
  // A cancelled request is not announced by a late push, and a broken provider or notifier fails nothing.
  await act(alice, { action: 'remove', otherUid: 'bob' });
  broken = true;
  assert.equal((await request(bob, alice)).state, 'outgoing');
  assert.equal(await consistent(alice, bob), 'pending');
  await act(bob, { action: 'cancel', otherUid: 'alice' });
  broken = false;
  await live({ kind: 'request', to: 'alice', from: 'bob', name: 'Bob', key: 'late' });
  assert.equal(sent.length, 3);
  friends = new FriendsStore(db, () => now, { auth, notify: async () => { throw new Error('notifier bug'); } });
  assert.equal((await request(bob, alice)).state, 'outgoing');
  assert.equal((await act(alice, { action: 'accept', otherUid: 'bob' })).state, 'friends');
  assert.equal(await consistent(alice, bob), 'friends');
  friends = new FriendsStore(db, () => now); // no notifier configured
  await act(alice, { action: 'remove', otherUid: 'bob' });
  assert.equal((await act(bob, { action: 'redeem', code, requestId: rid() })).state, 'friends');
});

test('HTTP: the Friends routes reuse the Teams handler\'s authentication and error shape end to end', options, async () => {
  const users = { 'token-alice': alice, 'token-bob': bob };
  const handler = createTeamsHandler({ store: teams, friends, origins: ['https://lingxios.net'], createCustomToken: async () => '',
    verifyToken: async token => users[token] ?? Promise.reject(new Error('invalid')) });
  const call = (token, path, method, body, origin = 'https://lingxios.net') => handler(new Request(`https://test.invalid${path}`,
    { method, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), Origin: origin }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) }));
  const profile = await call('token-alice', '/v1/friends/profile', 'GET');
  assert.equal(profile.status, 200);
  assert.equal(profile.headers.get('Cache-Control'), 'no-store');
  assert.equal(profile.headers.get('Access-Control-Allow-Origin'), 'https://lingxios.net');
  const mine = await profile.json();
  assert.deepEqual([mine.uid, mine.displayName, mine.friendCode.link.startsWith('https://lingxios.net/open/friend/add?code=')], ['alice', 'Alice', true]);
  assert.equal((await call(null, '/v1/friends/profile', 'GET')).status, 401);
  assert.equal((await call('token-alice', '/v1/friends/profile', 'GET', undefined, 'https://evil.invalid')).status, 403);
  const cardResponse = await call('token-bob', '/v1/friends', 'POST', { action: 'lookup', code: mine.friendCode.code });
  assert.equal(cardResponse.status, 200);
  assert.deepEqual(await cardResponse.json(), { uid: 'alice', displayName: 'Alice', relation: 'none' });
  const miss = await call('token-bob', '/v1/friends', 'POST', { action: 'lookup', handle: 'nobody_here' });
  assert.deepEqual([miss.status, await miss.json()], [404, { error: { code: 'NOT_FOUND' } }]);
  const strict = await call('token-bob', '/v1/friends', 'POST', { action: 'lookup', code: mine.friendCode.code, uid: 'alice' });
  assert.deepEqual([strict.status, await strict.json()], [400, { error: { code: 'INVALID_INPUT' } }]);
  const joined = await call('token-bob', '/v1/friends', 'POST', { action: 'redeem', code: mine.friendCode.code, requestId: rid() });
  assert.deepEqual([joined.status, (await joined.json()).teamId], [200, pairTeamId('alice', 'bob', 1)]);
  const taken = await call('token-bob', '/v1/friends', 'POST', { action: 'profile', handle: 'Shared_Name' });
  assert.equal(taken.status, 200);
  const clash = await call('token-alice', '/v1/friends', 'POST', { action: 'profile', handle: 'shared_name' });
  assert.deepEqual([clash.status, await clash.json()], [409, { error: { code: 'HANDLE_TAKEN' } }]);
  for (let i = 0; i < LOOKUP_LIMIT; i++) await call('token-bob', '/v1/friends', 'POST', { action: 'lookup', handle: `missing_${i}` });
  const limited = await call('token-bob', '/v1/friends', 'POST', { action: 'lookup', handle: 'missing_over' });
  assert.deepEqual([limited.status, await limited.json()], [429, { error: { code: 'RATE_LIMITED' } }]);
  assert.equal((await call('token-bob', '/v1/friends/profile', 'POST', {})).status, 404);
  // Teams routes keep working beside them.
  const list = await call('token-alice', '/v1/teams', 'GET');
  assert.deepEqual(await list.json(), { teams: [] });
});
