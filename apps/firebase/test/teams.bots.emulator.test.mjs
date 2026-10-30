import assert from 'node:assert/strict';
import { before, beforeEach, after, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails, assertSucceeds } from '@firebase/rules-unit-testing';
import { doc, getDoc, setDoc, writeBatch, serverTimestamp } from 'firebase/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { botDigest, botOutputIdentity, triggerIdentity } from '../src/team-bots.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0', teamId = 'bots-team', conversationId = 'bots-private';
const owner = { uid: 'owner', name: 'Owner' }, alice = { uid: 'alice', name: 'Alice' }, bob = { uid: 'bob', name: 'Bob' };
const hostId = 'h'.repeat(22), botId = 'b'.repeat(22), botKey = `bot:owner:${hostId}:${botId}`;
const scope = { teamId, generation: 1 }, thread = { ...scope, conversationId };
const bind = { action: 'bindBot', ...scope, hostId, botId, expectedRevision: 0, name: 'Synthetic Bot', description: 'Test only', use: 'listed', uids: ['alice'] };
const send = (more = {}) => ({ action: 'mutate', ...thread, operation: 'create', messageId: 'human-msg01', changeId: 'human-change01', text: 'Synthetic request', ...more });
const run = (action, more = {}) => ({ action, ...thread, botKey, triggerId: triggerIdentity('human-msg01', botKey), attemptId: 'attempt-001', ...more });
let app, db, env, messages, teams, now;
const execute = (command, user = owner) => messages.execute(user, command);
const root = () => db.doc(`teams/${teamId}/conversations/${conversationId}`);
const sync = () => execute({ action: 'sync', ...thread, after: 0 }, alice);
const countMessages = async () => (await root().collection('messages').get()).size;
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  env = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port), rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'teams-bot-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!env) return;
  await env.clearFirestore(); now = Date.now(); teams = new TeamsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Synthetic Bot team' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite001', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const user of [alice, bob]) await teams.execute(user, { action: 'join', code, requestId: `join-${user.uid}` });
  await execute(bind);
  await execute({ action: 'create', ...thread, kind: 'bot_dm', title: 'Alice and Bot', participantUids: ['alice'], botKeys: [botKey] }, alice);
});
after(async () => { await env?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('T3 protocol: cards use trusted revisions, current owners and an explicit allowlist', options, async () => {
  const [card] = (await execute({ action: 'bots', ...scope }, alice)).bots;
  assert.equal(card.permitted, true); assert.equal(card.online, true); assert.equal(card.revision, 1);
  assert.equal((await execute({ action: 'bots', ...scope }, bob)).bots[0].permitted, false);
  await assert.rejects(execute({ action: 'heartbeatBot', ...scope, botKey, revision: 1 }, alice), { code: 'BOT_OWNER_REQUIRED' });
  await assert.rejects(execute(bind), { code: 'BOT_REVISION_CONFLICT' });
  await assert.rejects(execute({ ...bind, ownerUid: 'alice' }));
  await assertFails(setDoc(doc(env.authenticatedContext(owner.uid).firestore(), `teams/${teamId}/bots/${botKey}`), { enabled: true }));
  await teams.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await assert.rejects(execute({ ...bind, expectedRevision: 1 }), { code: 'PARTICIPANT_REQUIRED' });
  now += 60001;
  assert.equal((await execute({ action: 'bots', ...scope })).bots[0].online, false);
});

test('T3 protocol: a human send and authenticated execution evidence commit once together', options, async () => {
  const results = await Promise.all(Array.from({ length: 5 }, () => execute(send(), alice)));
  assert.ok(results.every(value => value.changeSeq === 1));
  assert.equal(await countMessages(), 1);
  const triggers = await root().collection('triggers').get(); assert.equal(triggers.size, 1);
  const trigger = triggers.docs[0].data(); assert.equal(trigger.actorUid, alice.uid); assert.equal(trigger.generation, 1); assert.equal(trigger.botRevision, 1);
  const receipt = (await root().collection('changes').doc('human-change01').get()).data();
  assert.deepEqual(receipt.executionIntent.targets, [botKey]); assert.equal(receipt.executionIntent.triggerIds[0], triggers.docs[0].id);
  await assert.rejects(execute(send({ targets: [botKey], text: 'Changed request' }), alice), { code: 'MESSAGE_ID_CONFLICT' });
  await execute(send({ operation: 'edit', changeId: 'edit-human01', text: 'Edited request' }), alice);
  await execute(send(), alice); // Immutable retry evidence survives edits.
  assert.equal((await root().collection('triggers').get()).size, 1);
});

test('T3 protocol: SDK mentions/messages cannot enqueue or be upgraded into trusted execution', options, async () => {
  const client = env.authenticatedContext(alice.uid).firestore(), parent = `teams/${teamId}/conversations/${conversationId}`;
  const batch = writeBatch(client), triggerId = triggerIdentity('human-msg01', botKey);
  batch.update(doc(client, parent), { lastSeq: 1, lastMessage: { id: 'human-msg01', seq: 1, at: serverTimestamp() }, lastChangeSeq: 1, lastChange: { id: 'human-change01', seq: 1 } });
  batch.set(doc(client, `${parent}/messages/human-msg01`), { seq: 1, author: { kind: 'user', uid: alice.uid }, kind: 'text', text: 'Synthetic request', mentions: [], createdAt: serverTimestamp(), changeSeq: 1, changeId: 'human-change01' });
  batch.set(doc(client, `${parent}/changes/human-change01`), { seq: 1, messageId: 'human-msg01', operation: 'create', actorUid: alice.uid, generation: 1, at: serverTimestamp(), textDigest: botDigest('Synthetic request') });
  await assertSucceeds(batch.commit());
  assert.equal((await root().collection('triggers').get()).size, 0);
  await assert.rejects(execute(send(), alice), { code: 'MESSAGE_ID_CONFLICT' });
  for (const collection of ['triggers', 'botRuns', 'botCursors', 'botQuestions']) {
    await assertFails(setDoc(doc(client, `${parent}/${collection}/${triggerId}`), { actorUid: alice.uid }));
    await assertFails(getDoc(doc(client, `${parent}/${collection}/${triggerId}`)));
  }
  const forge = writeBatch(client);
  forge.set(doc(client, `${parent}/changes/forged-intent`), { executionIntent: { targets: [botKey] } });
  await assertFails(forge.commit());
});

test('T3 protocol: group triggers require explicit current permitted participants', options, async () => {
  const group = { ...thread, conversationId: 'bots-group01' };
  await execute({ action: 'create', ...group, kind: 'group', title: 'Group and Bot', participantUids: ['alice', 'bob'], botKeys: [botKey] }, alice);
  await execute(send({ ...group, messageId: 'group-message1', changeId: 'group-change1', text: `@${botKey} is only text` }), alice);
  const ref = db.doc(`teams/${teamId}/conversations/${group.conversationId}`);
  assert.equal((await ref.collection('triggers').get()).size, 0);
  await assert.rejects(execute(send({ ...group, targets: [botKey], messageId: 'group-message2', changeId: 'group-change2' }), bob), { code: 'BOT_USE_REQUIRED' });
  await execute(send({ ...group, targets: [botKey], messageId: 'group-message2', changeId: 'group-change2' }), alice);
  assert.equal((await ref.collection('triggers').get()).size, 1);
  await assert.rejects(execute(send({ ...group, targets: [`bot:owner:${hostId}:${'x'.repeat(22)}`], messageId: 'group-message3', changeId: 'group-change3' }), alice), { code: 'BOT_PARTICIPANT_REQUIRED' });
});

test('T3 protocol: competing hosts receive one execution permit and cannot skip a live trigger', options, async () => {
  await execute(send(), alice);
  await execute(send({ messageId: 'human-msg02', changeId: 'human-change02' }), alice);
  const claims = await Promise.all([execute(run('claimBot')), execute(run('claimBot', { attemptId: 'attempt-002' }))]);
  assert.deepEqual(claims.map(value => value.state).sort(), ['busy', 'claimed']);
  const attemptId = claims[0].state === 'claimed' ? 'attempt-001' : 'attempt-002';
  const starts = await Promise.all([execute(run('startBot', { attemptId })), execute(run('startBot', { attemptId }))]);
  assert.equal(starts.filter(value => value.execute === true).length, 1);
  assert.equal(starts.filter(value => value.execute === false).length, 1);
  await assert.rejects(execute(run('claimBot', { triggerId: triggerIdentity('human-msg02', botKey) })), { code: 'BOT_RUN_BLOCKED' });
  await assert.rejects(execute({ action: 'botQueue', ...thread, botKey, after: 999 }), { code: 'INVALID_BOT_CURSOR' });
});

test('T3 protocol: a crash before start safely reclaims while fencing the old host', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); now += 30001;
  assert.equal((await execute(run('claimBot', { attemptId: 'attempt-002' }))).state, 'claimed');
  await assert.rejects(execute(run('startBot')), { code: 'BOT_LEASE_REQUIRED' });
  assert.equal((await execute(run('startBot', { attemptId: 'attempt-002' }))).execute, true);
});

test('T3 connector: only the one-shot start returns the authenticated execution snapshot', options, async () => {
  await execute(send(), alice);
  await execute(run('claimBot'));
  const first = await execute(run('startBot'));
  assert.equal(first.execute, true);
  assert.deepEqual(first.execution, {
    version: 1, teamId, conversationId, botKey, triggerId: triggerIdentity('human-msg01', botKey),
    attemptId: 'attempt-001', ownerUid: owner.uid, ownerGeneration: 1, hostId, botId, botRevision: 1,
    origin: { uid: alice.uid, generation: 1 }, messageId: 'human-msg01', seq: 1, text: 'Synthetic request',
  });
  const retry = await execute(run('startBot'));
  assert.deepEqual(retry, { state: 'running', execute: false, leaseUntil: first.leaseUntil });
  const poll = await execute(run('botPoll'));
  assert.equal(poll.leaseUntil, first.leaseUntil);
  assert.equal(poll.execution, undefined);
  assert.equal(poll.execute, undefined);
  assert.equal(first.leaseUntil, now + 300000);
});

test('T3 connector: an edit between claim and start cannot authorize the stale claim body', options, async () => {
  await execute(send(), alice);
  const claimed = await execute(run('claimBot'));
  assert.equal(claimed.text, 'Synthetic request');
  await execute(send({ operation: 'edit', changeId: 'between-claim-start', text: 'Changed after claim' }), alice);
  const reply = await execute(run('startBot'));
  assert.deepEqual(reply, { state: 'revoked' });
  assert.equal(reply.execution, undefined);
  assert.equal(await countMessages(), 1);
});

test('T3 connector: owner can manually settle an expired claim after undispatched start intent', options, async () => {
  await execute(send(), alice); await execute(run('claimBot'));
  const { attemptId: _, ...recovery } = run('recoverBot', { text: 'Owner closed an uncertain start without replay.' });
  await assert.rejects(execute(recovery), { code: 'RECOVERY_NOT_REQUIRED' });
  now += 30001;
  await assert.rejects(execute(recovery, alice), { code: 'BOT_OWNER_REQUIRED' });
  const first = await execute(recovery);
  assert.equal(first.state, 'recovered');
  assert.deepEqual(await execute(recovery), first);
  // A late original host or a new claimant sees a terminal receipt, no permit.
  assert.deepEqual(await execute(run('startBot')), first);
  assert.deepEqual(await execute(run('claimBot', { attemptId: 'late-attempt' })), first);
  assert.equal(await countMessages(), 2);
});

test('T3 connector: an active reclaimed lease fences stale manual recovery', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); now += 30001;
  await execute(run('claimBot', { attemptId: 'reclaimed-attempt' }));
  const { attemptId: _, ...recovery } = run('recoverBot', { text: 'Must not settle a live claim' });
  await assert.rejects(execute(recovery), { code: 'RECOVERY_NOT_REQUIRED' });
  await assert.rejects(execute(run('startBot')), { code: 'BOT_LEASE_REQUIRED' });
  assert.equal((await execute(run('startBot', { attemptId: 'reclaimed-attempt' }))).execute, true);
});

test('T3 protocol: ambiguous execution needs owner recovery, and recovery reply is idempotent', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); await execute(run('startBot'));
  now += 300001; // Simulate after-execution/before-reply interruption as well as a lost start ACK.
  assert.equal((await execute(run('claimBot', { attemptId: 'attempt-002' }))).state, 'recovery');
  assert.equal((await execute(run('startBot'))).state, 'recovery');
  const { attemptId: _, ...recovery } = run('recoverBot', { text: 'Closed after owner review' });
  await assert.rejects(execute(recovery, alice), { code: 'BOT_OWNER_REQUIRED' });
  const first = await execute(recovery), retry = await execute(recovery);
  assert.equal(first.state, 'recovered'); assert.deepEqual(first, retry); assert.equal(await countMessages(), 3);
  await assert.rejects(execute({ ...recovery, text: 'Different recovery' }), { code: 'BOT_RESULT_CONFLICT' });
  assert.equal((await execute({ action: 'botQueue', ...thread, botKey, after: 0 })).triggers.length, 0);
});

test('T3 protocol: reply and checkpoint commit atomically; lost reply ACK never repeats a reply', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); await execute(run('startBot'));
  const command = run('finishBot', { text: 'Synthetic Bot reply', outcome: 'completed' });
  const results = await Promise.all([execute(command), execute(command)]);
  assert.equal(results[0].replyId, results[1].replyId); assert.equal(await countMessages(), 3);
  assert.equal((await root().collection('triggers').get()).size, 1); // Bot reply/status do not trigger anything.
  const page = await sync(); assert.deepEqual(page.changes.map(value => value.seq), [1, 2, 3]); assert.equal(page.messages.at(-1).author.botKey, botKey);
  await assert.rejects(execute({ ...command, text: 'Different result' }), { code: 'BOT_RESULT_CONFLICT' });
});

test('T3 protocol: old human generations and revoked/rebound policies cannot revive queued work', options, async () => {
  await execute(send(), alice);
  await teams.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite002', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(alice, { action: 'join', code, requestId: 'alice-rejoin' });
  assert.equal((await execute(run('claimBot'))).state, 'revoked');
  await execute(send({ generation: 2, messageId: 'human-msg02', changeId: 'human-change02' }), alice);
  await execute({ action: 'unbindBot', ...scope, botKey, expectedRevision: 1 });
  await execute({ ...bind, expectedRevision: 2 });
  assert.equal((await execute(run('claimBot', { triggerId: triggerIdentity('human-msg02', botKey) }))).state, 'revoked');
  assert.equal(await countMessages(), 2);
});

test('T3 protocol: edits/recalls revoke pending work and recovery also rechecks revocation', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); await execute(run('startBot')); now += 300001;
  await execute(send({ operation: 'recall', changeId: 'recall-human1', text: '' }), alice);
  const { attemptId: _, ...recovery } = run('recoverBot', { text: 'Must not publish this result' });
  assert.equal((await execute(recovery)).state, 'revoked'); assert.equal(await countMessages(), 2);
  assert.ok(!(await sync()).messages.some(value => value.text === recovery.text));
});

test('T3 protocol: questions are answered only by the original human, with durable exact retries', options, async () => {
  await execute(send(), alice); await execute(run('claimBot')); await execute(run('startBot'));
  await assert.rejects(execute(run('botQuestion', { questionId: 'question-001', text: 'Cannot answer duplicate keys', questions: ['Same?', 'Same?'] })));
  await execute(run('botQuestion', { questionId: 'question-001', text: 'Which synthetic option?', questions: ['Which option?'] }));
  const answer = { action: 'botAnswer', ...thread, triggerId: triggerIdentity('human-msg01', botKey), questionId: 'question-001', answerId: 'answer-001', answers: { 'Which option?': 'Synthetic choice' } };
  await assert.rejects(execute(answer), { code: 'QUESTION_RECIPIENT_REQUIRED' });
  await assert.rejects(execute(answer, bob), { code: 'QUESTION_RECIPIENT_REQUIRED' });
  await execute(answer, alice); await execute(answer, alice);
  assert.deepEqual((await execute(run('botPoll'))).answers, answer.answers);
  await assert.rejects(execute({ ...answer, answers: { 'Which option?': 'Different' } }, alice), { code: 'ANSWER_CONFLICT' });
  await execute(run('finishBot', { text: 'Question answered', outcome: 'completed' }));
  await execute(answer, alice); // The answer ACK can be retried after the Bot finishes.
  await assert.rejects(execute({ ...answer, generation: 2 }, alice), { code: 'STALE_MEMBERSHIP' });
});

test('T3 protocol: participants cannot preoccupy reserved Bot output message or change IDs', options, async () => {
  const triggerId = triggerIdentity('human-msg01', botKey), messageId = botOutputIdentity(triggerId, 'reply'), changeId = botOutputIdentity(triggerId, 'reply:change');
  await assert.rejects(execute(send({ messageId }), alice), { code: 'INVALID_INPUT' });
  await assert.rejects(execute(send({ changeId }), alice), { code: 'INVALID_INPUT' });
  const client = env.authenticatedContext(alice.uid).firestore(), parent = `teams/${teamId}/conversations/${conversationId}`;
  // Both SDK paths supply otherwise valid coupled writes; rejection must be
  // specifically enforced even when no trusted output exists yet.
  for (const [mid, cid] of [[messageId, 'human-change01'], ['human-msg01', changeId]]) {
    const batch = writeBatch(client);
    batch.update(doc(client, parent), { lastSeq: 1, lastMessage: { id: mid, seq: 1, at: serverTimestamp() }, lastChangeSeq: 1, lastChange: { id: cid, seq: 1 } });
    batch.set(doc(client, `${parent}/messages/${mid}`), { seq: 1, author: { kind: 'user', uid: alice.uid }, kind: 'text', text: 'Preoccupation', mentions: [], createdAt: serverTimestamp(), changeSeq: 1, changeId: cid });
    batch.set(doc(client, `${parent}/changes/${cid}`), { seq: 1, messageId: mid, operation: 'create', actorUid: alice.uid, generation: 1, at: serverTimestamp(), textDigest: botDigest('Preoccupation') });
    await assertFails(batch.commit());
  }
  await execute(send(), alice); await execute(run('claimBot')); await execute(run('startBot'));
  assert.equal((await execute(run('finishBot', { outcome: 'completed', text: 'Reserved output succeeds' }))).replyId, messageId);
  await assertFails(setDoc(doc(client, `${parent}/messages/${messageId}`), { text: 'Overwritten' }, { merge: true }));
});

test('T3 protocol: expired offline work gets one missed reply and HTTP cannot spoof authorship', options, async () => {
  await execute(send(), alice); now += 86400001;
  assert.equal((await execute(run('claimBot'))).state, 'missed');
  assert.equal((await execute(run('claimBot'))).state, 'missed'); assert.equal(await countMessages(), 2);
  const handler = createTeamsHandler({ store: teams, verifyToken: async () => alice, createCustomToken: async () => 'unused' });
  const response = await handler(new Request('https://lingxios.net/v1/teams/messages', { method: 'POST', headers: { Authorization: 'Bearer fixture' }, body: JSON.stringify(send({ author: { kind: 'bot', uid: 'owner', botKey } })) }));
  assert.equal(response.status, 400);
});
