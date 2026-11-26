// Bot v3 B8: a Bot hands part of a request to another Bot of the same Chat. `delegateTask` asks and goes on; `pauseTask` of kind `delegation` asks and waits for the
// reply, which the other Bot's task hands back instead of posting it. Both Bots have to agree, the person's own rights decide, a chain is two hops long, a root task
// makes three delegations, and the person and both maintainers can end it.
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { admissionId } from '../src/team-execution-contract.mjs';
import { delegationId, delegationWaitMs, delegationsPerRoot } from '../src/delegations.mjs';
import { approvalMs, expireHeldReply } from '../src/held-replies.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-delegation', teamId = 'team_deleg_01', trip = 'chat_deleg_trip', small = 'chat_deleg_small';
const [botA, botB, botC, botD] = ['sharedbot_alpha', 'sharedbot_beta', 'sharedbot_gamma', 'sharedbot_delta'];
// The person who asks, a member who only reads, the maintainer of Alpha, Gamma and Delta, and the maintainer of Beta.
const asker = { uid: 'asker', name: 'Asker' }, bystander = { uid: 'bystander', name: 'Bystander' };
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' };
const bots = {
  [botA]: { name: 'Alpha', maintainer: owner, host: { deploymentId: 'deployment_alpha', hostId: 'host_alpha_01', accountEpoch: 1 } },
  [botB]: { name: 'Beta', maintainer: member, host: { deploymentId: 'deployment_beta', hostId: 'host_beta_01', accountEpoch: 1 } },
  [botC]: { name: 'Gamma', maintainer: owner, host: { deploymentId: 'deployment_gamma', hostId: 'host_gamma_01', accountEpoch: 1 } },
  [botD]: { name: 'Delta', maintainer: owner, host: { deploymentId: 'deployment_delta', hostId: 'host_delta_01', accountEpoch: 1 } },
};
// Every Bot takes requests from any Bot of the Team. Alpha asks Beta and Gamma, Beta asks Gamma and Alpha, Gamma asks Delta.
const defaults = {
  [botA]: { acceptDelegation: 'team_bots', delegateTo: [botB, botC], delegation: { daily: 10 } },
  [botB]: { acceptDelegation: 'team_bots', delegateTo: [botC, botA], delegation: { daily: 10 } },
  [botC]: { acceptDelegation: 'team_bots', delegateTo: [botD], delegation: { daily: 10 } },
  [botD]: { acceptDelegation: 'team_bots' },
};
const asking = 'Plan the trip, and find out what the 2026 figures were.', reply = 'Revenue was 12.4 million.';
const checkpoint = 'd'.repeat(64), day = 24 * 60 * 60 * 1000, waitFor = n => `wait_deleg_${n}`, waitId = waitFor(1);
let app, db, teams, store, messages, now, serial = 0;

const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const taskRef = id => db.doc(`teams/${teamId}/tasks/${id}`);
const taskData = async id => (await taskRef(id).get()).data();
const waitData = async id => (await db.doc(`teams/${teamId}/waits/${id}`).get()).data();
const delegationData = async id => (await db.doc(`teams/${teamId}/delegations/${id}`).get()).data();
const botData = async id => (await db.doc(`teams/${teamId}/sharedBots/${id}`).get()).data();
const chatRef = id => db.doc(`teams/${teamId}/conversations/${id}`);
const chatMessages = async chat => (await chatRef(chat).collection('messages').orderBy('seq').get()).docs.map(doc => ({ id: doc.id, ...doc.data() }));
const count = async name => (await db.collection(`teams/${teamId}/${name}`).get()).size;
const messageId = (chat, n) => `msg_${chat.slice(-4)}_${String(n).padStart(4, '0')}`;
const lines = async chat => (await chatMessages(chat)).map(message => `${message.author.sharedBotId ?? message.author.uid}: ${message.text}`);
const execute = async (action, payload, actor = owner, commandId) => {
  // A start is at the Chat as it stands, as the host reads it.
  if (action === 'startTask' && !Object.hasOwn(payload, 'expectedLastChangeSeq')) {
    const conversationId = (await taskData(payload.taskId)).conversationId;
    payload = { ...payload, expectedLastChangeSeq: (await chatRef(conversationId).get()).data().lastChangeSeq };
  }
  return store.execute(actor, command(action, payload, commandId));
};
const publishBot = async (id, extra = {}, capabilities = { durableWait: true, writeOperations: false, delegation: true }) => {
  const { name, host, maintainer } = bots[id], revision = (await botData(id))?.identity.revision ?? 0;
  await execute('publishSharedBot', { sharedBotId: id, expectedRevision: revision, name, description: `${name} helps with the trip.`, usePolicy: { mode: 'all' },
    deployment: { ...host, sharedBotId: id, revision: revision + 1, owner: { uid: maintainer.uid, generation: 1 }, instanceId: `instance_${id}`,
      definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2, capabilities, enabled: true },
    inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
    quotas: { team: 100, member: 20, bot: 100 }, ...extra }, maintainer);
  await execute('heartbeatDeployment', { ...host, ready: true }, maintainer);
};
/** Every host reports again: a clock that moved on is a host that stayed online. */
const beat = async (...ids) => { for (const id of ids.length ? ids : Object.keys(bots)) await execute('heartbeatDeployment', { ...bots[id].host, ready: true }, bots[id].maintainer); };
/** A person asks the Bots named (Alpha by default) in a Chat; the first task is returned. */
const ask = async (n, targets = [botA], chat = trip, actor = asker) => (await execute('submit', { conversationId: chat, messageId: messageId(chat, n),
  changeId: `chg_${chat.slice(-4)}_${String(n).padStart(4, '0')}`, text: asking, targets }, actor)).tasks[0].taskId;
const startTask = (taskId, id) => execute('startTask', { ...bots[id].host, taskId }, bots[id].maintainer);
const runA = async (n = 1, chat = trip) => { const taskId = await ask(n, [botA], chat); return { taskId, started: await startTask(taskId, botA) }; };
const stepOf = (taskId, n = 1) => `${taskId}/step_${n}`;
const grantsOf = started => started.inputs.map(grant => grant.grantId);
/** The run of `id` asks `target` and goes on. */
const delegate = (started, id, target, { step = 1, request = 'Find the 2026 figures.' } = {}) => execute('delegateTask', { ...bots[id].host, taskId: started.task.taskId,
  permitId: started.permitId, logicalStepId: stepOf(started.task.taskId, step), target, request, inputGrantIds: grantsOf(started), outputAudience: { mode: 'published_inputs' } }, bots[id].maintainer);
/** The run of `id` asks `target` and waits for the reply. */
const pauseOn = (started, id, target, { step = 1, request = 'Find the 2026 figures.', wait = waitId, ...override } = {}) => execute('pauseTask', { ...bots[id].host, taskId: started.task.taskId,
  permitId: started.permitId, waitId: wait, kind: 'delegation', delegation: { logicalStepId: stepOf(started.task.taskId, step), target, request }, requestDigest: 'c'.repeat(64),
  checkpointDigest: checkpoint, pausedAt: now, expiresInMs: delegationWaitMs, dispatchedRequests: 2, inputGrantIds: grantsOf(started), outputAudience: { mode: 'published_inputs' }, ...override },
  bots[id].maintainer);
const resultOf = (started, id, text, more = {}) => ({ ...bots[id].host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text, finishedAt: now,
  inputGrantIds: grantsOf(started), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const complete = (started, id, text, more) => execute('completeTask', resultOf(started, id, text, more), bots[id].maintainer);
const hold = (started, id, text, more) => execute('holdReply', resultOf(started, id, text, more), bots[id].maintainer);
const detach = (id, taskId, more = {}) => execute('detachDelegation', { ...bots[id].host, taskId, waitId, ...more }, bots[id].maintainer);
const prepare = (id, taskId) => execute('prepareTask', { ...bots[id].host, taskId }, bots[id].maintainer);
const goOn = (id, taskId, more = {}) => execute('continueTask', { ...bots[id].host, taskId, expectedEpoch: 1, checkpointDigest: checkpoint, reason: 'answer', dispatchedRequests: 3, ...more }, bots[id].maintainer);
const offered = result => (result.delegates ?? []).map(entry => entry.sharedBotId).sort();
/** Alpha's run for a message, waiting for Beta's reply: the ids of what was made. */
async function waiting(n = 1) {
  const { taskId, started } = await runA(n), paused = await pauseOn(started, botA, botB, { wait: waitFor(n) }), id = delegationId(taskId, stepOf(taskId));
  return { taskId, started, paused, id, bTaskId: (await delegationData(id)).targetTaskId };
}
/** A task nobody asked for starts only once its round has chosen it. */
const win = async taskId => {
  const task = await taskData(taskId);
  if (!task.roundId || task.round) return;
  const round = (await db.doc(`teams/${teamId}/replyRounds/${task.roundId}`).get()).data();
  for (const candidate of round.candidates) await execute('bidReply', { ...bots[candidate.sharedBotId].host, taskId: candidate.taskId, tier: candidate.taskId === taskId ? 'run' : 'decline',
    source: 'rule' }, bots[candidate.sharedBotId].maintainer);
};

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'delegation-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  for (const name of ['teams', 'users']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 26); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Delegation test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, asker, bystander]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  for (const id of [botA, botB, botC, botD]) await publishBot(id, defaults[id]);
  await execute('createConversation', { conversationId: trip, kind: 'group', title: 'Trip', participantUids: [asker.uid, owner.uid, bystander.uid],
    sharedBotIds: [botA, botB, botC, botD] }, asker);
  await execute('createConversation', { conversationId: small, kind: 'group', title: 'Small', participantUids: [asker.uid], sharedBotIds: [botA, botB] }, asker);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('delegateTask: a Bot asks another Bot of the Chat as itself, the other Bot answers in the Chat, and one step is one delegation', options, async () => {
  const { taskId, started } = await runA(1), before = await chatMessages(trip);
  assert.deepEqual(offered(started), [botB, botC], 'the run is offered the Bots it may ask');
  const sent = await delegate(started, botA, botB), id = delegationId(taskId, stepOf(taskId));
  assert.deepEqual([sent.sent, sent.delegationId], [true, id]);
  // A message of the Bot's own, a reply to the one that started its run, which asks the other Bot.
  const after = await chatMessages(trip), said = after.at(-1);
  assert.equal(after.length, before.length + 1);
  assert.deepEqual([said.id, said.author, said.text, said.replyTo, said.taskIds, said.mentions],
    [sent.messageId, { kind: 'shared_bot', sharedBotId: botA, displayName: 'Alpha' }, '@Beta Find the 2026 figures.', messageId(trip, 1), [sent.taskId], [`shared_bot:${botB}`]]);
  // The other Bot's task is an asked task of the same person, caused by the first, and shown in its maintainer's log as addressed to it.
  const task = await taskData(sent.taskId);
  assert.deepEqual([task.sharedBotId, task.state, task.requester, task.rootTaskId, task.optional], [botB, 'queued', { uid: asker.uid, generation: 1 }, taskId, undefined]);
  assert.deepEqual(task.delegation, { delegationId: id, rootTaskId: taskId, parentTaskId: taskId, parentBotId: botA, depth: 1,
    chain: [{ sharedBotId: botA, name: 'Alpha' }, { sharedBotId: botB, name: 'Beta' }], expectReply: false });
  assert.deepEqual(task.triage, { action: 'run', source: 'rule', reason: 'addressed', at: now });
  const record = await delegationData(id);
  assert.deepEqual([record.parentTaskId, record.parentBotId, record.rootTaskId, record.targetBotId, record.targetTaskId, record.conversationId, record.messageId, record.expectReply,
    record.depth, record.state, record.waitId, record.createdAt], [taskId, botA, taskId, botB, sent.taskId, trip, sent.messageId, false, 1, 'open', null, now]);
  const row = (await execute('listTriage', { sharedBotId: botB }, member)).items[0];
  assert.deepEqual([row.taskId, row.triage.reason, row.delegation.chain.length], [sent.taskId, 'addressed', 2]);
  const view = await execute('getTask', { taskId: sent.taskId }, asker);
  assert.deepEqual(view.delegation.chain.map(link => link.name), ['Alpha', 'Beta']);
  // No round and no auto-reply, and the cost is the delegating Bot's own daily limit, not the person's.
  assert.equal(await count('replyRounds'), 0);
  const quotas = (await db.collection(`teams/${teamId}/requestQuotas`).get()).docs.map(doc => doc.data());
  assert.deepEqual(quotas.filter(quota => quota.limit === 10).map(quota => quota.used), [1]);
  assert.deepEqual(quotas.filter(quota => quota.limit === 20).map(quota => quota.used), [1], 'the person paid for their own message only');
  // The same step again is the same delegation; another request under the same step is not.
  assert.deepEqual(await delegate(started, botA, botB), sent);
  assert.equal((await chatMessages(trip)).length, before.length + 1);
  await assert.rejects(delegate(started, botA, botB, { request: 'Something else.' }), { code: 'DELEGATION_CONFLICT' });
  // The other Bot's Desktop runs the task, and its reply is posted in the Chat as a reply to the Bot's message.
  const finished = await complete(await startTask(sent.taskId, botB), botB, reply);
  assert.deepEqual([finished.state, typeof finished.replyId], ['completed', 'string']);
  const posted = (await chatMessages(trip)).at(-1);
  assert.deepEqual([posted.author.sharedBotId, posted.text, posted.replyTo], [botB, reply, sent.messageId]);
  assert.equal((await taskData(sent.taskId)).handedBack, undefined);
});

test('delegateTask: both Bots have to agree, the other Bot has to be in the Chat, and both Desktops have to run the tool', options, async () => {
  const { started } = await runA(1);
  let step = 0;
  const tryAsk = (target, id = botA) => delegate(started, id, target, { step: ++step });
  const deployment = id => db.doc(`teams/${teamId}/deployments/${bots[id].host.deploymentId}`);
  // The other Bot takes requests from nobody, or only from the Bots it lists.
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'none' });
  assert.deepEqual(await tryAsk(botB), { sent: false, code: 'DELEGATION_NOT_ACCEPTED' });
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'listed', acceptFrom: [botC] });
  assert.deepEqual(await tryAsk(botB), { sent: false, code: 'DELEGATION_NOT_ACCEPTED' });
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'listed', acceptFrom: [botA] });
  assert.equal((await tryAsk(botB)).sent, true);
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'team_bots' });
  assert.equal((await tryAsk(botB)).sent, true);
  // The delegating Bot asks only the Bots it lists.
  assert.deepEqual(await tryAsk(botD), { sent: false, code: 'DELEGATION_NOT_ALLOWED' });
  // The Desktop of the other Bot, and the Desktop of the delegating Bot, have to report that they run the tool.
  await deployment(botB).update({ 'capabilities.delegation': false });
  assert.deepEqual(await tryAsk(botB), { sent: false, code: 'DELEGATION_UNSUPPORTED_TARGET' });
  await deployment(botB).update({ 'capabilities.delegation': true });
  await deployment(botA).update({ 'capabilities.delegation': false });
  assert.deepEqual(await tryAsk(botB), { sent: false, code: 'DELEGATION_UNSUPPORTED' });
  await deployment(botA).update({ 'capabilities.delegation': true });
  // A Bot that is paused is not available.
  await execute('pauseSharedBot', { sharedBotId: botB, expectedRevision: (await botData(botB)).identity.revision }, member);
  assert.deepEqual(await tryAsk(botB), { sent: false, code: 'DELEGATION_TARGET_UNAVAILABLE' });
  // A refusal makes nothing; the two that were sent are all there is.
  assert.equal(await count('delegations'), 2);
  // The other Bot has to be in the Chat, and a Bot that is not is not offered.
  const inSmall = await startTask(await ask(1, [botA], small), botA);
  assert.deepEqual(offered(inSmall), []);
  assert.deepEqual(await delegate(inSmall, botA, botC), { sent: false, code: 'BOT_PARTICIPANT_REQUIRED' });
});

test('delegateTask: the person who asked decides what the other Bot may be asked, not the Bot that asks for them', options, async () => {
  await publishBot(botB, { ...defaults[botB], usePolicy: { mode: 'listed', members: [{ uid: owner.uid, generation: 1 }] } });
  const { started } = await runA(1);
  assert.deepEqual(offered(started), [botC], 'a Bot the person may not use is not offered');
  assert.deepEqual(await delegate(started, botA, botB), { sent: false, code: 'BOT_USE_REQUIRED' });
  assert.equal(await count('delegations'), 0);
  // Somebody who may use it asks the same Bot, and the delegation is made as them.
  await complete(started, botA, 'Nothing more to do.');
  const second = await startTask(await ask(2, [botA], trip, owner), botA);
  assert.deepEqual(offered(second), [botB, botC]);
  const sent = await delegate(second, botA, botB);
  assert.equal(sent.sent, true);
  assert.deepEqual((await taskData(sent.taskId)).requester, { uid: owner.uid, generation: 1 });
});

test('delegateTask: a chain is two hops from the person, and a Bot already on it is not asked again', options, async () => {
  const { taskId, started } = await runA(1);
  assert.deepEqual(offered(started), [botB, botC]);
  const toB = await delegate(started, botA, botB), b = await startTask(toB.taskId, botB);
  assert.deepEqual(offered(b), [botC], 'Alpha began the chain, so Beta is not offered Alpha');
  assert.deepEqual(await delegate(b, botB, botA), { sent: false, code: 'DELEGATION_LOOP' });
  const toC = await delegate(b, botB, botC), cTask = await taskData(toC.taskId);
  assert.deepEqual([cTask.delegation.depth, cTask.rootTaskId, cTask.delegation.parentBotId], [2, taskId, botB]);
  assert.deepEqual(cTask.delegation.chain, [{ sharedBotId: botA, name: 'Alpha' }, { sharedBotId: botB, name: 'Beta' }, { sharedBotId: botC, name: 'Gamma' }]);
  const c = await startTask(toC.taskId, botC);
  assert.deepEqual(offered(c), [], 'the third Bot cannot delegate');
  assert.deepEqual(await delegate(c, botC, botD), { sent: false, code: 'DELEGATION_DEPTH' });
});

test('delegateTask: a root task makes three delegations, and a data handoff of the same root counts as one of them', options, async () => {
  assert.equal(delegationsPerRoot, 3);
  const { started } = await runA(1);
  for (const step of [1, 2, 3]) assert.equal((await delegate(started, botA, botB, { step })).sent, true);
  assert.deepEqual(await delegate(started, botA, botB, { step: 4 }), { sent: false, code: 'DELEGATION_LIMIT' });
  assert.equal(await count('delegations'), 3);
  // Another root task: a task a data handoff made under it, and two delegations are all there is room for.
  await complete(started, botA, 'Done.');
  const next = await runA(2);
  await db.doc(`teams/${teamId}/tasks/task_handoff_0001`).set({ taskId: 'task_handoff_0001', sharedBotId: botD, rootTaskId: next.taskId, state: 'completed',
    dataCause: { kind: 'handoff', quotaOwner: botA } });
  assert.equal((await delegate(next.started, botA, botB, { step: 1 })).sent, true);
  assert.equal((await delegate(next.started, botA, botB, { step: 2 })).sent, true);
  assert.deepEqual(await delegate(next.started, botA, botB, { step: 3 }), { sent: false, code: 'DELEGATION_LIMIT' });
});

test('delegateTask: the delegating Bot is charged its own daily limit, and runs out of it', options, async () => {
  await publishBot(botA, { ...defaults[botA], delegation: { daily: 2 } });
  const { started } = await runA(1);
  assert.equal((await delegate(started, botA, botB, { step: 1 })).sent, true);
  assert.equal((await delegate(started, botA, botB, { step: 2 })).sent, true);
  assert.deepEqual(await delegate(started, botA, botB, { step: 3 }), { sent: false, code: 'REQUEST_QUOTA_EXCEEDED' });
  assert.equal(await count('delegations'), 2);
});

test('a delegation that waits: the run waits, the other Bot hands its reply back instead of posting it, and the run goes on with it', options, async () => {
  const { taskId, paused, id, bTaskId } = await waiting(1);
  const record = await delegationData(id);
  assert.deepEqual(paused, { taskId, state: 'waiting', waitId, messageId: record.messageId, expiresAt: now + delegationWaitMs, holdsUntil: now + day });
  assert.deepEqual([record.expectReply, record.waitId, record.state], [true, waitId, 'open']);
  const waits = await taskData(taskId);
  assert.deepEqual([waits.state, waits.currentWaitKind, waits.currentWaitUntil, waits.deadline, waits.currentDelegation],
    ['waiting', 'delegation', now + delegationWaitMs, now + day, { sharedBotId: botB, taskId: bTaskId }]);
  assert.deepEqual([(await waitData(waitId)).kind, (await waitData(waitId)).state, (await waitData(waitId)).delegationId], ['delegation', 'pending', id]);
  assert.equal((await taskData(bTaskId)).delegation.expectReply, true);
  // The person reads it as a wait that nobody answers, and a message cannot answer it.
  const view = await execute('getTask', { taskId }, asker);
  assert.deepEqual([view.state, view.wait.kind, view.wait.expiresAt, view.wait.delegation], ['waiting', 'delegation', now + delegationWaitMs, { sharedBotId: botB, taskId: bTaskId }]);
  await assert.rejects(execute('answerWait', { conversationId: trip, waitId, messageId: 'answer_deleg_01', changeId: 'answerchg_deleg_01', text: 'Done' }, asker), { code: 'WAIT_KIND_MISMATCH' });
  // The other Bot runs and finishes: nothing of its reply enters the Chat or any projection.
  const before = await lines(trip), b = await startTask(bTaskId, botB);
  assert.deepEqual(await complete(b, botB, reply), { taskId: bTaskId, state: 'completed', replyId: null, artifactId: null });
  assert.deepEqual(await lines(trip), before);
  assert.equal((await taskData(bTaskId)).handedBack, true);
  assert.doesNotMatch(JSON.stringify([await execute('getTask', { taskId: bTaskId }, asker), await execute('listTasks', { conversationId: trip }, bystander)]), /12\.4/u);
  assert.deepEqual([(await delegationData(id)).state, (await delegationData(id)).result], ['done', { text: reply }]);
  // The waiting run is queued again, with the Chat still its own.
  const queued = await taskData(taskId);
  assert.deepEqual([queued.state, queued.resume, queued.deadline], ['queued', { waitId, delegation: id }, now + day]);
  assert.deepEqual([(await waitData(waitId)).state, (await waitData(waitId)).delegationOutcome], ['resolved', 'done']);
  // Its host is told how it ended, and the reply, before it asks to go on and again with the permit.
  const outcome = { waitId, delegation: { delegationId: id, targetBotId: botB, targetName: 'Beta', outcome: 'done', text: reply } };
  const prepared = await prepare(botA, taskId);
  assert.deepEqual(prepared.wait, outcome);
  assert.deepEqual(offered(prepared), [botB, botC]);
  const continued = await goOn(botA, taskId);
  assert.equal(continued.execute, true);
  assert.deepEqual(continued.wait, outcome);
  assert.deepEqual(offered(continued), [botB, botC]);
  await complete(continued, botA, `Per Beta, ${reply.toLowerCase()}`);
  assert.deepEqual(await lines(trip), [`${asker.uid}: ${asking}`, `${botA}: @Beta Find the 2026 figures.`, `${botA}: Per Beta, ${reply.toLowerCase()}`]);
});

test('a delegation that waits: a Desktop that is off keeps the other Bot\'s task a day, the run goes on after ten minutes, and the reply is posted when it comes', options, async () => {
  // Only Alpha is online when the message is sent: Beta's Desktop is off.
  now += 10 * 60 * 1000; await beat(botA);
  const { taskId, started } = await runA(1), paused = await pauseOn(started, botA, botB), id = delegationId(taskId, stepOf(taskId));
  const bTaskId = (await delegationData(id)).targetTaskId, bTask = await taskData(bTaskId);
  assert.deepEqual([bTask.hostWait, bTask.deadline], [true, now + day]);
  assert.equal((await execute('getTask', { taskId: bTaskId }, asker)).hostWait, true);
  // The time is the service's: before it is up the host is told how long is left.
  now += delegationWaitMs - 1000;
  await assert.rejects(detach(botA, taskId), { code: 'DELEGATION_WAIT_ACTIVE' });
  now += 1000; await beat();
  assert.equal(paused.expiresAt, now, 'the wait ends ten minutes after it began');
  assert.deepEqual(await detach(botA, taskId), { taskId, state: 'queued', outcome: 'detached' });
  assert.deepEqual(await detach(botA, taskId), { taskId, state: 'queued', outcome: 'detached' }, 'asking again finds it decided');
  assert.deepEqual([(await delegationData(id)).state, (await taskData(taskId)).resume], ['detached', { waitId, delegation: id }]);
  // Beta's Desktop comes back and answers: the run no longer waits, so the reply goes to the Chat like any other.
  const done = await complete(await startTask(bTaskId, botB), botB, reply);
  assert.deepEqual([done.state, typeof done.replyId], ['completed', 'string']);
  assert.equal((await lines(trip)).at(-1), `${botB}: ${reply}`);
  assert.equal((await taskData(bTaskId)).handedBack, undefined);
  // The run is told it has not finished; it is offered the Bot it asked even though that Bot no longer takes requests.
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'none' });
  const outcome = { waitId, delegation: { delegationId: id, targetBotId: botB, targetName: 'Beta', outcome: 'detached' } };
  assert.deepEqual((await prepare(botA, taskId)).wait, outcome);
  const continued = await goOn(botA, taskId);
  assert.deepEqual(continued.wait, outcome);
  assert.deepEqual(offered(continued), [botB, botC]);
});

/** `id` is published on another deployment (a new revision of it), as when its maintainer moves it; `run` is what happens while it is there. */
async function movedTo(id, deploymentId, run) {
  const old = bots[id].host;
  bots[id].host = { deploymentId, hostId: `host_${deploymentId}`, accountEpoch: 1 };
  try {
    await publishBot(id, defaults[id]);
    return await run(old);
  } finally { bots[id].host = old; }
}

test('a delegation that waits: the Bot that waits moved to another deployment, so its run is not resumed and the reply is posted in the Chat', options, async () => {
  const { taskId, id, bTaskId } = await waiting(1);
  await movedTo(botA, 'deployment_alpha_2', async old => {
    // A new revision refuses every earlier task of Alpha at its next step, as for every wait: the old deployment is not Alpha's any more, and the new one is not given the run.
    await assert.rejects(execute('detachDelegation', { ...old, taskId, waitId }, owner), { code: 'DEPLOYMENT_REQUIRED' });
    assert.deepEqual((await execute('listHostTasks', { ...bots[botA].host }, owner)).tasks, []);
    // Beta answers: nobody waits for it any more, so the reply is a reply like any other, under its own name.
    const done = await complete(await startTask(bTaskId, botB), botB, reply);
    assert.deepEqual([done.state, typeof done.replyId], ['completed', 'string']);
    assert.equal((await lines(trip)).at(-1), `${botB}: ${reply}`);
    assert.equal((await taskData(bTaskId)).handedBack, undefined);
    assert.deepEqual([(await delegationData(id)).state, (await delegationData(id)).result], ['open', undefined]);
    assert.deepEqual([(await taskData(taskId)).state, (await taskData(taskId)).resume], ['waiting', undefined]);
  });
});

test('a delegation that waits: the other Bot moved to another deployment, so its old deployment\'s result is refused as usual and the run is told it has not finished', options, async () => {
  const { taskId, id, bTaskId } = await waiting(1);
  const started = await startTask(bTaskId, botB), before = await lines(trip);
  await movedTo(botB, 'deployment_beta_2', async old => {
    await assert.rejects(execute('completeTask', resultOf(started, botB, reply, old), member), { code: 'DEPLOYMENT_REQUIRED' });
    assert.deepEqual(await lines(trip), before);
    assert.deepEqual([(await delegationData(id)).state, (await taskData(taskId)).state, (await waitData(waitId)).state], ['open', 'waiting', 'pending']);
    // The run waits as long as it was told, and then goes on without it.
    now += delegationWaitMs; await beat();
    assert.deepEqual(await detach(botA, taskId), { taskId, state: 'queued', outcome: 'detached' });
    assert.deepEqual((await prepare(botA, taskId)).wait, { waitId, delegation: { delegationId: id, targetBotId: botB, targetName: 'Beta', outcome: 'detached' } });
  });
});

test('a delegation the service refuses at the pause queues the run again at once, with the reason, and makes nothing', options, async () => {
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'none' });
  const { taskId, started } = await runA(1), before = await lines(trip);
  const paused = await pauseOn(started, botA, botB);
  assert.deepEqual(paused, { taskId, state: 'queued', waitId, messageId: null, expiresAt: now, holdsUntil: now + day });
  const queued = await taskData(taskId);
  assert.deepEqual([queued.state, queued.resume], ['queued', { waitId, refusal: 'DELEGATION_NOT_ACCEPTED', target: botB }]);
  assert.deepEqual([await count('delegations'), await lines(trip)], [0, before]);
  assert.equal(await count('tasks'), 1);
  const outcome = { waitId, delegation: { outcome: 'refused', code: 'DELEGATION_NOT_ACCEPTED', targetBotId: botB, targetName: 'Beta' } };
  assert.deepEqual((await prepare(botA, taskId)).wait, outcome);
  const continued = await goOn(botA, taskId);
  assert.deepEqual(continued.wait, outcome);
  assert.deepEqual(offered(continued), [botB, botC], 'the replay of the call needs the Bot it named');
});

for (const [who, actor] of [['the person who asked', asker], ['the maintainer of the other Bot', member], ['the maintainer of the Bot that asked', owner]]) {
  test(`cancelTask: ${who} cancels what the other Bot was asked, and the run that waits goes on at once`, options, async () => {
    const { taskId, id, bTaskId } = await waiting(1);
    await assert.rejects(execute('cancelTask', { taskId: bTaskId }, bystander), { code: 'TASK_CANCEL_FORBIDDEN' });
    assert.deepEqual(await execute('cancelTask', { taskId: bTaskId }, actor), { taskId: bTaskId, state: 'cancelled' });
    assert.equal((await taskData(bTaskId)).state, 'cancelled');
    assert.deepEqual([(await delegationData(id)).state, (await waitData(waitId)).delegationOutcome, (await taskData(taskId)).state], ['cancelled', 'cancelled', 'queued']);
    assert.deepEqual((await prepare(botA, taskId)).wait, { waitId, delegation: { delegationId: id, targetBotId: botB, targetName: 'Beta', outcome: 'cancelled' } });
    await assert.rejects(execute('cancelTask', { taskId: bTaskId }, actor), { code: 'TASK_FINISHED' });
  });
}

test('cancelTask: cancelling the first task cancels what was asked under it, and nobody else may cancel a task another Bot asked for', options, async () => {
  const { taskId, started } = await runA(1);
  const toB = await delegate(started, botA, botB), b = await startTask(toB.taskId, botB), toC = await delegate(b, botB, botC);
  await assert.rejects(execute('cancelTask', { taskId: toC.taskId }, bystander), { code: 'TASK_CANCEL_FORBIDDEN' });
  assert.deepEqual(await execute('cancelTask', { taskId }, asker), { taskId, state: 'running' });
  const [a, bTask, cTask] = await Promise.all([taskData(taskId), taskData(toB.taskId), taskData(toC.taskId)]);
  // The tasks that are running are asked to stop, and the one that has not started is ended; a delegation that waits for no reply is left as it was.
  assert.deepEqual([a.cancelRequestedAt, bTask.cancelRequestedAt, cTask.state], [now, now, 'cancelled']);
});

test('cancelTask: cancelling the run that waits cancels what it asked, and nothing is handed back to a run that is gone', options, async () => {
  const { taskId, id, bTaskId } = await waiting(1);
  assert.deepEqual(await execute('cancelTask', { taskId }, asker), { taskId, state: 'cancelled' });
  assert.deepEqual([(await taskData(taskId)).state, (await taskData(bTaskId)).state, (await delegationData(id)).state, (await waitData(waitId)).state],
    ['cancelled', 'cancelled', 'cancelled', 'revoked']);
  // Neither Desktop is offered either task any more.
  await assert.rejects(prepare(botB, bTaskId), { code: 'TASK_ALREADY_STARTED' });
  await assert.rejects(prepare(botA, taskId), { code: 'TASK_ALREADY_STARTED' });
});

test('a delegation message opens no reply round and no auto-reply task, whatever the Chat lets its Bots answer', options, async () => {
  await publishBot(botB, { ...defaults[botB], autoReply: { daily: 50 } });
  await publishBot(botC, { ...defaults[botC], autoReply: { daily: 50 } });
  await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 0, targets: [botB, botC] }, asker);
  const { taskId, started } = await runA(1);
  assert.equal(await count('tasks'), 1, 'asking Alpha by name asks only Alpha');
  const sent = await delegate(started, botA, botB);
  assert.equal(sent.sent, true);
  assert.deepEqual([await count('tasks'), await count('replyRounds')], [2, 0]);
  const task = await taskData(sent.taskId);
  assert.deepEqual([task.optional, task.roundId, task.rootTaskId], [undefined, undefined, taskId]);
});

test('delegateTask: a Bot nobody asked does not ask other Bots', options, async () => {
  await publishBot(botA, { ...defaults[botA], autoReply: { daily: 50 } });
  await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 0, targets: [botA] }, asker);
  await messages.execute(asker, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(trip, 1), changeId: 'chg_trip_0001', operation: 'create', text: asking });
  const taskId = admissionId(messageId(trip, 1), botA);
  await win(taskId);
  const started = await startTask(taskId, botA);
  assert.deepEqual(offered(started), []);
  assert.deepEqual(await delegate(started, botA, botB), { sent: false, code: 'DELEGATION_UNASKED' });
  assert.equal(await count('delegations'), 0);
});

test('held reply: the maintainer of the other Bot approves it, and the waiting run is handed it instead of the Chat', options, async () => {
  const { taskId, id, bTaskId } = await waiting(1), before = await lines(trip);
  const held = await hold(await startTask(bTaskId, botB), botB, reply);
  assert.deepEqual(held, { taskId: bTaskId, state: 'awaiting_approval', replyId: null, artifactId: null });
  assert.deepEqual([(await taskData(taskId)).state, (await delegationData(id)).state], ['waiting', 'open'], 'the run waits while the maintainer reads');
  const approved = await execute('approveReply', { taskId: bTaskId, text: 'Revenue was 12.4 million (checked).' }, member);
  assert.deepEqual([approved.state, approved.replyId], ['completed', null]);
  assert.deepEqual([(await delegationData(id)).state, (await delegationData(id)).result], ['done', { text: 'Revenue was 12.4 million (checked).' }]);
  assert.deepEqual([(await taskData(taskId)).state, await lines(trip)], ['queued', before]);
  assert.equal((await prepare(botA, taskId)).wait.delegation.text, 'Revenue was 12.4 million (checked).');
});

test('held reply: one the maintainer rejects, or lets run out, is not handed to the waiting run, which is told it may not be shared', options, async () => {
  const first = await waiting(1);
  await hold(await startTask(first.bTaskId, botB), botB, reply);
  assert.deepEqual(await execute('rejectReply', { taskId: first.bTaskId }, member), { taskId: first.bTaskId, state: 'declined', declineReason: 'approval_rejected' });
  assert.deepEqual([(await delegationData(first.id)).state, (await delegationData(first.id)).result, (await taskData(first.taskId)).state], ['withheld', undefined, 'queued']);
  assert.deepEqual((await prepare(botA, first.taskId)).wait.delegation, { delegationId: first.id, targetBotId: botB, targetName: 'Beta', outcome: 'withheld' });
  assert.deepEqual(await lines(trip), [`${asker.uid}: ${asking}`, `${botA}: @Beta Find the 2026 figures.`]);
  // The same when nobody decides in time.
  await complete(await goOn(botA, first.taskId), botA, 'Without Beta.');
  const second = await waiting(2);
  await hold(await startTask(second.bTaskId, botB), botB, reply);
  now += approvalMs; await beat();
  assert.deepEqual(await expireHeldReply(db, () => now, { teamId, taskId: second.bTaskId }), { ended: true });
  assert.deepEqual([(await delegationData(second.id)).state, (await taskData(second.taskId)).state], ['withheld', 'queued']);
});

test('the directory says which Bots take requests from other Bots, and whom they ask', options, async () => {
  const cards = async () => Object.fromEntries((await execute('listSharedBots', {}, asker)).bots.map(card => [card.sharedBotId, card]));
  let all = await cards();
  assert.deepEqual(all[botA].capabilities, { durableWait: true, writeOperations: false, delegation: true });
  assert.deepEqual([all[botA].acceptDelegation, all[botA].delegateTo, all[botA].delegation], ['team_bots', [botB, botC], { daily: 10 }]);
  assert.deepEqual([all[botD].acceptDelegation, all[botD].delegateTo, all[botD].delegation], ['team_bots', undefined, undefined]);
  await publishBot(botB, { ...defaults[botB], acceptDelegation: 'listed', acceptFrom: [botA] });
  await publishBot(botC, {}, { durableWait: true, writeOperations: false, delegation: false });
  all = await cards();
  assert.deepEqual([all[botB].acceptDelegation, all[botB].acceptFrom], ['listed', [botA]]);
  assert.deepEqual([all[botC].acceptDelegation, all[botC].delegateTo, all[botC].capabilities.delegation], [undefined, undefined, false]);
});
