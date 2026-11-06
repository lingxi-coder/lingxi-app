import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { planRetiredTeamCleanup, applyRetiredTeamCleanup } from '../src/retired-teams-data.mjs';
const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 30000 };
const projectId = 'demo-teams-retired-cleanup', teamId = 'team_cleanup', oldKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
let app, db;
before(() => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'retired-teams-cleanup'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  await db.recursiveDelete(db.collection('teams'));
  const batch = db.batch();
  for (const [path, value] of [
    [`teams/${teamId}`, { state: 'active', sentinel: 'preserve-team' }],
    [`teams/${teamId}/members/owner`, { generation: 1, sentinel: 'preserve-member' }],
    [`teams/${teamId}/bots/${oldKey}`, { ownerUid: 'owner' }],
    [`teams/${teamId}/conversations/old_chat`, { participants: ['user:member', oldKey] }],
    [`teams/${teamId}/conversations/old_chat/messages/message01`, { text: 'obsolete' }],
    [`teams/${teamId}/conversations/old_chat/botRuns/obsolete01`, { state: 'running' }],
    [`teams/${teamId}/conversations/human_chat`, { participants: ['user:owner', 'user:member'], sentinel: 'preserve-human-chat' }],
    [`teams/${teamId}/conversations/human_chat/messages/message01`, { text: 'preserve-human-message' }],
    [`teams/${teamId}/conversations/human_chat/botCursors/obsolete01`, { seq: 20 }],
    [`teams/${teamId}/sharedBots/sharedbot01`, { sentinel: 'preserve-new-bot' }],
    [`teams/${teamId}/conversations/new_chat`, { participants: ['user:member', 'shared_bot:sharedbot01'], sentinel: 'preserve-new-chat' }],
    [`teams/${teamId}/artifacts/artifact01`, { sentinel: 'preserve-artifact' }],
  ]) batch.set(db.doc(path), value);
  await batch.commit();
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });
test('retired cleanup: dry-run is exact and deletes only known obsolete roots while preserving sentinels', options, async () => {
  const plan = await planRetiredTeamCleanup(db, [teamId]);
  assert.equal(plan.entries.length, 3);
  assert.equal((await db.doc(`teams/${teamId}/conversations/old_chat/messages/message01`).get()).exists, true);
  const result = await applyRetiredTeamCleanup(db, plan, { oldExecutionStopped: true }); assert.equal(result.deletedRoots, 3);
  assert.equal((await db.doc(`teams/${teamId}/conversations/old_chat/messages/message01`).get()).exists, false);
  for (const [suffix, sentinel] of [
    ['', 'preserve-team'], ['/members/owner', 'preserve-member'], ['/conversations/human_chat', 'preserve-human-chat'],
    ['/sharedBots/sharedbot01', 'preserve-new-bot'], ['/conversations/new_chat', 'preserve-new-chat'], ['/artifacts/artifact01', 'preserve-artifact'],
  ]) assert.equal((await db.doc(`teams/${teamId}${suffix}`).get()).data().sentinel, sentinel);
  assert.equal((await db.doc(`teams/${teamId}/conversations/human_chat/messages/message01`).get()).data().text, 'preserve-human-message');
  const repeated = await planRetiredTeamCleanup(db, [teamId]); assert.equal(repeated.entries.length, 0);
  assert.equal((await applyRetiredTeamCleanup(db, repeated, { oldExecutionStopped: true })).deletedRoots, 0);
});
test('retired cleanup: unstopped cutover and changed plan refuse deletion', options, async () => {
  const plan = await planRetiredTeamCleanup(db, [teamId]);
  await assert.rejects(applyRetiredTeamCleanup(db, plan), /CUTOVER_NOT_STOPPED/u);
  await db.doc(`teams/${teamId}/conversations/old_chat`).update({ changed: true });
  await assert.rejects(applyRetiredTeamCleanup(db, plan, { oldExecutionStopped: true }), /CLEANUP_PLAN_CHANGED/u);
  assert.equal((await db.doc(`teams/${teamId}/bots/${oldKey}`).get()).exists, true);
});
test('retired cleanup: malformed old collection records and unspecified Team IDs fail closed', options, async () => {
  await assert.rejects(planRetiredTeamCleanup(db, []), /EXPLICIT_TEAM_IDS_REQUIRED/u);
  await db.doc(`teams/${teamId}/bots/unexpected`).set({ sentinel: 'unknown-format' });
  await assert.rejects(planRetiredTeamCleanup(db, [teamId]), /UNRECOGNIZED_RETIRED_BOT_ID/u);
  assert.equal((await db.doc(`teams/${teamId}/bots/unexpected`).get()).exists, true);
});
