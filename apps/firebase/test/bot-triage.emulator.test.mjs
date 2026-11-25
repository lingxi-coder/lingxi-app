// Bot v3 B5a: the auto-reply log. The host records what it decided about each message nobody asked its Bot about; the maintainer reads those verdicts, newest first.
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { FieldValue, getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { admissionId } from '../src/team-execution-contract.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-bot-triage', teamId = 'team_triage01', botId = 'sharedbot_01', otherBotId = 'sharedbot_02';
const trip = 'chat_triage_a', lunch = 'chat_triage_b';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const memberHost = { deploymentId: 'deployment_02', hostId: 'host_0002', accountEpoch: 1 };
const asking = 'Does anyone know a quiet hotel near the station?';
let app, db, teams, store, messages, now, serial = 0;
const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner, commandId) => store.execute(actor, command(action, payload, commandId));
const publication = (sharedBotId, deploymentHost, maintainer) => ({ sharedBotId, expectedRevision: 0, name: sharedBotId === botId ? 'Travel assistant' : 'Lunch assistant',
  description: '', usePolicy: { mode: 'all' }, autoReply: { daily: 50 },
  deployment: { ...deploymentHost, sharedBotId, revision: 1, owner: { uid: maintainer.uid, generation: 1 }, instanceId: `instance_${sharedBotId}`,
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: false, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 } });
const taskRef = id => db.doc(`teams/${teamId}/tasks/${id}`);
const taskData = async id => (await taskRef(id).get()).data();
const chatRef = id => db.doc(`teams/${teamId}/conversations/${id}`);
const messageId = (chat, n) => `msg_${chat === trip ? 'a' : 'b'}_${String(n).padStart(4, '0')}`;
const taskOf = (chat, n, sharedBotId = botId) => admissionId(messageId(chat, n), sharedBotId);
/** A member writes to the Chat; each Bot of it that allows auto-reply is offered the message as an optional task nobody was shown. */
const say = (actor, chat, n, text = asking) => messages.execute(actor, { action: 'mutate', teamId, generation: 1, conversationId: chat, messageId: messageId(chat, n),
  changeId: `chg_${chat === trip ? 'a' : 'b'}_${String(n).padStart(4, '0')}`, operation: 'create', text });
const record = (chat, n, verdict, { actor = owner, deploymentHost = host, sharedBotId = botId, commandId } = {}) =>
  execute('recordTriage', { ...deploymentHost, taskId: taskOf(chat, n, sharedBotId), ...verdict }, actor, commandId);
const log = (more = {}, actor = owner, sharedBotId = botId) => execute('listTriage', { sharedBotId, ...more }, actor);
const start = async (chat, n) => execute('startTask', { ...host, taskId: taskOf(chat, n), expectedLastChangeSeq: (await chatRef(chat).get()).data().lastChangeSeq });
const complete = (started, more = {}) => execute('completeTask', { ...host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: 'The Station Inn is quiet.',
  finishedAt: now, inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const decline = (chat, n) => execute('declineTask', { ...host, taskId: taskOf(chat, n), reason: 'triage' });
const ran = { action: 'run', source: 'rule', reason: 'keyword' };
const silent = { action: 'decline', source: 'cooldown', reason: 'cooldown' };
const judged = { action: 'run_may_decline', source: 'decision', reason: 'between_thresholds', probability: 0.5 };

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'bot-triage-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  for (const name of ['teams', 'users']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 24); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Triage test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  // The owner's Bot and the member's Bot, both allowing auto-reply, share two group Chats that switch it on for both.
  await execute('publishSharedBot', publication(botId, host, owner));
  await execute('heartbeatDeployment', { ...host, ready: true });
  await execute('publishSharedBot', publication(otherBotId, memberHost, member), member);
  await execute('heartbeatDeployment', { ...memberHost, ready: true }, member);
  for (const [id, title] of [[trip, 'Trip'], [lunch, 'Lunch']]) {
    await execute('createConversation', { conversationId: id, kind: 'group', title, participantUids: [member.uid, other.uid], sharedBotIds: [botId, otherBotId] }, member);
    await execute('setConversationAutoReply', { conversationId: id, expectedRevision: 0, targets: [botId, otherBotId] }, member);
  }
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('recordTriage: the verdict is kept on the task in codes and the service clock, and members never see it', options, async () => {
  await say(member, trip, 1);
  const taskId = taskOf(trip, 1);
  assert.equal((await taskData(taskId)).triage, undefined);
  assert.deepEqual(await record(trip, 1, judged), { taskId });
  assert.deepEqual((await taskData(taskId)).triage, { ...judged, at: now });
  // Only what the host decided is kept, as codes: a probability is rounded to three decimals, and a verdict without a reason or a probability has neither.
  await say(member, trip, 2); await say(member, trip, 3);
  await record(trip, 2, { action: 'run', source: 'decision', reason: 'above_threshold', probability: 0.123456 });
  assert.equal((await taskData(taskOf(trip, 2))).triage.probability, 0.123);
  await record(trip, 3, { action: 'run', source: 'fallback' });
  assert.deepEqual((await taskData(taskOf(trip, 3))).triage, { action: 'run', source: 'fallback', at: now });
  // The task a member can read says nothing of it, and a task nobody was shown is still not theirs to list.
  assert.equal((await taskData(taskId)).visibility, 'hidden');
  assert.deepEqual((await execute('listTasks', { conversationId: trip }, member)).tasks, []);
  const view = await execute('getTask', { taskId }, member);
  assert.equal(Object.hasOwn(view, 'triage'), false);
  assert.doesNotMatch(JSON.stringify(view), /between_thresholds|run_may_decline/u);
  await assert.rejects(execute('recordTriage', { ...host, taskId, action: 'run', source: 'rule', text: 'I would rather not.' }), /./u);
});

test('recordTriage: the same verdict again changes nothing, a later one replaces it while the task is queued, and a task that went on keeps its own', options, async () => {
  await say(member, trip, 1);
  const taskId = taskOf(trip, 1), first = now;
  await record(trip, 1, judged, { commandId: 'command_fixed' });
  now += 5000;
  // The same command again is answered from its receipt; a new command with the same verdict finds it recorded and leaves its time.
  assert.deepEqual(await record(trip, 1, judged, { commandId: 'command_fixed' }), { taskId });
  assert.deepEqual(await record(trip, 1, judged), { taskId });
  assert.equal((await taskData(taskId)).triage.at, first);
  // A host that started again judged again: until the task starts, the later verdict stands, and nothing of the earlier one is left.
  assert.deepEqual(await record(trip, 1, { action: 'decline', source: 'quiet', reason: 'quiet_hours' }), { taskId });
  assert.deepEqual((await taskData(taskId)).triage, { action: 'decline', source: 'quiet', reason: 'quiet_hours', at: now });
  await record(trip, 1, ran);
  assert.deepEqual((await taskData(taskId)).triage, { ...ran, at: now });
  // Started, the task keeps the verdict it started with.
  const started = await start(trip, 1);
  assert.equal(started.execute, true);
  const at = now;
  now += 5000;
  assert.deepEqual(await record(trip, 1, ran), { taskId }, 'the same verdict is answered');
  await assert.rejects(record(trip, 1, judged), { code: 'TRIAGE_CONFLICT' });
  await assert.rejects(record(trip, 1, { ...ran, reason: 'addressed' }), { code: 'TRIAGE_CONFLICT' });
  assert.deepEqual((await taskData(taskId)).triage, { ...ran, at });
  // Finished, so too.
  await complete(started);
  await assert.rejects(record(trip, 1, silent), { code: 'TRIAGE_CONFLICT' });
  assert.equal((await taskData(taskId)).state, 'completed');
});

test('recordTriage: a task that started or ended with no verdict is not given one, and a declined one keeps what it was declined with', options, async () => {
  await say(member, trip, 1); await say(member, trip, 2);
  // Started without a verdict: the host records before it acts, so there is nothing to say afterwards.
  await start(trip, 1);
  await assert.rejects(record(trip, 1, ran), { code: 'TASK_ALREADY_STARTED' });
  assert.equal((await taskData(taskOf(trip, 1))).triage, undefined);
  // The host declines after it records.
  await record(trip, 2, silent);
  assert.deepEqual(await decline(trip, 2), { taskId: taskOf(trip, 2), state: 'declined' });
  assert.deepEqual(await record(trip, 2, silent), { taskId: taskOf(trip, 2) });
  await assert.rejects(record(trip, 2, ran), { code: 'TRIAGE_CONFLICT' });
  assert.deepEqual((await taskData(taskOf(trip, 2))).triage, { ...silent, at: now });
  // A declined task with no verdict (superseded by another, say) is not given one either.
  await say(member, lunch, 3); await say(other, lunch, 4);
  await start(lunch, 4);
  assert.equal((await taskData(taskOf(lunch, 3))).state, 'declined');
  await assert.rejects(record(lunch, 3, ran), { code: 'TASK_ALREADY_STARTED' });
});

test('recordTriage: only the Bot\'s current deployment records, and only for an optional task of its own', options, async () => {
  await say(member, trip, 1);
  // Not the maintainer, not the deployment's host, not its account epoch.
  await assert.rejects(record(trip, 1, ran, { actor: member }), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(record(trip, 1, ran, { actor: other }), { code: 'DEPLOYMENT_REQUIRED' });
  for (const wrong of [{ hostId: 'host_other' }, { accountEpoch: 2 }, { deploymentId: 'deployment_09' }]) {
    await assert.rejects(record(trip, 1, ran, { deploymentHost: { ...host, ...wrong } }), { code: 'DEPLOYMENT_REQUIRED' }, JSON.stringify(wrong));
  }
  // The member's own Bot is a deployment of its own: it records for its own tasks, never for the owner's.
  await assert.rejects(record(trip, 1, ran, { actor: member, deploymentHost: memberHost }), { code: 'TASK_REQUIRED' });
  assert.deepEqual(await record(trip, 1, ran, { actor: member, deploymentHost: memberHost, sharedBotId: otherBotId }), { taskId: taskOf(trip, 1, otherBotId) });
  assert.equal((await taskData(taskOf(trip, 1))).triage, undefined);
  // A task somebody asked is judged by nobody, and one that does not exist is none.
  const asked = await execute('submit', { conversationId: trip, messageId: 'asked_msg_01', changeId: 'asked_chg_01', text: 'Hotel?', targets: [botId] }, member);
  await assert.rejects(execute('recordTriage', { ...host, taskId: asked.tasks[0].taskId, ...ran }), { code: 'TASK_NOT_OPTIONAL' });
  await assert.rejects(execute('recordTriage', { ...host, taskId: 'task_does_not_exist', ...ran }), { code: 'TASK_REQUIRED' });
});

test('listTriage: the maintainer reads their Bot\'s verdicts newest first with where each task stands, hidden ones included and no message text', options, async () => {
  for (const [chat, n] of [[trip, 1], [lunch, 2], [lunch, 3], [trip, 4], [trip, 5]]) { await say(member, chat, n); now += 1000; }
  // Trip 1 waits, lunch 2 was declined, lunch 3 was answered, trip 4 waits, trip 5 was never judged.
  await record(trip, 1, ran); now += 1000;
  await record(lunch, 2, silent); await decline(lunch, 2); now += 1000;
  await record(lunch, 3, { action: 'run', source: 'decision', reason: 'above_threshold', probability: 0.9 }); await complete(await start(lunch, 3)); now += 1000;
  await record(trip, 4, judged);
  const page = await log();
  assert.deepEqual(page.items.map(item => item.taskId), [taskOf(trip, 4), taskOf(lunch, 3), taskOf(lunch, 2), taskOf(trip, 1)]);
  assert.deepEqual(page.items.map(item => item.state), ['queued', 'completed', 'declined', 'queued']);
  assert.deepEqual(page.items.map(item => item.conversationTitle), ['Trip', 'Lunch', 'Lunch', 'Trip']);
  assert.equal(page.nextCursor, undefined);
  assert.deepEqual(page.items[0], { taskId: taskOf(trip, 4), conversationId: trip, messageId: messageId(trip, 4), createdAt: (await taskData(taskOf(trip, 4))).createdAt,
    triage: { ...judged, at: now }, state: 'queued', conversationTitle: 'Trip' });
  assert.equal((await taskData(taskOf(trip, 1))).visibility, 'hidden', 'a task nobody was shown is in the maintainer\'s log');
  assert.doesNotMatch(JSON.stringify(page), /hotel|station|Station Inn/iu, 'the log holds codes and ids, not what anyone wrote');
  // The other Bot\'s tasks are not in it, and its verdicts are its maintainer\'s.
  await record(trip, 1, ran, { actor: member, deploymentHost: memberHost, sharedBotId: otherBotId });
  assert.equal((await log()).items.length, 4);
  assert.deepEqual((await log({}, member, otherBotId)).items.map(item => item.taskId), [taskOf(trip, 1, otherBotId)]);
});

test('listTriage: only the Bot\'s maintainer reads it, and reading it leaves no receipt', options, async () => {
  await say(member, trip, 1); await record(trip, 1, ran);
  const receipts = (await db.collection(`teams/${teamId}/executionReceipts`).get()).size;
  assert.equal((await log()).items.length, 1);
  assert.equal((await db.collection(`teams/${teamId}/executionReceipts`).get()).size, receipts, 'a read keeps no receipt');
  assert.equal((await db.collection(`teams/${teamId}/requestQuotas`).get()).size, 0);
  for (const actor of [member, other]) await assert.rejects(log({}, actor), { code: 'MAINTAINER_REQUIRED' }, actor.uid);
  await assert.rejects(log({}, owner, otherBotId), { code: 'MAINTAINER_REQUIRED' }, 'nor is a Bot of someone else the owner\'s to read');
  await assert.rejects(log({}, owner, 'sharedbot_missing'), { code: 'MAINTAINER_REQUIRED' });
  // A member who is not in the Team gets nothing from the service at all.
  await assert.rejects(execute('listTriage', { sharedBotId: botId }, { uid: 'stranger', name: 'Stranger' }), { code: 'MEMBERSHIP_REQUIRED' });
});

test('listTriage: pages follow the cursor without gaps or repeats, a Chat can be asked for alone, and a cursor must be one of the log\'s own', options, async () => {
  // Seven verdicts, the last three at the same instant: the order of equals is fixed, so no page repeats or skips one.
  for (const n of [1, 2, 3, 4]) { await say(member, trip, n); now += 1000; await record(trip, n, ran); }
  for (const n of [5, 6, 7]) await say(member, lunch, n);
  now += 1000;
  for (const n of [5, 6, 7]) await record(lunch, n, silent);
  const all = [];
  let cursor;
  for (let pages = 0; pages < 4; pages += 1) {
    const page = await log({ limit: 3, ...(cursor ? { cursor } : {}) });
    all.push(...page.items.map(item => item.taskId));
    cursor = page.nextCursor;
    assert.equal(page.items.length, cursor ? 3 : 1, `page ${pages}`);
    if (!cursor) break;
  }
  assert.equal(cursor, undefined);
  assert.equal(new Set(all).size, 7);
  assert.deepEqual(all.slice(3).map(taskId => taskId), [4, 3, 2, 1].map(n => taskOf(trip, n)), 'the earlier instant follows the three at the same one');
  assert.deepEqual(all.slice(0, 3).sort(), [5, 6, 7].map(n => taskOf(lunch, n)).sort());
  // One Chat alone, in the same order, with its own end.
  const first = await log({ conversationId: trip, limit: 3 });
  assert.deepEqual(first.items.map(item => item.taskId), [4, 3, 2].map(n => taskOf(trip, n)));
  assert.equal(first.nextCursor, taskOf(trip, 2));
  const last = await log({ conversationId: trip, limit: 3, cursor: first.nextCursor });
  assert.deepEqual(last.items.map(item => item.taskId), [taskOf(trip, 1)]);
  assert.equal(last.nextCursor, undefined);
  assert.deepEqual((await log({ conversationId: lunch })).items.map(item => item.conversationTitle), ['Lunch', 'Lunch', 'Lunch']);
  assert.equal((await log({ limit: 50 })).items.length, 7);
  // A cursor is a task of this Bot's log, in the Chat asked for.
  await say(member, trip, 8); await say(member, trip, 9);
  await record(trip, 9, ran, { actor: member, deploymentHost: memberHost, sharedBotId: otherBotId });
  for (const bad of [taskOf(trip, 8), taskOf(trip, 9, otherBotId), 'task_does_not_exist']) {
    await assert.rejects(log({ cursor: bad }), { code: 'INVALID_TRIAGE_CURSOR' }, bad);
  }
  await assert.rejects(log({ conversationId: lunch, cursor: taskOf(trip, 1) }), { code: 'INVALID_TRIAGE_CURSOR' });
  await assert.rejects(execute('listTriage', { sharedBotId: botId, limit: 51 }), /./u);
});

test('listTriage: a Chat the maintainer no longer reads is left out of the log', options, async () => {
  await say(member, trip, 1); await say(member, lunch, 2);
  await record(trip, 1, ran); now += 1000; await record(lunch, 2, silent);
  assert.equal((await log()).items.length, 2);
  await chatRef(lunch).update({ memberUids: FieldValue.arrayRemove(owner.uid) });
  const page = await log();
  assert.deepEqual(page.items.map(item => item.taskId), [taskOf(trip, 1)]);
  assert.deepEqual((await log({ conversationId: lunch })).items, [], 'asking for that Chat alone shows none of it');
});
