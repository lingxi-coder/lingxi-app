// Bot v3 B6a: a Bot's reply that its maintainer approves before it is posted. The host sends `holdReply` instead of `completeTask`; the service keeps the
// reply as a draft only it reads, the maintainer approves (as it is or edited), rejects or lets it time out, and a newer message of the Chat ends it.
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { admissionId } from '../src/team-execution-contract.mjs';
import { HeldReplyJobs } from '../src/held-reply-jobs.mjs';
import { approvalMs, expireHeldReply } from '../src/held-replies.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-held-replies', teamId = 'team_held_01', botId = 'sharedbot_01', otherBotId = 'sharedbot_02';
const trip = 'chat_held_trip', dm = 'chat_held_dm', lunch = 'chat_held_lunch';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const memberHost = { deploymentId: 'deployment_02', hostId: 'host_0002', accountEpoch: 1 };
const asking = 'Does anyone know a quiet hotel near the station?';
const draft = 'The Station Inn is quiet, and ten minutes from the station.';
let app, db, teams, store, messages, now, serial = 0;
const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner, commandId) => store.execute(actor, command(action, payload, commandId));
const publication = (sharedBotId, deploymentHost, maintainer) => ({ sharedBotId, expectedRevision: 0, name: sharedBotId === botId ? 'Travel bot' : 'Lunch bot',
  description: '', usePolicy: { mode: 'all' }, autoReply: { daily: 50 },
  deployment: { ...deploymentHost, sharedBotId, revision: 1, owner: { uid: maintainer.uid, generation: 1 }, instanceId: `instance_${sharedBotId}`,
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: true, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 } });
const taskRef = id => db.doc(`teams/${teamId}/tasks/${id}`);
const taskData = async id => (await taskRef(id).get()).data();
const draftRef = id => db.doc(`teams/${teamId}/heldReplies/${id}`);
const draftData = async id => (await draftRef(id).get()).data();
const chatRef = id => db.doc(`teams/${teamId}/conversations/${id}`);
const chatMessages = async chat => (await chatRef(chat).collection('messages').orderBy('seq').get()).docs.map(doc => ({ id: doc.id, ...doc.data() }));
const messageId = (chat, n) => `msg_${chat.slice(-4)}_${String(n).padStart(4, '0')}`;
const taskOf = (chat, n, sharedBotId = botId) => admissionId(messageId(chat, n), sharedBotId);
/** A member writes to the Chat; each Bot of it that allows auto-reply is offered the message as an optional task nobody was shown. */
const say = (actor, chat, n, text = asking) => messages.execute(actor, { action: 'mutate', teamId, generation: 1, conversationId: chat, messageId: messageId(chat, n),
  changeId: `chg_${chat.slice(-4)}_${String(n).padStart(4, '0')}`, operation: 'create', text });
/** A member asks the Bot by name: a task somebody asked for. */
const ask = (actor, chat, n, sharedBotId = botId) => execute('submit', { conversationId: chat, messageId: messageId(chat, n), changeId: `chg_${chat.slice(-4)}_${String(n).padStart(4, '0')}`,
  text: asking, targets: [sharedBotId] }, actor).then(receipt => receipt.tasks[0].taskId);
const start = async (chat, taskId, actor = owner, deploymentHost = host) => execute('startTask', { ...deploymentHost, taskId,
  expectedLastChangeSeq: (await chatRef(chat).get()).data().lastChangeSeq }, actor);
const result = (started, more = {}, deploymentHost = host) => ({ ...deploymentHost, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: draft,
  finishedAt: now, inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const hold = (started, more = {}, actor = owner, commandId) => execute('holdReply', result(started, more), actor, commandId);
const complete = (started, more = {}) => execute('completeTask', result(started, more));
const listed = (more = {}, actor = owner, sharedBotId = botId) => execute('listHeldReplies', { sharedBotId, ...more }, actor);
const approve = (taskId, more = {}, actor = owner, commandId) => execute('approveReply', { taskId, ...more }, actor, commandId);
const reject = (taskId, actor = owner, commandId) => execute('rejectReply', { taskId }, actor, commandId);
const expire = taskId => expireHeldReply(db, () => now, { teamId, taskId });
const log = (more = {}) => execute('listTriage', { sharedBotId: botId, ...more });
/** The hosts report again: a clock that jumped ten minutes is a host that stayed online. */
const beat = async () => { await execute('heartbeatDeployment', { ...host, ready: true }); await execute('heartbeatDeployment', { ...memberHost, ready: true }, member); };
const edit = (chat, n, text) => messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId: chat, messageId: messageId(chat, n),
  changeId: `chg_edit_${chat.slice(-4)}_${String(n).padStart(4, '0')}`, operation: 'edit', text });
/** A task of the Bot's, asked for in the group Chat and held. */
async function heldAsked(n = 1, more = {}) {
  const taskId = await ask(member, trip, n), started = await start(trip, taskId);
  return { taskId, started, held: await hold(started, more) };
}
/** A message nobody asked about, held: the host judged it and the Bot answered. */
async function heldUnasked(n = 1, chat = trip, more = {}) {
  await say(member, chat, n);
  const taskId = taskOf(chat, n), started = await start(chat, taskId);
  return { taskId, started, held: await hold(started, more) };
}

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'held-replies-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  for (const name of ['teams', 'users']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 25); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Held replies test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  // The owner's Bot and the member's Bot, both allowing auto-reply; two group Chats switch it on, and the owner's Bot has a Bot DM with the member too.
  await execute('publishSharedBot', publication(botId, host, owner));
  await execute('heartbeatDeployment', { ...host, ready: true });
  await execute('publishSharedBot', publication(otherBotId, memberHost, member), member);
  await execute('heartbeatDeployment', { ...memberHost, ready: true }, member);
  for (const [id, title] of [[trip, 'Trip'], [lunch, 'Lunch']]) {
    await execute('createConversation', { conversationId: id, kind: 'group', title, participantUids: [member.uid, other.uid], sharedBotIds: [botId, otherBotId] }, member);
    await execute('setConversationAutoReply', { conversationId: id, expectedRevision: 0, targets: [botId, otherBotId] }, member);
  }
  await execute('createConversation', { conversationId: dm, kind: 'bot_dm', title: 'Travel bot', participantUids: [member.uid], sharedBotIds: [botId] }, member);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('holdReply: a reply that would be posted is kept as a draft, and nothing of it reaches the Chat or the members', options, async () => {
  const taskId = await ask(member, trip, 1), started = await start(trip, taskId);
  assert.equal((await db.collection(`teams/${teamId}/chatExecutionLocks`).get()).size, 1, 'the run holds the Bot x Chat lock');
  const before = await chatMessages(trip);
  const held = await hold(started);
  assert.deepEqual(held, { taskId, state: 'awaiting_approval', replyId: null, artifactId: null });
  // The task waits for the maintainer for as long as they have; the host's work is done and the Chat is free for its Bot again.
  const task = await taskData(taskId);
  assert.equal(task.state, 'awaiting_approval');
  assert.equal(task.heldAt, now);
  assert.equal(task.deadline, now + approvalMs);
  assert.equal(task.replyId, undefined);
  assert.equal((await db.collection(`teams/${teamId}/chatExecutionLocks`).get()).size, 0, 'the lock was released');
  assert.deepEqual(await chatMessages(trip), before, 'nothing was posted');
  // The draft is a document of its own: exactly the reply, for the maintainer, with no word of it on the task.
  const kept = await draftData(taskId);
  assert.equal(kept.result.text, draft);
  assert.deepEqual([kept.sharedBotId, kept.conversationId, kept.maintainer, kept.requester, kept.unasked, kept.heldAt, kept.expiresAt],
    [botId, trip, { uid: owner.uid, generation: 1 }, { uid: member.uid, generation: 1 }, false, now, now + approvalMs]);
  assert.doesNotMatch(JSON.stringify(task), /Station Inn/u);
  // A member sees that it waits for approval, and nothing of what it says.
  const view = await execute('getTask', { taskId }, member);
  assert.deepEqual([view.state, view.deadline, view.replyId], ['awaiting_approval', now + approvalMs, undefined]);
  const rows = (await execute('listTasks', { conversationId: trip }, other)).tasks;
  assert.deepEqual(rows.map(row => [row.taskId, row.state]), [[taskId, 'awaiting_approval']]);
  assert.doesNotMatch(JSON.stringify([view, rows]), /Station Inn|quiet/u);
  // The host is answered the same again for the same request, and no second draft or message appears.
  assert.deepEqual(await hold(started), held);
  assert.deepEqual(await chatMessages(trip), before);
});

test('holdReply: the maintainer reads the draft, and only they', options, async () => {
  const { taskId } = await heldAsked();
  now += 5000;
  assert.deepEqual(await listed(), { replies: [{ taskId, conversationId: trip, messageId: messageId(trip, 1), requester: { uid: member.uid, generation: 1 }, unasked: false,
    heldAt: now - 5000, expiresAt: now - 5000 + approvalMs, text: draft, conversationTitle: 'Trip' }] });
  for (const actor of [member, other]) await assert.rejects(listed({}, actor), { code: 'MAINTAINER_REQUIRED' }, actor.uid);
  await assert.rejects(listed({}, owner, otherBotId), { code: 'MAINTAINER_REQUIRED' }, 'a Bot of someone else is not theirs');
  await assert.rejects(listed({}, owner, 'sharedbot_missing'), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(approve(taskId, {}, member), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(reject(taskId, other), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(approve('task_does_not_exist', {}, owner), { code: 'TASK_REQUIRED' });
  assert.equal((await taskData(taskId)).state, 'awaiting_approval', 'a refused decision changes nothing');
  // Reading keeps no receipt, and a Chat can be asked for alone.
  const receipts = (await db.collection(`teams/${teamId}/executionReceipts`).get()).size;
  assert.deepEqual((await listed({ conversationId: lunch })).replies, []);
  assert.equal((await listed({ conversationId: trip })).replies.length, 1);
  assert.equal((await db.collection(`teams/${teamId}/executionReceipts`).get()).size, receipts);
});

/** What a task and the Bot's message look like once the reply is posted, without the ids, the clock and the cursors that differ between two tasks. */
async function posted(chat, taskId) {
  const { id: _id, seq: _seq, changeSeq: _changeSeq, changeId: _changeId, createdAt: _createdAt, taskId: _taskId, replyTo: _replyTo, deliveryId: _deliveryId, ...message } = (await chatMessages(chat)).find(row => row.taskId === taskId);
  const task = await taskData(taskId), change = (await chatRef(chat).collection('changes').doc((await chatMessages(chat)).find(row => row.taskId === taskId).changeId).get()).data();
  return { message, state: task.state, usage: task.usage, replied: typeof task.replyId === 'string', delivered: Number.isSafeInteger(task.deliveredAt),
    change: { operation: change.operation, actorUid: change.actorUid, generation: change.generation, textDigest: change.textDigest } };
}

test('approveReply: posts the reply by the code completeTask posts one with, as the maintainer, while their host is off', options, async () => {
  // The same reply twice: once as the host completes it, once as it is held and then approved.
  const direct = await ask(member, trip, 1);
  await complete(await start(trip, direct));
  const { taskId } = await heldAsked(2);
  await execute('heartbeatDeployment', { ...host, ready: false });
  now += 7 * 60 * 1000;
  const before = await chatMessages(trip);
  const response = await approve(taskId);
  assert.deepEqual(response, { taskId, state: 'completed', replyId: (await taskData(taskId)).replyId, artifactId: null });
  assert.equal((await chatMessages(trip)).length, before.length + 1);
  const task = await taskData(taskId);
  assert.deepEqual([task.state, task.approval, task.heldAt, task.deliveredAt], ['completed', { decision: 'approved', by: { uid: owner.uid, generation: 1 }, at: now }, now - 7 * 60 * 1000, now]);
  assert.deepEqual(await posted(trip, taskId), await posted(trip, direct), 'the same message, task and change as a host that completed it itself');
  const message = (await chatMessages(trip)).find(row => row.taskId === taskId);
  assert.deepEqual([message.text, message.author, message.replyTo, message.kind], [draft, { kind: 'shared_bot', sharedBotId: botId, displayName: 'Travel bot' }, messageId(trip, 2), 'text']);
  assert.equal((await chatRef(trip).get()).data().lastMessage.id, message.id);
  // The draft is gone with the waiting, and the requester sees a completed task with its reply.
  assert.equal((await draftRef(taskId).get()).exists, false);
  assert.deepEqual((await listed()).replies, []);
  const view = await execute('getTask', { taskId }, member);
  assert.deepEqual([view.state, view.replyId], ['completed', message.id]);
});

test('approveReply: the maintainer may send their own text, which is trimmed; the document the run made stays', options, async () => {
  const document = { title: 'Hotels', body: '# Station Inn\n\nQuiet, near the station.' };
  const { taskId } = await heldAsked(1, { document });
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 0, 'a held document is not an artifact yet');
  const response = await approve(taskId, { text: '  The Station Inn is quiet. I checked.\n' });
  const message = (await chatMessages(trip)).find(row => row.taskId === taskId);
  assert.equal(message.text, 'The Station Inn is quiet. I checked.');
  assert.doesNotMatch(JSON.stringify(await chatMessages(trip)), /ten minutes from the station/u, 'what the host wrote is not posted');
  assert.equal(response.artifactId, message.artifactId);
  const artifact = (await db.doc(`teams/${teamId}/artifacts/${response.artifactId}`).get()).data();
  assert.deepEqual([artifact.document, artifact.revision, artifact.taskId, artifact.conversationId], [document, 1, taskId, trip]);
  assert.deepEqual((await taskData(taskId)).approval, { decision: 'approved', by: { uid: owner.uid, generation: 1 }, at: now, edited: true });
  // Text that is the host's own, trimmed or not, is not an edit.
  const second = await heldAsked(2);
  await approve(second.taskId, { text: ` ${draft} ` });
  assert.equal((await taskData(second.taskId)).approval.edited, undefined);
  // The edit has the limits of a run's reply: something to say, and at most 8 KiB.
  const third = await heldAsked(3);
  for (const text of ['', '   \n', 'x'.repeat(8193), '中'.repeat(2731), 1, null]) await assert.rejects(approve(third.taskId, { text }), /./u, JSON.stringify(text)?.slice(0, 20));
  await assert.rejects(execute('approveReply', { taskId: third.taskId, document }, owner), /./u, 'there is nothing to say about the document');
  assert.equal((await taskData(third.taskId)).state, 'awaiting_approval');
  await approve(third.taskId, { text: '中'.repeat(2730) });
  assert.equal((await taskData(third.taskId)).state, 'completed');
});

test('rejectReply: nothing is posted, and the draft is deleted with the wait', options, async () => {
  const { taskId } = await heldAsked();
  const before = await chatMessages(trip);
  assert.deepEqual(await reject(taskId), { taskId, state: 'declined', declineReason: 'approval_rejected' });
  const task = await taskData(taskId);
  assert.deepEqual([task.state, task.declineReason, task.approval, task.finishedAt], ['declined', 'approval_rejected', { decision: 'rejected', by: { uid: owner.uid, generation: 1 }, at: now }, now]);
  assert.equal((await draftRef(taskId).get()).exists, false);
  assert.deepEqual(await chatMessages(trip), before);
  assert.doesNotMatch(JSON.stringify(await taskData(taskId)), /Station Inn/u);
  assert.equal((await execute('getTask', { taskId }, member)).state, 'declined');
  // Whoever is later is told how it ended.
  await assert.rejects(approve(taskId), { code: 'REPLY_REJECTED' });
  await assert.rejects(reject(taskId), { code: 'REPLY_REJECTED' });
  assert.deepEqual((await listed()).replies, []);
});

test('timeout: ten minutes after the hold the job declines the reply, and the view says so even before it runs', options, async () => {
  const { taskId } = await heldAsked();
  const held = now;
  // The queue never hands the job over early; if its clock is apart from ours it is tried again.
  now = held + approvalMs - 1;
  await assert.rejects(expire(taskId), /HELD_REPLY_NOT_DUE/u);
  assert.equal((await taskData(taskId)).state, 'awaiting_approval');
  // Time is up: decided already for everyone who asks, before any job.
  now = held + approvalMs;
  assert.equal((await execute('getTask', { taskId }, member)).state, 'declined');
  assert.deepEqual((await listed()).replies, []);
  await assert.rejects(approve(taskId), { code: 'REPLY_EXPIRED' });
  await assert.rejects(reject(taskId), { code: 'REPLY_EXPIRED' });
  assert.equal((await chatMessages(trip)).length, 1, 'the message that asked, and nothing else');
  assert.deepEqual(await expire(taskId), { ended: true });
  const task = await taskData(taskId);
  assert.deepEqual([task.state, task.declineReason, task.finishedAt], ['declined', 'approval_timeout', now]);
  assert.equal((await draftRef(taskId).get()).exists, false);
  // Once more is nothing, and what happened is what a late decision hears.
  assert.deepEqual(await expire(taskId), { ended: false });
  await assert.rejects(approve(taskId), { code: 'REPLY_EXPIRED' });
});

test('a held run that left an operation of unknown outcome still needs a person to check it, however its reply ends', options, async () => {
  // Timed out, asked for.
  const asked = await ask(member, trip, 1), askedStart = await start(trip, asked);
  await taskRef(asked).update({ unverifiedOperations: 1 });
  await hold(askedStart);
  // Superseded, nobody asked for it, so nobody was shown its task until now.
  await say(member, lunch, 1);
  const unaskedId = taskOf(lunch, 1), unaskedStart = await start(lunch, unaskedId);
  await taskRef(unaskedId).update({ unverifiedOperations: 1 });
  await hold(unaskedStart);
  await say(other, lunch, 2, 'Thanks anyway.');
  const superseded = await taskData(unaskedId);
  assert.deepEqual([superseded.state, superseded.reconciledState, superseded.declineReason, superseded.visibility], ['needs_reconciliation', 'declined', 'superseded', 'shown']);
  const held = now;
  now = held + approvalMs;
  assert.equal((await execute('getTask', { taskId: asked }, member)).state, 'needs_reconciliation', 'before the job runs too');
  assert.deepEqual(await expire(asked), { ended: true });
  const timedOut = await taskData(asked);
  assert.deepEqual([timedOut.state, timedOut.reconciledState, timedOut.declineReason, timedOut.visibility], ['needs_reconciliation', 'declined', 'approval_timeout', 'asked']);
});

test('timeout: a reply that was decided is left alone by the job', options, async () => {
  const approved = await heldAsked(1), rejected = await heldAsked(2);
  await approve(approved.taskId); await reject(rejected.taskId);
  now += approvalMs;
  assert.deepEqual(await expire(approved.taskId), { ended: false });
  assert.deepEqual(await expire(rejected.taskId), { ended: false });
  assert.deepEqual(await expire('task_does_not_exist'), { ended: false });
  assert.deepEqual([(await taskData(approved.taskId)).state, (await taskData(rejected.taskId)).declineReason], ['completed', 'approval_rejected']);
});

test('a held task is not touched by the deadline logic of the run: it waits for the maintainer for the whole of its time', options, async () => {
  const { taskId } = await heldAsked();
  // Well past the run budget (five minutes) and past the queue limit, but within the ten minutes of the approval.
  now += 9 * 60 * 1000;
  assert.equal((await execute('getTask', { taskId }, member)).state, 'awaiting_approval');
  assert.deepEqual((await execute('listHostTasks', host)).tasks, [], 'the host is offered nothing to do for it');
  assert.deepEqual(await execute('startTask', { ...host, taskId, expectedLastChangeSeq: 0 }), { execute: false, taskId });
  assert.equal((await taskData(taskId)).state, 'awaiting_approval');
  assert.equal((await approve(taskId)).state, 'completed');
});

test('supersede: a new message of the Chat ends a held reply nobody asked for; a reply somebody asked for waits on', options, async () => {
  // The asked reply is held first: a member asking the Bot is itself a new message, which would have ended an unasked reply held before it.
  const asked = await heldAsked(1), unasked = await heldUnasked(2);
  assert.deepEqual((await listed()).replies.map(row => [row.taskId, row.unasked]).sort(), [[unasked.taskId, true], [asked.taskId, false]].sort());
  assert.equal((await taskData(unasked.taskId)).visibility, 'hidden', 'nobody asked, so the Chat is not shown that a reply waits');
  assert.deepEqual((await execute('listTasks', { conversationId: trip }, member)).tasks.map(row => row.taskId), [asked.taskId]);
  // A message of another Chat changes nothing here.
  await say(member, lunch, 9);
  assert.equal((await taskData(unasked.taskId)).state, 'awaiting_approval');
  // A member writes again in the Chat: the unasked reply is declined for it, the asked one is not.
  now += 1000;
  await say(other, trip, 3);
  const task = await taskData(unasked.taskId);
  assert.deepEqual([task.state, task.declineReason, task.finishedAt], ['declined', 'superseded', now]);
  assert.equal((await draftRef(unasked.taskId).get()).exists, false);
  assert.equal((await taskData(asked.taskId)).state, 'awaiting_approval');
  assert.deepEqual((await listed()).replies.map(row => row.taskId), [asked.taskId]);
  await assert.rejects(approve(unasked.taskId), { code: 'REPLY_SUPERSEDED' });
  await assert.rejects(reject(unasked.taskId), { code: 'REPLY_SUPERSEDED' });
  assert.equal((await chatMessages(trip)).filter(row => row.author.kind === 'shared_bot').length, 0, 'nothing of the Bot was posted');
  // Asking the Bot, or anyone writing to it, is a new message too, and still leaves the asked reply to its maintainer.
  await ask(member, trip, 4);
  assert.equal((await taskData(asked.taskId)).state, 'awaiting_approval');
  assert.equal((await approve(asked.taskId)).state, 'completed');
});

test('supersede: a Bot\'s own message and a message of another Chat leave it; a member asking a Bot ends it', options, async () => {
  const held = await heldUnasked(1);
  // The other Bot answers the Chat, and the member writes to the Bot DM: neither is a new message of this Chat by a member.
  const otherStarted = await start(trip, taskOf(trip, 1, otherBotId), member, memberHost);
  await execute('completeTask', result(otherStarted, { text: 'Lunch is at noon.' }, memberHost), member);
  assert.equal((await chatMessages(trip)).filter(row => row.author.kind === 'shared_bot').length, 1);
  await ask(member, dm, 1);
  assert.equal((await taskData(held.taskId)).state, 'awaiting_approval');
  // A member asking the other Bot is a message of this Chat: it goes through `submit`, not the ordinary message path.
  await ask(other, trip, 2, otherBotId);
  assert.deepEqual([(await taskData(held.taskId)).state, (await taskData(held.taskId)).declineReason], ['declined', 'superseded']);
});

test('supersede: a member\'s answer to a Bot\'s question is a new message of the Chat too', options, async () => {
  const question = await ask(member, trip, 1, otherBotId), asked = await start(trip, question, member, memberHost);
  const waitId = 'wait_held_0001';
  await execute('pauseTask', { ...memberHost, taskId: question, permitId: asked.permitId, waitId, requestDigest: 'a'.repeat(64), checkpointDigest: 'b'.repeat(64),
    pausedAt: now, expiresInMs: 600000, dispatchedRequests: 1, inputGrantIds: asked.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' },
    kind: 'human_input', question: 'Which day?' }, member);
  assert.equal((await taskData(question)).state, 'waiting');
  // The owner's Bot has a reply nobody asked for, held after the question was asked; the member answers.
  const held = await heldUnasked(2);
  assert.equal((await taskData(held.taskId)).state, 'awaiting_approval');
  await execute('answerWait', { conversationId: trip, waitId, messageId: 'answer_msg_0001', changeId: 'answer_chg_0001', text: 'Tuesday' }, member);
  assert.deepEqual([(await taskData(held.taskId)).state, (await taskData(held.taskId)).declineReason], ['declined', 'superseded']);
  assert.equal((await draftRef(held.taskId).get()).exists, false);
});

test('supersede: a reply to a Bot DM, which the member asked for, is never superseded; it is decided or it times out', options, async () => {
  const taskId = await ask(member, dm, 1), started = await start(dm, taskId);
  await hold(started);
  assert.equal((await taskData(taskId)).visibility, 'asked');
  assert.equal((await listed({ conversationId: dm })).replies[0].unasked, false);
  // The member writes again in the DM.
  await ask(member, dm, 2);
  assert.equal((await taskData(taskId)).state, 'awaiting_approval');
  assert.equal((await approve(taskId)).state, 'completed');
  assert.equal((await chatMessages(dm)).filter(row => row.author.kind === 'shared_bot')[0].text, draft);
});

const botMessages = async (chat, taskId) => (await chatMessages(chat)).filter(row => row.author.kind === 'shared_bot' && (!taskId || row.taskId === taskId));

test('races: approving and rejecting at once decide a reply once, and the one that lost is told how', options, async () => {
  for (const n of [1, 2, 3, 4, 5, 6]) {
    const { taskId } = await heldAsked(n);
    const settled = await Promise.allSettled([approve(taskId), reject(taskId)]);
    const lost = settled.filter(row => row.status === 'rejected'), task = await taskData(taskId);
    assert.equal(settled.length - lost.length, 1, `round ${n}: exactly one decision stands`);
    if (task.state === 'completed') {
      assert.equal(lost[0].reason.code, 'REPLY_APPROVED', `round ${n}`);
      assert.equal((await botMessages(trip, taskId)).length, 1);
    } else {
      assert.deepEqual([task.state, task.declineReason, lost[0].reason.code], ['declined', 'approval_rejected', 'REPLY_REJECTED'], `round ${n}`);
      assert.equal((await botMessages(trip, taskId)).length, 0);
    }
    assert.equal((await draftRef(taskId).get()).exists, false);
  }
});

test('races: approving at the moment the time is up, or at the moment a member writes again, is decided by whoever commits first', options, async () => {
  for (const n of [1, 2, 3, 4]) {
    const { taskId } = await heldAsked(n), expiresAt = now + approvalMs;
    const early = new SharedBotsStore(db, () => expiresAt - 1);
    const settled = await Promise.allSettled([early.execute(owner, command('approveReply', { taskId })), expireHeldReply(db, () => expiresAt, { teamId, taskId })]);
    const task = await taskData(taskId);
    if (task.state === 'completed') assert.deepEqual([settled[0].status, settled[1].value], ['fulfilled', { ended: false }], `round ${n}`);
    else assert.deepEqual([task.declineReason, settled[0].reason?.code, settled[1].value], ['approval_timeout', 'REPLY_EXPIRED', { ended: true }], `round ${n}`);
    assert.equal((await botMessages(trip, taskId)).length, task.state === 'completed' ? 1 : 0);
  }
  for (const n of [11, 12, 13, 14]) {
    const { taskId } = await heldUnasked(n, lunch);
    const settled = await Promise.allSettled([approve(taskId), say(member, lunch, n + 10)]);
    const task = await taskData(taskId);
    assert.equal(settled[1].status, 'fulfilled', 'the member\'s message is posted either way');
    if (task.state === 'completed') assert.equal(settled[0].status, 'fulfilled', `round ${n}`);
    else assert.deepEqual([task.declineReason, settled[0].reason.code], ['superseded', 'REPLY_SUPERSEDED'], `round ${n}`);
    assert.equal((await botMessages(lunch, taskId)).length, task.state === 'completed' ? 1 : 0);
    assert.equal((await draftRef(taskId).get()).exists, false);
  }
});

test('idempotency: a repeated command is answered as it was, a command id is one request, and a late command is told how it ended', options, async () => {
  const taskId = await ask(member, trip, 1), started = await start(trip, taskId);
  const first = await hold(started, {}, owner, 'command_hold_001');
  assert.deepEqual(await hold(started, {}, owner, 'command_hold_001'), first, 'the same command: its receipt');
  assert.deepEqual(await hold(started, {}, owner, 'command_hold_002'), first, 'a host that sent it again under another id: the task has the result');
  await assert.rejects(hold(started, { text: 'Another reply.' }, owner, 'command_hold_003'), { code: 'TASK_RESULT_CONFLICT' });
  await assert.rejects(hold(started, { text: 'Another reply.' }, owner, 'command_hold_001'), { code: 'COMMAND_ID_CONFLICT' });
  assert.deepEqual(await complete(started), first, 'the same result as a completion is the task as it is: nothing is posted');
  assert.equal((await botMessages(trip)).length, 0);
  assert.equal((await db.collection(`teams/${teamId}/heldReplies`).get()).size, 1);
  // Approval: at once twice, then again, and with another id.
  const both = await Promise.all([approve(taskId, { text: 'Edited.' }, owner, 'command_approve_01'), approve(taskId, { text: 'Edited.' }, owner, 'command_approve_01')]);
  assert.deepEqual(both[0], both[1]);
  assert.equal((await botMessages(trip)).length, 1, 'posted once');
  assert.deepEqual(await approve(taskId, { text: 'Edited.' }, owner, 'command_approve_01'), both[0]);
  await assert.rejects(approve(taskId, { text: 'Not that.' }, owner, 'command_approve_01'), { code: 'COMMAND_ID_CONFLICT' });
  await assert.rejects(approve(taskId, { text: 'Edited.' }, owner, 'command_approve_02'), { code: 'REPLY_APPROVED' });
  await assert.rejects(reject(taskId, owner, 'command_reject_01'), { code: 'REPLY_APPROVED' });
  // The host that was answered late is told the task as it stands.
  assert.deepEqual(await hold(started, {}, owner, 'command_hold_004'), { taskId, state: 'completed', replyId: both[0].replyId, artifactId: null });
  // Rejection.
  const second = await heldAsked(2), rejected = await reject(second.taskId, owner, 'command_reject_02');
  assert.deepEqual(await reject(second.taskId, owner, 'command_reject_02'), rejected);
  await assert.rejects(reject(second.taskId, owner, 'command_reject_03'), { code: 'REPLY_REJECTED' });
  await assert.rejects(approve(second.taskId, {}, owner, 'command_approve_03'), { code: 'REPLY_REJECTED' });
  await assert.rejects(reject(second.taskId, member), { code: 'MAINTAINER_REQUIRED' });
  assert.equal((await botMessages(trip)).length, 1);
});

test('cancelTask: the member who asked, or the maintainer, withdraws a held reply with its task, and the draft goes', options, async () => {
  const a = await heldAsked(1), b = await heldAsked(2);
  await assert.rejects(execute('cancelTask', { taskId: a.taskId }, other), { code: 'TASK_CANCEL_FORBIDDEN' });
  assert.deepEqual(await execute('cancelTask', { taskId: a.taskId }, member), { taskId: a.taskId, state: 'cancelled' });
  assert.deepEqual(await execute('cancelTask', { taskId: b.taskId }, owner), { taskId: b.taskId, state: 'cancelled' });
  for (const { taskId } of [a, b]) {
    assert.equal((await taskData(taskId)).state, 'cancelled');
    assert.equal((await draftRef(taskId).get()).exists, false);
    await assert.rejects(approve(taskId), { code: 'REPLY_NOT_HELD' });
    await assert.rejects(execute('cancelTask', { taskId }, owner), { code: 'TASK_FINISHED' });
  }
  assert.deepEqual((await listed()).replies, []);
  assert.equal((await botMessages(trip)).length, 0);
});

test('holdReply: a result that would not be posted is not held, and ends exactly as completeTask would end it', options, async () => {
  // A refusal: the audience the host names is not the published inputs. Asked, the notice is posted; nobody asked, nothing is.
  const asked = await ask(member, trip, 1), a = await start(trip, asked);
  const refused = await hold(a, { outputAudience: { mode: 'owner' } });
  assert.deepEqual([refused.state, refused.artifactId, typeof refused.replyId], ['authorization_required', null, 'string']);
  assert.match((await botMessages(trip, asked))[0].text, /授权/u);
  await say(member, trip, 2);
  const unasked = taskOf(trip, 2), u = await start(trip, unasked);
  assert.deepEqual(await hold(u, { outputAudience: { mode: 'owner' } }), { taskId: unasked, state: 'authorization_required', replyId: null, artifactId: null });
  assert.equal((await botMessages(trip)).length, 1);
  // A message edited while the Bot worked: asked, the notice; unasked, silence.
  await say(member, trip, 3);
  const edited = taskOf(trip, 3), e = await start(trip, edited);
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(trip, 3), changeId: 'chg_edit_0003', operation: 'edit', text: 'Never mind.' });
  assert.deepEqual(await hold(e, {}), { taskId: edited, state: 'declined', replyId: null, artifactId: null });
  assert.equal((await taskData(edited)).declineReason, 'revoked');
  // The Chat stopped letting the Bot answer unasked.
  await say(member, trip, 4);
  const stopped = taskOf(trip, 4), s = await start(trip, stopped);
  await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 1, targets: [otherBotId] }, member);
  assert.deepEqual(await hold(s, {}), { taskId: stopped, state: 'declined', replyId: null, artifactId: null });
  // A task that was canceled while it ran.
  const running = await ask(member, trip, 5), r = await start(trip, running);
  await execute('cancelTask', { taskId: running }, member);
  assert.deepEqual(await hold(r), { taskId: running, state: 'cancelled', replyId: null, artifactId: null });
  assert.equal((await db.collection(`teams/${teamId}/heldReplies`).get()).size, 0, 'none of them was held');
  assert.deepEqual((await listed()).replies, []);
});

test('holdReply: only a completed reply with text, of a task that is not private, is held', options, async () => {
  const taskId = await ask(member, trip, 1), started = await start(trip, taskId);
  for (const change of [{ outcome: 'failed' }, { outcome: 'declined', text: '' }, { text: '' }, { privateData: true }, { text: 'x'.repeat(8193) }, { revises: { artifactId: 'artifact_0001', expectedRevision: 1 } },
    { unknown: true }]) await assert.rejects(hold(started, change), /./u, JSON.stringify(change));
  assert.equal((await taskData(taskId)).state, 'running');
  // A private-data run keeps what it keeps today: its result is never a reply.
  await taskRef(taskId).update({ privateDataSource: 'source_0001' });
  await assert.rejects(hold(started), { code: 'DATA_PRIVATE_TASK' });
  await taskRef(taskId).update({ privateDataSource: null });
  // Not the permit of this run, nor its host, nor a Bot that is not this one's maintainer's.
  await assert.rejects(hold(started, { permitId: 'permit_of_another_run' }), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(hold(started, {}, member), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(execute('holdReply', { ...result(started), hostId: 'host_other' }), { code: 'DEPLOYMENT_REQUIRED' });
  assert.equal((await taskData(taskId)).state, 'running');
  assert.equal((await hold(started)).state, 'awaiting_approval');
});

test('approveReply: the reply is checked again when it is approved, and what changed since it was held ends it as a late result would end', options, async () => {
  // The message that asked was edited while the reply waited: the task is revoked, and nothing is posted, whether somebody asked or not.
  const asked = await heldAsked(1);
  await edit(trip, 1, 'Another question.');
  assert.deepEqual(await approve(asked.taskId, { text: 'Approved.' }), { taskId: asked.taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  assert.equal((await draftRef(asked.taskId).get()).exists, false);
  const unasked = await heldUnasked(2);
  await edit(trip, 2, 'Never mind.');
  assert.deepEqual(await approve(unasked.taskId), { taskId: unasked.taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  assert.equal((await botMessages(trip)).length, 0);
  // A message from before it, which the Bot was given, was edited: the reply is out of date. Asked, the Chat is told to ask again; nobody asked, nothing is posted.
  await say(member, trip, 3);
  const later = await heldAsked(4);
  await edit(trip, 3, 'Something else.');
  const stale = await approve(later.taskId, { text: 'Approved.' });
  assert.deepEqual([stale.state, stale.artifactId, typeof stale.replyId], ['failed', null, 'string']);
  assert.match((await botMessages(trip))[0].text, /请重新提问/u);
  assert.doesNotMatch(JSON.stringify(await botMessages(trip)), /Approved\.|Station Inn/u, 'the reply is not posted');
  assert.equal((await draftRef(later.taskId).get()).exists, false);
  await say(member, trip, 5);
  const quiet = await heldUnasked(6);
  await edit(trip, 5, 'Something else again.');
  assert.deepEqual(await approve(quiet.taskId), { taskId: quiet.taskId, state: 'failed', replyId: null, artifactId: null });
  assert.equal((await botMessages(trip)).length, 1, 'only the notice of the one that was asked');
  // A requester who is no longer a member of the Team: the task is revoked.
  const left = await heldAsked(7);
  await db.doc(`teams/${teamId}/members/${member.uid}`).update({ generation: 2 });
  assert.deepEqual(await approve(left.taskId), { taskId: left.taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  assert.equal((await draftRef(left.taskId).get()).exists, false);
  assert.equal((await taskData(left.taskId)).declineReason, 'revoked');
});

test('approveReply: a Bot that was republished or paused since the reply was held no longer has it posted', options, async () => {
  const a = await heldAsked(1);
  const published = publication(botId, host, owner);
  await execute('publishSharedBot', { ...published, expectedRevision: 1, deployment: { ...published.deployment, revision: 2 } });
  assert.deepEqual(await approve(a.taskId), { taskId: a.taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  assert.equal((await botMessages(trip)).length, 0);
});

test('listTriage: a reply held for approval says how it went, held, approved, rejected, timed out or superseded', options, async () => {
  const triaged = async (chat, n) => {
    await say(member, chat, n);
    const taskId = taskOf(chat, n);
    await execute('recordTriage', { ...host, taskId, action: 'run_may_decline', source: 'decision', reason: 'between_thresholds', probability: 0.5 });
    now += 1000;
    return { taskId, started: await start(chat, taskId) };
  };
  const rows = async () => Object.fromEntries((await log()).items.map(item => [item.taskId, [item.state, item.approval]]));
  // One of each, in the order they come to be: the later ones are held while the earlier ones are decided.
  const approved = await triaged(trip, 1); await hold(approved.started); await approve(approved.taskId);
  const rejected = await triaged(trip, 2); await hold(rejected.started); await reject(rejected.taskId);
  const timeout = await triaged(trip, 3); await hold(timeout.started); now += approvalMs; await expire(timeout.taskId); await beat();
  const superseded = await triaged(trip, 4); await hold(superseded.started); await say(other, trip, 5);
  const waiting = await triaged(trip, 6); await hold(waiting.started);
  const plain = await triaged(lunch, 7); await complete(plain.started);
  assert.deepEqual(await rows(), {
    [approved.taskId]: ['completed', 'approved'], [rejected.taskId]: ['declined', 'rejected'], [timeout.taskId]: ['declined', 'timeout'],
    [superseded.taskId]: ['declined', 'superseded'], [waiting.taskId]: ['awaiting_approval', 'held'], [plain.taskId]: ['completed', undefined],
  });
  // A held reply whose time is up before any job ran is already a timeout, and nothing of the reply is in the log.
  now += approvalMs;
  assert.deepEqual((await rows())[waiting.taskId], ['declined', 'timeout']);
  assert.doesNotMatch(JSON.stringify(await log()), /Station Inn/u);
});

test('HeldReplyJobs: a held reply is scheduled for the end of its time and its maintainer is told once, without a word of it', options, async () => {
  const device = (uid, locale) => ({ id: `device_${uid}`, uid, revision: 1, expiresAt: { toMillis: () => now + 3600000 }, fcmInstallationId: `fid_${uid}`, appId: 'com.lingxi.code', locale });
  const devices = { owner: [device('owner', 'en'), { ...device('owner', 'zh-Hans'), id: 'device_owner_zh', fcmInstallationId: 'fid_owner_zh' }], member: [device('member', 'en')] };
  const pushStore = { async active(uid) { return devices[uid] ?? []; }, device(id) { return { async get() { return { data: () => Object.values(devices).flat().find(row => row.id === id) }; } }; } };
  const sent = [], queued = [], messaging = { async send(message) { sent.push(message); } };
  const jobs = new HeldReplyJobs({ db, pushStore, messaging, enqueue: async (payload, id, delay) => { queued.push({ payload, id, delay }); }, clock: () => now });
  const { taskId } = await heldUnasked(1);
  await jobs.held(teamId, taskId);
  assert.equal(queued.length, 1);
  assert.deepEqual(queued[0].payload, { type: 'heldReply', teamId, taskId });
  assert.match(queued[0].id, /^[a-f0-9]{64}$/u);
  assert.equal(queued[0].delay, approvalMs / 1000 + 1, 'a second after the deadline');
  // The maintainer on each of their devices, in the device\'s language, with the Bot\'s name and the Chat to open, and nothing of the reply.
  assert.deepEqual(sent.map(message => [message.fid, message.data.title, message.data.body, message.data.route, message.data.kind]), [
    ['fid_owner', 'Travel bot', 'A reply is waiting for your approval', `team/chat?conversation=${trip}&team=${teamId}`, 'reply_approval'],
    ['fid_owner_zh', 'Travel bot', '有一条回复等待你批准', `team/chat?conversation=${trip}&team=${teamId}`, 'reply_approval']]);
  assert.doesNotMatch(JSON.stringify(sent), /Station Inn|hotel|quiet/iu);
  // Run again, as a trigger that is retried: the same job id, and nobody is told twice.
  await jobs.held(teamId, taskId);
  assert.equal(queued.length, 2);
  assert.equal(queued[1].id, queued[0].id);
  assert.equal(sent.length, 2);
  assert.equal((await chatRef(trip).collection('pushAttempts').get()).size, 1);
  // The job at the deadline ends the reply; before it, a reply nobody decided is still held.
  const asJob = new HeldReplyJobs({ db, pushStore, messaging, enqueue: async () => {}, clock: () => now });
  now += approvalMs;
  await asJob.expire({ type: 'heldReply', teamId, taskId });
  assert.deepEqual([(await taskData(taskId)).state, (await taskData(taskId)).declineReason], ['declined', 'approval_timeout']);
  await beat();
  // A reply decided before the trigger ran is neither scheduled nor announced.
  const decided = await heldAsked(2);
  await reject(decided.taskId);
  await jobs.held(teamId, decided.taskId);
  assert.equal(queued.length, 2);
  assert.equal(sent.length, 2);
  // Nobody is told of a reply whose maintainer no longer reads the Chat.
  const away = await heldAsked(3);
  await chatRef(trip).update({ memberUids: (await chatRef(trip).get()).data().memberUids.filter(uid => uid !== owner.uid) });
  await jobs.held(teamId, away.taskId);
  assert.equal(sent.length, 2);
});
