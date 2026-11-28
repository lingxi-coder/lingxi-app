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
test('M3-3: the owner hands the team to a current member and stays as an admin; only an owner can, and asking again changes nothing', options, async () => {
  const bob = { uid: 'bob', name: 'Bob' }, roles = async () => Object.fromEntries((await store.detail(owner.uid, teamId)).members.map(member => [member.uid, member.role]));
  await create(); const { code } = await invite(); await join(code); await join(code, bob);
  await assert.rejects(store.execute(alice, { action: 'handOver', teamId, uid: bob.uid, generation: 1 }), { code: 'OWNER_REQUIRED' });
  await assert.rejects(store.execute(owner, { action: 'handOver', teamId, uid: owner.uid, generation: 1 }), { code: 'ROLE_REQUIRED' });
  await assert.rejects(store.execute(owner, { action: 'handOver', teamId, uid: alice.uid, generation: 2 }), { code: 'STALE_MEMBERSHIP' });
  await assert.rejects(store.execute(owner, { action: 'handOver', teamId, uid: 'stranger', generation: 1 }), { code: 'STALE_MEMBERSHIP' });
  assert.deepEqual(await roles(), { owner: 'owner', alice: 'member', bob: 'member' });
  assert.deepEqual(await store.execute(owner, { action: 'handOver', teamId, uid: alice.uid, generation: 1 }), { ok: true });
  assert.deepEqual(await roles(), { owner: 'admin', alice: 'owner', bob: 'member' });
  assert.deepEqual([(await store.list(alice.uid)).teams[0].role, (await store.list(owner.uid)).teams[0].role], ['owner', 'admin'], 'each person’s own list agrees');
  // Its answer was lost: the earlier owner asks again and nothing moves; nobody takes the team back that way.
  assert.deepEqual(await store.execute(owner, { action: 'handOver', teamId, uid: alice.uid, generation: 1 }), { ok: true });
  await assert.rejects(store.execute(owner, { action: 'handOver', teamId, uid: bob.uid, generation: 1 }), { code: 'OWNER_REQUIRED' });
  assert.deepEqual(await roles(), { owner: 'admin', alice: 'owner', bob: 'member' });
  // The new owner has the owner's rights and duties, and the earlier one an admin's.
  await assert.rejects(store.execute(owner, { action: 'delete', teamId }), { code: 'OWNER_REQUIRED' });
  await assert.rejects(store.execute(alice, { action: 'leave', teamId, generation: 1 }), { code: 'OWNER_CANNOT_LEAVE' });
  await store.execute(alice, { action: 'role', teamId, uid: bob.uid, generation: 1, role: 'admin' });
  await store.execute(owner, { action: 'leave', teamId, generation: 1 });
  assert.deepEqual(await store.execute(alice, { action: 'handOver', teamId, uid: bob.uid, generation: 1 }), { ok: true });
  assert.deepEqual(Object.fromEntries((await store.detail(bob.uid, teamId)).members.map(member => [member.uid, member.role])), { alice: 'admin', bob: 'owner' });
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

test('team profiles are private, versioned, and shared with every current member', options, async () => {
  const admin = { uid: 'admin' };
  await create();
  const { code } = await invite();
  await join(code, alice);
  await join(code, admin);
  await store.execute(owner, { action: 'role', teamId, uid: admin.uid, generation: 1, role: 'admin' });
  assert.deepEqual((await store.detail(alice.uid, teamId)).profile, { avatar: null, description: '', version: 0 });
  const handle = createTeamsHandler({ store, verifyToken: async uid => ({ uid }) });
  const path = `/v1/teams/${teamId}/profile`;
  const request = (actor, version, changes, extra = {}) => handle(new Request(`https://test.invalid${path}`, {
    method: 'POST', headers: { Authorization: `Bearer ${actor.uid}` },
    body: JSON.stringify({ action: 'updateProfile', teamId, generation: 1, version, changes, ...extra }),
  }));
  assert.equal((await request(alice, 0, { name: 'Forbidden' })).status, 403);
  assert.equal((await request({ uid: 'outsider' }, 0, { description: 'Forbidden' })).status, 403);
  assert.equal((await request(owner, 0, { name: 'Wrong team' }, { teamId: 'other-team' })).status, 400);
  assert.equal((await request(owner, 0, { description: 'Oversized'.repeat(12000) })).status, 413);
  assert.equal((await request(owner, 0, { avatar: 'dGV4dA==' })).status, 400);
  const concurrent = await Promise.all([request(owner, 0, { name: 'New name' }), request(admin, 0, { description: 'New description' })]);
  assert.deepEqual(concurrent.map(response => response.status).sort(), [200, 409]);
  const conflict = concurrent.find(response => response.status === 409);
  assert.equal((await conflict.json()).error.code, 'TEAM_PROFILE_CHANGED');

  // An actual 96 × 96 PNG.
  const avatar = 'iVBORw0KGgoAAAANSUhEUgAAAGAAAABgCAIAAABt+uBvAAAAMklEQVR4nO3BMQEAAADCoPVPbQ0PoAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgBcDbGAAAVwixpAAAAAASUVORK5CYII=';
  const update = { name: ' Studio ', description: ' Shared workshop ', avatar };
  assert.equal((await request(admin, 1, update)).status, 200);
  assert.equal((await request(admin, 1, update)).status, 200, 'a lost success response can be retried');
  assert.deepEqual((await store.detail(alice.uid, teamId)).profile, { avatar, description: 'Shared workshop', version: 2 });
  assert.equal((await store.list(alice.uid)).teams[0].name, 'Studio');
  const index = await getDoc(doc(client(alice.uid), `users/${alice.uid}/teams/${teamId}`));
  assert.equal(index.data().profileVersion, 2, 'existing team index listeners receive a change');
  await assertFails(getDoc(doc(client(alice.uid), `teams/${teamId}/profile/current`)));
  await create(); // The idempotent initial name is still recognized after rename.
  assert.equal((await request(owner, 2, { avatar: null, description: '' })).status, 200);
  assert.deepEqual((await store.detail(alice.uid, teamId)).profile, { avatar: null, description: '', version: 3 });
  await store.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await assert.rejects(store.detail(alice.uid, teamId), { code: 'MEMBERSHIP_REQUIRED' });
  await store.execute(owner, { action: 'delete', teamId });
  assert.equal((await db.doc(`teams/${teamId}/profile/current`).get()).exists, false);
});

test('team removal requires every published Bot to be paused or handed over, including offline and restricted Bots', options, async () => {
  const admin = { uid: 'admin' }, bob = { uid: 'bob' };
  await create();
  const { code } = await invite();
  await join(code, alice);
  await join(code, admin);
  await join(code, bob);
  await store.execute(owner, { action: 'role', teamId, uid: admin.uid, generation: 1, role: 'admin' });
  const target = { teamId, uid: alice.uid, generation: 1 };
  const stop = (botId, revision, actor = admin) => store.execute(actor, { action: 'pauseRemovalBot', ...target, actorGeneration: 1, botId, revision });
  const blockers = () => store.execute(admin, { action: 'removalBlockers', ...target });
  const remove = () => store.execute(admin, { action: 'remove', ...target });
  assert.deepEqual(await blockers(), { blockingBots: [] });
  const teamRef = db.doc(`teams/${teamId}`);
  const privateBot = teamRef.collection('sharedBots').doc('private-bot');
  const otherBot = teamRef.collection('sharedBots').doc('second-bot');
  const identity = { maintainer: { uid: alice.uid, generation: 1 }, state: 'published', revision: 1, usePolicy: { mode: 'listed', members: [] } };
  await privateBot.set({ name: 'Private Bot', identity, transfer: { transferId: 'first-transfer' } });
  await teamRef.collection('transfers').doc('first-transfer').set({ state: 'offered' });
  const transferPart = privateBot.collection('transferParts').doc('first-part');
  await transferPart.set({ data: 'package part' });
  await otherBot.set({ name: 'Second Bot', identity });
  await teamRef.collection('sharedBots').doc('previous-membership').set({ name: 'Old Bot', identity: { ...identity, maintainer: { uid: alice.uid, generation: 0 } } });

  assert.deepEqual((await blockers()).blockingBots, [
    { id: privateBot.id, name: 'Private Bot', revision: 1 }, { id: otherBot.id, name: 'Second Bot', revision: 1 },
  ]);
  await assert.rejects(remove(), { code: 'TEAM_MEMBER_HAS_BOTS' });
  await assert.rejects(stop(privateBot.id, 1, bob), { code: 'ROLE_REQUIRED' });
  await assert.rejects(store.execute(admin, { action: 'pauseRemovalBot', ...target, uid: owner.uid,
    actorGeneration: 1, botId: privateBot.id, revision: 1 }), { code: 'ROLE_REQUIRED' });
  await assert.rejects(stop(privateBot.id, 2), { code: 'BOT_REVISION_CONFLICT' });
  await stop(privateBot.id, 1);
  await stop(privateBot.id, 1);
  assert.equal((await privateBot.get()).data().identity.state, 'paused');
  assert.equal((await teamRef.collection('transfers').doc('first-transfer').get()).data().state, 'cancelled');
  assert.equal((await transferPart.get()).exists, false);
  // A re-publish during removal makes the old confirmation insufficient.
  await privateBot.update({ 'identity.state': 'published', 'identity.revision': 3 });
  await assert.rejects(remove(), { code: 'TEAM_MEMBER_HAS_BOTS' });
  await stop(privateBot.id, 3);
  assert.equal((await blockers()).blockingBots.length, 1);
  await otherBot.update({ 'identity.maintainer': { uid: bob.uid, generation: 1 }, 'identity.revision': 2 });
  await assert.rejects(stop(otherBot.id, 1), { code: 'BOT_REVISION_CONFLICT' });
  assert.deepEqual(await blockers(), { blockingBots: [] });
  assert.deepEqual(await remove(), { ok: true });
  assert.equal((await otherBot.get()).data().identity.state, 'published');
  assert.equal((await store.detail(admin.uid, teamId)).members.some(member => member.uid === alice.uid), false);
});
