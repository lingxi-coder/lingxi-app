import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails, assertSucceeds } from '@firebase/rules-unit-testing';
import { doc, getDoc, setDoc, getDocs, collection } from 'firebase/firestore';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

const projectId = 'demo-lingxi-teams-t0', options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
let app, db, environment, store, now;
const owner = { uid: 'owner', name: 'Owner' }, alice = { uid: 'alice', name: 'Alice' };
const teamId = 'members-team';
const create = () => store.execute(owner, { action: 'create', teamId, name: 'Fixture team' });
const invite = (overrides = {}, actor = owner) => store.execute(actor, { action: 'invite', teamId,
  requestId: 'request01', role: 'member', maxUses: 10, ttlSeconds: 3600, ...overrides });
const join = (code, user = alice, requestId = randomUUID()) => store.execute(user, { action: 'join', code, requestId });
const client = uid => environment.authenticatedContext(uid).firestore();
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'teams-member-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore(); now = Date.now(); store = new TeamsStore(db, () => now);
});
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('T1: team creation is idempotent, indexed and cannot overwrite another owner', options, async () => {
  await create(); await create();
  assert.deepEqual((await store.list(owner.uid)).teams, [{ id: teamId, name: 'Fixture team', memberCount: 1, role: 'owner', generation: 1 }]);
  await assert.rejects(store.execute(alice, { action: 'create', teamId, name: 'Fixture team' }), { code: 'TEAM_ID_CONFLICT' });
  assert.equal((await store.detail(owner.uid, teamId)).members.length, 1);
});
test('T1: concurrent joins honor the final invite use and retry consumes no second use', options, async () => {
  await create(); const { code } = await invite({ maxUses: 1 });
  const results = await Promise.allSettled([join(code), join(code, { uid: 'bob' }), join(code, { uid: 'carol' })]);
  assert.equal(results.filter(result => result.status === 'fulfilled').length, 1);
  const detail = await store.detail(owner.uid, teamId), joined = detail.members.find(member => member.uid !== owner.uid);
  assert.equal(detail.team.memberCount, 2); assert.equal(detail.invites[0].uses, 1);
  assert.equal((await join(code, { uid: joined.uid })).alreadyMember, true);
  assert.equal((await store.detail(owner.uid, teamId)).invites[0].uses, 1);
});
test('T1: expired and revoked invitations cannot create membership', options, async () => {
  await create(); const { code } = await invite(); now += 3600001;
  await assert.rejects(join(code), { code: 'INVITE_UNAVAILABLE' });
  now -= 3600001; const detail = await store.detail(owner.uid, teamId);
  await store.execute(owner, { action: 'revokeInvite', teamId, inviteId: detail.invites[0].id });
  await assert.rejects(join(code), { code: 'INVITE_UNAVAILABLE' });
  assert.equal((await store.list(alice.uid)).teams.length, 0);
});
test('T1: removal denies the very next SDK read, rejoin increments the durable epoch', options, async () => {
  await create(); const { code } = await invite(); await join(code, alice, 'join-req-01');
  await assertSucceeds(getDoc(doc(client(alice.uid), `teams/${teamId}`)));
  await store.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await assertFails(getDoc(doc(client(alice.uid), `teams/${teamId}`)));
  await assertFails(getDocs(collection(client(alice.uid), `teams/${teamId}/members`)));
  await assert.rejects(store.detail(alice.uid, teamId), { code: 'MEMBERSHIP_REQUIRED' });
  assert.deepEqual((await store.list(alice.uid)).teams, []);
  await assert.rejects(join(code, alice, 'join-req-01'), { code: 'STALE_MEMBERSHIP' });
  assert.equal((await join(code)).generation, 2);
  await assert.rejects(join(code, alice, 'join-req-01'), { code: 'STALE_MEMBERSHIP' });
  assert.equal((await db.doc(`teams/${teamId}/membershipEpochs/alice`).get()).data().generation, 2);
  await assertSucceeds(getDoc(doc(client(alice.uid), `teams/${teamId}`)));
  await assert.rejects(store.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 }), { code: 'STALE_MEMBERSHIP' });
  await assert.rejects(store.execute(owner, { action: 'role', teamId, uid: alice.uid, generation: 1, role: 'admin' }), { code: 'STALE_MEMBERSHIP' });
});
test('T1: role hierarchy prevents escalation and protects the owner', options, async () => {
  await create(); const { code } = await invite(); await join(code);
  await assert.rejects(invite({ requestId: 'request02' }, alice), { code: 'ROLE_REQUIRED' });
  await store.execute(owner, { action: 'role', teamId, uid: alice.uid, generation: 1, role: 'admin' });
  assert.equal((await store.list(alice.uid)).teams[0].role, 'admin');
  await assert.rejects(invite({ role: 'admin' }, alice), { code: 'ROLE_REQUIRED' });
  await assert.rejects(store.execute(alice, { action: 'role', teamId, uid: alice.uid, generation: 1, role: 'admin' }), { code: 'ROLE_REQUIRED' });
  await assert.rejects(store.execute(alice, { action: 'remove', teamId, uid: owner.uid, generation: 1 }), { code: 'ROLE_REQUIRED' });
  await assert.rejects(store.execute(owner, { action: 'leave', teamId, generation: 1 }), { code: 'OWNER_CANNOT_LEAVE' });
  await assert.rejects(store.execute(alice, { action: 'delete', teamId }), { code: 'OWNER_REQUIRED' });
});
test('T1: leave preserves the epoch, and stale leave cannot remove a rejoin', options, async () => {
  await create(); const { code } = await invite(); await join(code);
  await store.execute(alice, { action: 'leave', teamId, generation: 1 });
  assert.equal((await join(code)).generation, 2);
  await assert.rejects(store.execute(alice, { action: 'leave', teamId, generation: 1 }), { code: 'STALE_MEMBERSHIP' });
});
test('T1: team deletion atomically removes all indexes and blocks old invites and nested reads', options, async () => {
  await create(); const { code } = await invite(); await join(code);
  await db.doc(`teams/${teamId}/conversations/old-chat`).set({ memberUids: ['alice'] });
  await store.execute(owner, { action: 'delete', teamId });
  assert.deepEqual((await store.list(owner.uid)).teams, []);
  assert.deepEqual((await store.list(alice.uid)).teams, []);
  await assert.rejects(join(code), { code: 'INVITE_UNAVAILABLE' });
  await assertFails(getDoc(doc(client('alice'), `teams/${teamId}/conversations/old-chat`)));
  assert.equal((await db.doc(`teams/${teamId}/membershipEpochs/alice`).get()).data().generation, 1);
});
test('T1: invite creation retries return the same link, with conflicts rejected', options, async () => {
  await create(); const first = await invite(); assert.deepEqual(await invite(), first);
  assert.equal((await store.detail(owner.uid, teamId)).invites.length, 1);
  await assert.rejects(invite({ maxUses: 11 }), { code: 'REQUEST_ID_CONFLICT' });
});
test('T1: a concurrent delete and join cannot leave active members or stale team indexes', options, async () => {
  await create(); const { code } = await invite();
  const [deleted] = await Promise.allSettled([store.execute(owner, { action: 'delete', teamId }), join(code)]);
  assert.equal(deleted.status, 'fulfilled');
  assert.equal((await db.doc(`teams/${teamId}`).get()).data().state, 'deleted');
  assert.deepEqual((await store.list(owner.uid)).teams, []);
  assert.deepEqual((await store.list(alice.uid)).teams, []);
  assert.equal((await db.collection(`teams/${teamId}/members`).get()).size, 0);
  await assert.rejects(join(code), { code: 'INVITE_UNAVAILABLE' });
});
test('T1: clients cannot forge membership, epochs, invites, indexes or team state', options, async () => {
  await create();
  for (const path of [`teams/${teamId}`, `teams/${teamId}/members/alice`, `teams/${teamId}/membershipEpochs/alice`,
    `teams/${teamId}/invites/forged`, `users/alice/teams/${teamId}`, 'teamInvites/forged']) {
    await assertFails(setDoc(doc(client('owner'), path), { role: 'owner', generation: 1, state: 'active' }));
  }
  await assertFails(getDoc(doc(client('owner'), `teams/${teamId}/membershipEpochs/alice`)));
  await assertFails(getDoc(doc(client('owner'), `teams/${teamId}/invites/forged`)));
});
test('T1: HTTP authentication, strict inputs, CORS and custom-token UID are enforced', options, async () => {
  const tokens = [];
  const handle = createTeamsHandler({ store, origins: ['https://lingxios.net'],
    verifyToken: async token => { if (token !== 'valid') throw new Error(); return owner; },
    createCustomToken: async uid => { tokens.push(uid); return 'synthetic-token'; } });
  const request = (body, { token = 'valid', origin = 'https://lingxios.net', path = '/v1/teams' } = {}) => handle(new Request(`https://test.invalid${path}`,
    { method: 'POST', headers: { Authorization: `Bearer ${token}`, Origin: origin }, body: JSON.stringify(body) }));
  assert.equal((await request({}, { token: 'bad' })).status, 401);
  assert.equal((await request({}, { origin: 'https://evil.invalid' })).status, 403);
  assert.equal((await request({ action: 'join', code: 'a'.repeat(32), uid: 'owner' })).status, 400);
  assert.equal((await request({ uid: 'other' }, { path: '/v1/teams/session' })).status, 400);
  const session = await request({}, { path: '/v1/teams/session' });
  assert.equal(session.status, 200); assert.deepEqual(await session.json(), { uid: 'owner', customToken: 'synthetic-token' });
  assert.deepEqual(tokens, ['owner']); assert.equal(session.headers.get('Cache-Control'), 'no-store');
  assert.equal((await request({ action: 'create', teamId, name: 'Fixture team' })).status, 200);
  assert.equal((await handle(new Request(`https://test.invalid/v1/teams/${teamId}`, { headers: { Authorization: 'Bearer valid' } }))).status, 200);
});
