import assert from 'node:assert/strict';
import { after, afterEach, before, beforeEach, test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { TeamMarketEligibilityStore, createTeamMarketEligibilityHandler } from '../src/team-market-eligibility.mjs';

// Dedicated demo project and unique fixture records: never clear another suite's data.
const projectId = 'demo-lingxi-market-eligibility';
const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
let app, db, teams, eligibility, teamId, owner, member, outsider, inviteCode;
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'market-eligibility-emulator-tests');
  db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  const suffix = randomUUID().replaceAll('-', '');
  teamId = `market_${suffix}`;
  owner = { uid: `owner_${suffix}`, name: 'Fixture owner' };
  member = { uid: `member_${suffix}`, name: 'Fixture member' };
  outsider = { uid: `outsider_${suffix}`, name: 'Fixture outsider' };
  teams = new TeamsStore(db);
  eligibility = new TeamMarketEligibilityStore(db);
  await teams.execute(owner, { action: 'create', teamId, name: 'Market eligibility fixture' });
  const invite = await teams.execute(owner, { action: 'invite', teamId, requestId: randomUUID(), role: 'member', maxUses: 10, ttlSeconds: 3600 });
  inviteCode = invite.code;
  await teams.execute(member, { action: 'join', code: inviteCode, requestId: randomUUID() });
});
afterEach(async () => {
  if (!db || !teamId) return;
  // Delete only this test's randomly named fixtures, including index/receipt records.
  await db.recursiveDelete(db.doc(`teams/${teamId}`));
  for (const actor of [owner, member, outsider]) await db.recursiveDelete(db.doc(`users/${actor.uid}`));
  const locators = await db.collection('teamInvites').where('teamId', '==', teamId).get();
  await Promise.all(locators.docs.map(doc => doc.ref.delete()));
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });
const lookup = (actor = member, operation = 'read') => eligibility.eligibility(teamId, actor.uid, operation);
function denied(value) {
  assert.equal(value.schemaVersion, 1); assert.equal(value.eligible, false); assert.equal(value.memberGeneration, null);
  assert.ok(Number.isSafeInteger(value.policyRevision) && value.policyRevision >= 0);
}

test('market eligibility emulator: authoritative membership permits current members and denies an outsider despite a forged user index', options, async () => {
  const first = await lookup();
  assert.equal(first.eligible, true); assert.equal(first.memberGeneration, 1); assert.ok(first.policyRevision > 0);
  assert.deepEqual(await lookup(), first, 'an unchanged authorization snapshot retains its revision');
  assert.equal((await lookup(owner, 'publish')).eligible, true);
  await db.doc(`users/${outsider.uid}/teams/${teamId}`).set({ role: 'owner', generation: 99 });
  denied(await lookup(outsider)); denied(await lookup(outsider, 'publish'));
});

test('market eligibility emulator: publish permission follows actual owner/admin role changes, not membership alone', options, async () => {
  const original = await lookup(); denied(await lookup(member, 'publish'));
  await teams.execute(owner, { action: 'role', teamId, uid: member.uid, generation: 1, role: 'admin' });
  const promoted = await lookup(member, 'publish');
  assert.equal(promoted.eligible, true); assert.equal(promoted.memberGeneration, 1);
  assert.ok(promoted.policyRevision > original.policyRevision, 'role commit invalidates the old policy revision');
  await teams.execute(owner, { action: 'role', teamId, uid: member.uid, generation: 1, role: 'member' });
  const demoted = await lookup(member, 'publish'); denied(demoted);
  assert.ok(demoted.policyRevision > promoted.policyRevision); assert.equal((await lookup()).eligible, true);
});

test('market eligibility emulator: removal immediately denies, durable epoch is not membership, and rejoin returns the new generation', options, async () => {
  const joined = await lookup();
  await teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 });
  const removed = await lookup(); denied(removed);
  assert.ok(removed.policyRevision > joined.policyRevision);
  assert.equal((await db.doc(`teams/${teamId}/membershipEpochs/${member.uid}`).get()).data().generation, 1);
  denied(await lookup(member, 'publish'));
  await teams.execute(member, { action: 'join', code: inviteCode, requestId: randomUUID() });
  const rejoined = await lookup(); assert.equal(rejoined.eligible, true); assert.equal(rejoined.memberGeneration, 2);
  assert.ok(rejoined.policyRevision > removed.policyRevision);
  await assert.rejects(teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 }), { code: 'STALE_MEMBERSHIP' });
  assert.deepEqual(await lookup(), rejoined);
});

test('market eligibility emulator: leave is visible on the next lookup and team deactivation overrides residual member records', options, async () => {
  await teams.execute(member, { action: 'leave', teamId, generation: 1 }); denied(await lookup());
  const before = await lookup(owner);
  await db.doc(`teams/${teamId}`).update({ state: 'suspended' });
  const inactive = await lookup(owner); denied(inactive); denied(await lookup(owner, 'publish'));
  assert.ok(inactive.policyRevision > before.policyRevision);
  assert.equal((await db.doc(`teams/${teamId}/members/${owner.uid}`).get()).exists, true, 'the inactive-team check does not rely on removing its owner row');
});

test('market eligibility emulator: real Team deletion denies all operations and preserves the revocation revision', options, async () => {
  const before = await lookup();
  await teams.execute(owner, { action: 'delete', teamId });
  const deleted = await lookup(); denied(deleted); denied(await lookup(owner, 'publish'));
  assert.ok(deleted.policyRevision > before.policyRevision);
  assert.equal((await db.doc(`teams/${teamId}`).get()).data().state, 'deleted');
  assert.deepEqual(await lookup(), deleted);
});

test('market eligibility emulator: absent, zero, fractional and unsafe generations cannot authorize malformed membership', options, async () => {
  const ref = db.doc(`teams/${teamId}/members/${member.uid}`);
  for (const generation of [undefined, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    await ref.set({ role: 'admin', ...(generation === undefined ? {} : { generation }) });
    denied(await lookup()); denied(await lookup(member, 'publish'));
  }
});

test('market eligibility emulator: authenticated handler forwards read/publish into the real store and returns current revocation without private Team data', options, async () => {
  const config = { serviceEmail: 'market-api-runtime@example.iam.gserviceaccount.com', serviceSubject: '100000001',
    serviceAudience: 'https://market-eligibility.example.run.app', projectId, tenantId: null };
  const handler = createTeamMarketEligibilityHandler({ store: eligibility, config: () => config,
    // OIDC cryptographic acceptance belongs to the separate real-service gate;
    // this test exercises handler claims and operation dispatch with real Firestore.
    verifyServiceToken: async token => { assert.equal(token, 'fixture-service-token'); return { iss: 'https://accounts.google.com', aud: config.serviceAudience,
      sub: config.serviceSubject, email: config.serviceEmail, email_verified: true }; } });
  const request = async operation => {
    const response = await handler(new Request('https://fixture.invalid/internal/v1/market/eligibility', { method: 'POST',
      headers: { Authorization: 'Bearer fixture-service-token', 'Content-Type': 'application/json' }, body: JSON.stringify({ schemaVersion: 1,
        principal: { uid: member.uid, issuer: `https://securetoken.google.com/${projectId}`, audience: projectId, tenantId: null },
        teamId, operation, requestDigest: 'a'.repeat(64) }) }));
    assert.equal(response.status, 200); assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.equal(response.headers.has('Access-Control-Allow-Origin'), false);
    const value = await response.json(); assert.deepEqual(Object.keys(value).sort(), ['eligible', 'memberGeneration', 'policyRevision', 'schemaVersion']);
    return value;
  };
  assert.equal((await request('read')).eligible, true); denied(await request('publish'));
  await teams.execute(owner, { action: 'role', teamId, uid: member.uid, generation: 1, role: 'admin' });
  assert.equal((await request('publish')).eligible, true);
  await teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 });
  denied(await request('read')); denied(await request('publish'));
});
