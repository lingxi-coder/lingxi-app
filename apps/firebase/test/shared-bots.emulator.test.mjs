import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { createHash } from 'node:crypto';
import { getFirestore, FieldValue } from 'firebase-admin/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { onTeamMessageCreated } from '../src/team-message-push.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-execution', teamId = 'team_execution', conversationId = 'chat_execution';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
let app, db, teams, store, now, serial = 0;
const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = async (action, payload, actor = owner, commandId) => {
  if (action === 'startTask' && !Object.hasOwn(payload, 'expectedLastChangeSeq')) {
    payload = { ...payload, expectedLastChangeSeq: (await chatRef().get()).data().lastChangeSeq };
  }
  return store.execute(actor, command(action, payload, commandId));
};
const input = (kind, resourceId, sourceAudience = { mode: 'team' }) => ({ kind, resourceId, revision: 1, digest: 'a'.repeat(64), sourceAudience });
const publish = (override = {}) => ({ sharedBotId: 'sharedbot_01', expectedRevision: 0, name: 'Travel assistant', description: 'Shared travel notes',
  usePolicy: { mode: 'all' }, deployment: { ...host, sharedBotId: 'sharedbot_01', revision: 1, owner: { uid: owner.uid, generation: 1 },
    instanceId: 'instance_01', definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: false, writeOperations: false, delegation: false }, enabled: true },
  inputs: [input('persona', 'persona_01'), input('file', 'travel_file')], quotas: { team: 100, member: 20, bot: 100 }, ...override });
const submit = (id = ++serial) => ({ conversationId, messageId: `message_${id}`, changeId: `change_${id}`, text: 'What should we pack?', targets: ['sharedbot_01'] });
const chatRef = () => db.doc(`teams/${teamId}/conversations/${conversationId}`);
async function start(payload = submit()) {
  const receipt = await execute('submit', payload, member);
  return execute('startTask', { ...host, taskId: receipt.tasks[0].taskId });
}
function result(started, override = {}) {
  return { ...host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: 'Pack your passports.', finishedAt: now,
    inputGrantIds: started.inputs.map(g => g.grantId), outputAudience: { mode: 'published_inputs' },
    document: { title: 'Packing list', body: 'Pack your passports.' }, usage: { status: 'missing' }, ...override };
}
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'shared-bot-execution-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  await db.recursiveDelete(db.collection('teams')); await db.recursiveDelete(db.collection('users'));
  now = Date.UTC(2026, 10, 5); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Execution test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  await execute('publishSharedBot', publish());
  await execute('heartbeatDeployment', { ...host, ready: true });
  await execute('createConversation', { conversationId, kind: 'bot_dm', title: 'Travel',
    participantUids: [member.uid], sharedBotIds: ['sharedbot_01'] }, member);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('execution 2: simultaneous duplicate submissions create one message, admission and quota charge', options, async () => {
  const payload = submit(), values = await Promise.all(Array.from({ length: 8 }, () => execute('submit', payload, member)));
  assert.ok(values.every(value => value.tasks[0].taskId === values[0].tasks[0].taskId));
  assert.equal((await chatRef().collection('messages').get()).size, 1);
  assert.equal((await db.collection(`teams/${teamId}/tasks`).get()).size, 1);
  assert.ok((await db.collection(`teams/${teamId}/requestQuotas`).get()).docs.every(doc => doc.data().used === 1));
  await assert.rejects(execute('submit', { ...payload, text: 'Changed' }, member), { code: 'MESSAGE_ID_CONFLICT' });
});
test('execution 2: concurrent start returns exactly one capability and never replays it', options, async () => {
  const receipt = await execute('submit', submit(), member), p = { ...host, taskId: receipt.tasks[0].taskId, expectedLastChangeSeq: receipt.changeSeq };
  const commands = Array.from({ length: 12 }, () => command('startTask', p));
  const values = await Promise.all(commands.map(value => store.execute(owner, value)));
  assert.equal(values.filter(value => value.execute).length, 1);
  const index = values.findIndex(value => value.execute);
  assert.deepEqual(await store.execute(owner, commands[index]), { execute: false, taskId: p.taskId });
  assert.equal((await chatRef().collection('triggers').get()).size, 0);
});
test('execution 2: command identity rejects changed payload without mutation', options, async () => {
  const value = command('submit', submit()); await store.execute(member, value);
  await assert.rejects(store.execute(member, { ...value, payload: { ...value.payload, text: 'changed' } }), { code: 'COMMAND_ID_CONFLICT' });
  assert.equal((await chatRef().get()).data().lastSeq, 1);
});
test('execution 2: twenty requests competing for ten slots admit at most ten', options, async () => {
  await db.doc(`teams/${teamId}/sharedBots/sharedbot_01`).update({ quotas: { team: 10, member: 10, bot: 10 } });
  const results = await Promise.allSettled(Array.from({ length: 20 }, (_, index) => execute('submit', submit(`quota_${index}`), member)));
  assert.equal(results.filter(value => value.status === 'fulfilled').length, 10);
  assert.ok(results.filter(value => value.status === 'rejected').every(value => value.reason.code === 'REQUEST_QUOTA_EXCEEDED'));
  assert.equal((await chatRef().get()).data().lastSeq, 10);
});
test('execution 2: member fourth request is denied before a task or model permit exists', options, async () => {
  await db.doc(`teams/${teamId}/sharedBots/sharedbot_01`).update({ quotas: { team: 100, member: 3, bot: 100 } });
  for (let i = 0; i < 3; i++) await execute('submit', submit(), member);
  await assert.rejects(execute('submit', submit(), member), { code: 'REQUEST_QUOTA_EXCEEDED' });
  assert.equal((await db.collection(`teams/${teamId}/tasks`).get()).size, 3);
  assert.ok((await db.collection(`teams/${teamId}/tasks`).get()).docs.every(doc => doc.data().state === 'queued'));
});
test('execution 2: published persona and shared file produce one visible reply and reopenable artifact', options, async () => {
  const started = await start(), payload = result(started);
  const values = await Promise.all(Array.from({ length: 6 }, () => execute('completeTask', payload)));
  assert.ok(values.every(value => value.replyId === values[0].replyId));
  assert.equal((await chatRef().collection('messages').get()).size, 2);
  const reply = (await chatRef().collection('messages').doc(values[0].replyId).get()).data();
  assert.equal(reply.text, payload.text); assert.equal(reply.author.kind, 'shared_bot');
  const artifact = await execute('readArtifact', { artifactId: values[0].artifactId }, member);
  assert.deepEqual(artifact.document, payload.document);
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 1);
  await assert.rejects(execute('readArtifact', { artifactId: values[0].artifactId }, other), { code: 'ARTIFACT_READ_REQUIRED' });
  await assert.rejects(execute('completeTask', { ...payload, text: 'different result' }), { code: 'TASK_RESULT_CONFLICT' });
});
test('execution 2: owner-only tool result cannot enter message, preview, artifact or shared receipt', options, async () => {
  const started = await start(), secret = 'PRIVATE-TOOL-RESULT-DO-NOT-SHARE';
  const value = await execute('completeTask', result(started, { text: secret, outputAudience: { mode: 'owner' }, document: { title: 'Private', body: secret } }));
  assert.equal(value.state, 'authorization_required'); assert.equal(value.artifactId, null);
  const docs = await chatRef().collection('messages').get();
  assert.ok(!JSON.stringify(docs.docs.map(doc => doc.data())).includes(secret));
  assert.ok(!JSON.stringify((await chatRef().get()).data()).includes(secret));
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 0);
  assert.ok(!JSON.stringify((await db.collection(`teams/${teamId}/executionReceipts`).get()).docs.map(doc => doc.data())).includes(secret));
});
test('execution 2: an extra Chat reader outside the input audience blocks shared output', options, async () => {
  const started = await start();
  const ref = db.doc(`teams/${teamId}/sharedBots/sharedbot_01`), bot = (await ref.get()).data();
  const grants = bot.grants.map(g => ({ ...g, usePolicy: { mode: 'listed', members: [{ uid: member.uid, generation: 1 }] } }));
  await ref.update({ grants });
  await chatRef().update({ memberUids: [member.uid, other.uid] });
  const completed = await execute('completeTask', result(started));
  assert.equal(completed.state, 'authorization_required');
  assert.ok(!(await chatRef().get()).data().lastMessage.preview.includes('passports'));
});
test('execution 2: private named input is never broadened by publishing', options, async () => {
  const ref = db.doc(`teams/${teamId}/sharedBots/sharedbot_01`), value = (await ref.get()).data();
  await ref.update({ grants: value.grants.map(g => g.kind === 'file' ? { ...g, sourceAudience: { mode: 'owner' } } : g) });
  await assert.rejects(execute('submit', submit(), member), { code: 'INPUT_GRANT_REQUIRED' });
  assert.equal((await chatRef().collection('messages').get()).size, 0);
});
test('execution 2: revocation and membership generations invalidate permit and artifact access', options, async () => {
  const started = await start();
  await teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 });
  await assert.rejects(execute('completeTask', result(started)), { code: 'TASK_REVOKED' });
  await assert.rejects(execute('submit', submit(), member), { code: 'MEMBERSHIP_REQUIRED' });
});
test('execution 2: five-minute deadline cannot be reset by reusing or replacing a command ID', options, async () => {
  const started = await start(); now += 300001;
  await assert.rejects(execute('completeTask', result(started)), { code: 'TASK_EXPIRED' });
  await execute('heartbeatDeployment', { ...host, ready: true });
  assert.equal((await execute('startTask', { ...host, taskId: started.task.taskId })).execute, false);
});
test('execution 2: mismatched host, account epoch, old actions and compatibility fields are rejected', options, async () => {
  const receipt = await execute('submit', submit(), member);
  await assert.rejects(execute('startTask', { ...host, hostId: 'wrong_host', taskId: receipt.tasks[0].taskId }), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(execute('startTask', { ...host, accountEpoch: 2, taskId: receipt.tasks[0].taskId }), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(execute('claimBot', {}));
  await assert.rejects(store.execute(owner, { ...command('startTask', { ...host, taskId: receipt.tasks[0].taskId }), executionProtocol: 'v1' }));
});
test('execution 2: listed generation, owner reading and explicit source re-sharing remain distinct', options, async () => {
  const original = publish(), next = { ...original, expectedRevision: 1, usePolicy: { mode: 'listed', members: [{ uid: member.uid, generation: 1 }] },
    deployment: { ...original.deployment, revision: 2 } };
  await execute('publishSharedBot', next); await execute('heartbeatDeployment', { ...host, ready: true });
  assert.deepEqual((await chatRef().get()).data().memberUids, ['member', 'owner']);
  const started = await start(), completed = await execute('completeTask', result(started));
  assert.equal(completed.state, 'completed');
  assert.equal((await execute('readArtifact', { artifactId: completed.artifactId }, member)).document.body, 'Pack your passports.');
  await assert.rejects(execute('submit', submit()), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute('publishSharedBot', { ...next, expectedRevision: 2, deployment: { ...next.deployment, revision: 3 },
    inputs: [input('persona', 'persona_01'), input('file', 'private_file', { mode: 'owner' })] }), { code: 'INPUT_AUDIENCE_NARROWER' });
});
test('execution 2: tasks in one Bot Chat start in message order and cannot overlap', options, async () => {
  const first = await execute('submit', submit(), member), second = await execute('submit', submit(), member);
  const retry = command('startTask', { ...host, taskId: second.tasks[0].taskId, expectedLastChangeSeq: second.changeSeq });
  assert.equal((await store.execute(owner, retry)).execute, false);
  await assert.rejects(store.execute(owner, { ...retry, payload: { ...retry.payload, expectedLastChangeSeq: 0 } }), { code: 'COMMAND_ID_CONFLICT' });
  const started = await execute('startTask', { ...host, taskId: first.tasks[0].taskId }); assert.equal(started.execute, true);
  assert.equal((await store.execute(owner, retry)).execute, false);
  await execute('completeTask', result(started));
  const contenders = await Promise.all(Array.from({ length: 6 }, () => store.execute(owner, retry)));
  assert.equal(contenders.filter(value => value.execute).length, 1);
  assert.equal((await store.execute(owner, retry)).execute, false);
});
test('execution 2: conversation policy revocation after admission blocks model start', options, async () => {
  const receipt = await execute('submit', submit(), member);
  // The maintainer has read access but is not a human Chat participant. A policy
  // manager acts only in a Chat they participate in, so seed the authoritative change.
  await chatRef().update({ executionPolicy: { revision: 1, targets: [] } });
  await assert.rejects(execute('startTask', { ...host, taskId: receipt.tasks[0].taskId }), { code: 'TASK_REVOKED' });
});
test('execution 2: host discovery pages past more than one hundred expired tasks', options, async () => {
  const submitted = await execute('submit', submit(), member);
  const active = (await db.doc(`teams/${teamId}/tasks/${submitted.tasks[0].taskId}`).get()).data();
  const batch = db.batch();
  batch.delete(db.doc(`teams/${teamId}/tasks/${active.taskId}`));
  active.taskId = 'zz_active_01';
  batch.set(db.doc(`teams/${teamId}/tasks/${active.taskId}`), active);
  for (let index = 0; index < 105; index++) {
    const taskId = `000_stale_${String(index).padStart(3, '0')}`;
    batch.set(db.doc(`teams/${teamId}/tasks/${taskId}`), { ...active, taskId, deadline: now - 1 });
  }
  await batch.commit();
  const first = await execute('listHostTasks', host);
  assert.equal(first.tasks.length, 0); assert.ok(first.nextCursor);
  const second = await execute('listHostTasks', { ...host, after: first.nextCursor });
  assert.deepEqual(second.tasks.map(task => task.taskId), [active.taskId]);
  assert.equal(second.nextCursor, null);
});
test('execution 2: removed claim/start HTTP actions racing the new start produce one permit total', options, async () => {
  const submitted = await execute('submit', submit(), member), taskId = submitted.tasks[0].taskId;
  const handler = createTeamsHandler({ store: teams, verifyToken: async () => owner, createCustomToken: async () => '' });
  const post = body => handler(new Request('https://fixture.local/v1/teams/messages', { method: 'POST', headers: { Authorization: 'Bearer fixture' }, body: JSON.stringify(body) }));
  const results = await Promise.all([
    post({ action: 'claimBot', teamId, generation: 1 }), post({ action: 'startBot', teamId, generation: 1 }),
    post(command('startTask', { ...host, taskId, expectedLastChangeSeq: submitted.changeSeq })), post(command('startTask', { ...host, taskId, expectedLastChangeSeq: submitted.changeSeq })),
  ]);
  assert.equal(results[0].status, 400); assert.equal(results[1].status, 400);
  const bodies = await Promise.all(results.map(response => response.json()));
  assert.equal(bodies.filter(body => body.execute === true).length, 1);
  assert.equal((await chatRef().collection('triggers').get()).size, 0);
});
test('execution 2: shared Bot reply sends one generic push without result body or preview', options, async () => {
  const started = await start(), completed = await execute('completeTask', result(started));
  const original = (await chatRef().collection('messages').doc(completed.replyId).get()).data();
  const sent = [], device = { id: 'fixture-device', uid: member.uid, revision: 1, fcmInstallationId: 'fixture-fid',
    expiresAt: { toMillis: () => now + 3600000 }, locale: 'zh-CN', appId: 'com.lingxi.code', platform: 'android' };
  const pushStore = { async active(uid) { return uid === member.uid ? [device] : []; }, device() { return { async get() { return { data: () => device }; } }; } };
  const args = { db, pushStore, messaging: { async send(value) { sent.push(value); } }, teamId, conversationId, messageId: completed.replyId, original, clock: () => now };
  await Promise.all([onTeamMessageCreated(args), onTeamMessageCreated(args)]);
  assert.equal(sent.length, 1); assert.equal(sent[0].data.body, '团队有新消息');
  assert.ok(!JSON.stringify(sent).includes('passports'));
});
test('execution 2: host restart rotates a durable account fence without minting another execution permit', options, async () => {
  const started = await start(), saved = result(started);
  await execute('activateDeployment', { deploymentId: host.deploymentId, hostId: host.hostId, expectedAccountEpoch: 1, newAccountEpoch: 2 });
  await assert.rejects(execute('heartbeatDeployment', { ...host, ready: true }), { code: 'DEPLOYMENT_REQUIRED' });
  await execute('heartbeatDeployment', { ...host, accountEpoch: 2, ready: true });
  assert.equal((await execute('startTask', { ...host, accountEpoch: 2, taskId: started.task.taskId })).execute, false);
  const delivered = await execute('completeTask', { ...saved, accountEpoch: 2 }); assert.equal(delivered.state, 'completed');
});
test('execution 2: a completed durable result can arrive late but executing after the deadline is rejected', options, async () => {
  const started = await start(), saved = result(started); now += 300001;
  const response = await execute('completeTask', saved); assert.equal(response.state, 'completed');
  assert.equal((await db.doc(`teams/${teamId}/tasks/${started.task.taskId}`).get()).data().finishedAt, saved.finishedAt);
  assert.equal((await db.doc(`teams/${teamId}/tasks/${started.task.taskId}`).get()).data().deliveredAt, now);
  assert.equal((await execute('startTask', { ...host, taskId: started.task.taskId })).execute, false);
});
test('execution 2: prior-source edit after preparation refuses start and edit during a model request refuses final plaintext', options, async () => {
  const { TeamMessagesStore } = await import('../src/team-messages.mjs');
  const messages = new TeamMessagesStore(db, () => now);
  await chatRef().update({ kind: 'group' });
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId, messageId: 'source_msg01', changeId: 'source_ch01', operation: 'create', text: 'Old detail' });
  const receipt = await execute('submit', submit(), member), taskId = receipt.tasks[0].taskId;
  const prepared = await execute('prepareTask', { ...host, taskId }); assert.equal(prepared.sourceLastChangeSeq, 2);
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId, messageId: 'source_msg01', changeId: 'source_ch02', operation: 'edit', text: 'Changed detail' });
  await assert.rejects(execute('startTask', { ...host, taskId, expectedLastChangeSeq: 2 }), { code: 'SOURCE_CHANGED' });
  const started = await execute('startTask', { ...host, taskId, expectedLastChangeSeq: 3 });
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId, messageId: 'source_msg01', changeId: 'source_ch03', operation: 'recall', text: '' });
  await assert.rejects(execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq: 3 }), { code: 'SOURCE_CHANGED' });
  const response = await execute('completeTask', result(started, { text: 'PRIVATE-DERIVED-OLD-DETAIL' })); assert.equal(response.state, 'failed');
  assert.ok(!(await chatRef().get()).data().lastMessage.preview.includes('PRIVATE-DERIVED'));
  assert.equal(response.artifactId, null);
});

// ---- M2 / Q02: durable human waits, continue permits and cancellation ----
const checkpoint = 'd'.repeat(64);
const taskRef = id => db.doc(`teams/${teamId}/tasks/${id}`);
const taskData = async id => (await taskRef(id).get()).data();
const waitData = async id => (await db.doc(`teams/${teamId}/waits/${id}`).get()).data();
const lockRefFor = (chat = conversationId) => db.collection(`teams/${teamId}/chatExecutionLocks`).doc(createHash('sha256')
  .update(JSON.stringify(['sharedbot_01', chat])).digest('hex'));
const messageCount = async () => (await chatRef().collection('messages').get()).size;
const enableWait = () => db.doc(`teams/${teamId}/deployments/${host.deploymentId}`).update({ 'capabilities.durableWait': true });
const heartbeat = () => execute('heartbeatDeployment', { ...host, ready: true });
const pausePayload = (started, override = {}) => ({ ...host, taskId: started.task.taskId, permitId: started.permitId, waitId: 'wait_0001',
  kind: 'human_input', question: 'Which city are you travelling to?', requestDigest: 'c'.repeat(64), checkpointDigest: checkpoint,
  pausedAt: now, expiresInMs: 3600000, dispatchedRequests: 2, inputGrantIds: started.inputs.map(g => g.grantId),
  outputAudience: { mode: 'published_inputs' }, ...override });
const answerPayload = (n = ++serial, override = {}) => ({ conversationId, waitId: 'wait_0001', messageId: `answer_${n}`, changeId: `answerchg_${n}`, text: 'Lisbon', ...override });
const continuePayload = (taskId, override = {}) => ({ ...host, taskId, expectedEpoch: 1, checkpointDigest: checkpoint, reason: 'answer', dispatchedRequests: 3, ...override });
// Running task, paused after `activeMs` of work.
async function paused({ activeMs = 60000, wait = {} } = {}) {
  await enableWait();
  const started = await start(); now += activeMs;
  const response = await execute('pauseTask', pausePayload(started, wait));
  return { started, response, taskId: started.task.taskId };
}
const asContinued = (started, granted) => ({ task: started.task, permitId: granted.permitId, inputs: granted.inputs });

test('wait: pause invalidates the permit and publishes one question with a durable wait record', options, async () => {
  const { started, response, taskId } = await paused({ activeMs: 60000 });
  assert.equal(response.state, 'waiting'); assert.equal(response.waitId, 'wait_0001'); assert.equal(response.expiresAt, now + 3600000);
  await assert.rejects(execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq: 0 }), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(execute('completeTask', result(started)), { code: 'EXECUTION_PERMIT_REQUIRED' });
  const question = (await chatRef().collection('messages').doc(response.messageId).get()).data(), submitted = await taskData(taskId);
  assert.equal(question.text, 'Which city are you travelling to?'); assert.equal(question.waitId, 'wait_0001'); assert.equal(question.taskId, taskId);
  assert.equal(question.replyTo, submitted.messageId); assert.equal(question.author.kind, 'shared_bot');
  assert.equal(question.seq, 2); assert.equal((await chatRef().get()).data().lastMessage.id, response.messageId);
  const { pauseDigest, ...wait } = await waitData('wait_0001');
  assert.match(pauseDigest, /^[a-f0-9]{64}$/u);
  assert.deepEqual(wait, { schemaVersion: 1, waitId: 'wait_0001', taskId, conversationId, sharedBotId: 'sharedbot_01', kind: 'human_input',
    recipient: { uid: member.uid, generation: 1 }, requestDigest: 'c'.repeat(64), checkpointDigest: checkpoint, messageId: response.messageId,
    createdAt: now, expiresAt: now + 3600000, state: 'pending' });
  assert.equal(submitted.state, 'waiting'); assert.equal(submitted.deadline, now + 3600000); assert.equal(submitted.activeSpentMs, 60000);
  assert.equal(submitted.physicalDispatchesSpent, 2); assert.equal(submitted.currentWaitId, 'wait_0001'); assert.equal(submitted.checkpointDigest, checkpoint);
  assert.deepEqual((await lockRefFor().get()).data(), { taskId, until: now + 3600000 });
  const view = await execute('getTask', { taskId }, member);
  assert.deepEqual({ state: view.state, epoch: view.epoch, activeSpentMs: view.activeSpentMs, wait: view.wait },
    { state: 'waiting', epoch: 1, activeSpentMs: 60000, wait: { waitId: 'wait_0001', kind: 'human_input', recipient: { uid: member.uid, generation: 1 }, expiresAt: now + 3600000, messageId: response.messageId } });
  assert.equal((await execute('listHostTasks', host)).tasks.length, 0);
  now += 3600000; assert.equal((await execute('getTask', { taskId }, member)).state, 'expired');
  assert.equal((await execute('getTask', { taskId }, member)).wait, undefined);
});
test('wait: a 30 minute wait is not charged; the answer grants epoch 2 and completion succeeds', options, async () => {
  const { started, taskId } = await paused({ activeMs: 60000 });
  now += 30 * 60000;
  const answered = await execute('answerWait', answerPayload(), member);
  assert.equal(answered.taskId, taskId);
  const queued = await taskData(taskId);
  assert.equal(queued.state, 'queued'); assert.equal(queued.deadline, now + 300000);
  assert.deepEqual(queued.resume, { waitId: 'wait_0001', answerMessageId: answered.messageId, answerSeq: 3 });
  assert.deepEqual((await lockRefFor().get()).data(), { taskId, until: now + 300000 });
  const resolved = await waitData('wait_0001');
  assert.deepEqual([resolved.state, resolved.responseMessageId, resolved.responseSeq, resolved.resolvedAt], ['resolved', answered.messageId, 3, now]);
  const listed = (await execute('listHostTasks', host)).tasks.find(task => task.taskId === taskId);
  assert.equal(listed.epoch, 1); assert.deepEqual(listed.resume, queued.resume);
  await heartbeat();
  assert.equal((await execute('startTask', { ...host, taskId })).execute, false);
  const granted = await execute('continueTask', continuePayload(taskId, { dispatchedRequests: 3 }));
  assert.equal(granted.execute, true); assert.equal(granted.epoch, 2); assert.equal(granted.remainingActiveMs, 240000);
  assert.equal(granted.physicalDispatchesSpent, 3); assert.deepEqual(granted.wait, { waitId: 'wait_0001', answerMessageId: answered.messageId, answerSeq: 3 });
  assert.equal(granted.task.state, 'running'); assert.equal(granted.task.epoch, 2); assert.equal(granted.messageId, queued.messageId);
  assert.equal(granted.messageSeq, 1); assert.equal(granted.deployment.deploymentId, host.deploymentId); assert.equal(granted.inputs.length, 2);
  const running = await taskData(taskId);
  assert.equal(running.activeSpentMs, 60000); assert.equal(running.deadline, now + 240000); assert.equal(running.resume, undefined);
  assert.equal(running.attemptStartedAt, now); assert.deepEqual((await lockRefFor().get()).data(), { taskId, until: now + 240000 });
  await assert.rejects(execute('completeTask', result(started)), { code: 'EXECUTION_PERMIT_REQUIRED' });
  const done = await execute('completeTask', result(asContinued(started, granted)));
  assert.equal(done.state, 'completed'); assert.equal((await lockRefFor().get()).exists, false);
});
test('wait: concurrent continue yields exactly one grant and the granted call never replays', options, async () => {
  const { taskId } = await paused(); await execute('answerWait', answerPayload(), member); await heartbeat();
  const commands = Array.from({ length: 8 }, () => command('continueTask', continuePayload(taskId)));
  const values = await Promise.all(commands.map(value => store.execute(owner, value)));
  assert.equal(values.filter(value => value.execute).length, 1);
  const index = values.findIndex(value => value.execute);
  assert.deepEqual(await store.execute(owner, commands[index]), { execute: false, taskId });
  assert.equal((await taskData(taskId)).epoch, 2);
});
test('wait: recovery continue replaces the permit without moving the absolute deadline', options, async () => {
  const started = await start(), taskId = started.task.taskId, deadline = started.task.deadline;
  assert.equal(deadline, now + 300000); assert.equal(started.task.epoch, 1);
  now += 10000; await heartbeat();
  const granted = await execute('continueTask', continuePayload(taskId, { reason: 'recovery', checkpointDigest: 'e'.repeat(64), dispatchedRequests: 1 }));
  assert.equal(granted.execute, true); assert.equal(granted.epoch, 2); assert.equal(granted.wait, undefined);
  assert.equal(granted.remainingActiveMs, deadline - now);
  const recovered = await taskData(taskId);
  assert.equal(recovered.deadline, deadline); assert.equal(recovered.activeSpentMs, 10000); assert.equal(recovered.checkpointDigest, 'e'.repeat(64));
  assert.deepEqual((await lockRefFor().get()).data(), { taskId, until: deadline });
  await assert.rejects(execute('completeTask', result(started)), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq: 0 }), { code: 'EXECUTION_PERMIT_REQUIRED' });
  // A stale epoch is refused; past the deadline nothing is granted.
  assert.equal((await execute('continueTask', continuePayload(taskId, { reason: 'recovery' }))).execute, false);
  now = deadline; await heartbeat();
  assert.deepEqual(await execute('continueTask', continuePayload(taskId, { reason: 'recovery', expectedEpoch: 2 })), { execute: false, taskId });
  assert.equal((await execute('getTask', { taskId }, member)).state, 'interrupted');
  assert.equal((await taskData(taskId)).state, 'running');
});
test('wait: recovery after the recorded host changed is refused', options, async () => {
  const started = await start(), taskId = started.task.taskId;
  await taskRef(taskId).update({ hostId: 'host_other' }); await heartbeat();
  assert.equal((await execute('continueTask', continuePayload(taskId, { reason: 'recovery' }))).execute, false);
  assert.equal((await taskData(taskId)).epoch, 1);
});
test('wait: exhausted active budget fails the task and releases the lock', options, async () => {
  const { taskId } = await paused({ activeMs: 299500 });
  assert.equal((await taskData(taskId)).activeSpentMs, 299500);
  await execute('answerWait', answerPayload(), member); await heartbeat();
  assert.deepEqual(await execute('continueTask', continuePayload(taskId)), { execute: false, taskId, state: 'failed' });
  assert.equal((await taskData(taskId)).state, 'failed'); assert.equal((await lockRefFor().get()).exists, false);
  assert.equal((await execute('getTask', { taskId }, member)).state, 'failed');
});
test('wait: continue checks checkpoint, epoch, cancellation, revocation and ordering', options, async () => {
  const { taskId } = await paused(); await execute('answerWait', answerPayload(), member); await heartbeat();
  await assert.rejects(execute('continueTask', continuePayload(taskId, { checkpointDigest: 'f'.repeat(64) })), { code: 'CHECKPOINT_MISMATCH' });
  assert.equal((await execute('continueTask', continuePayload(taskId, { expectedEpoch: 2 }))).execute, false);
  // The recovery reason applies only to a running attempt.
  assert.equal((await execute('continueTask', continuePayload(taskId, { reason: 'recovery' }))).execute, false);
  await chatRef().update({ executionPolicy: { revision: 1, targets: [] } });
  await assert.rejects(execute('continueTask', continuePayload(taskId)), { code: 'TASK_REVOKED' });
  await chatRef().update({ executionPolicy: FieldValue.delete() });
  await lockRefFor().set({ taskId: 'someone_else', until: now + 60000 });
  assert.equal((await execute('continueTask', continuePayload(taskId))).execute, false);
  await lockRefFor().set({ taskId, until: now + 60000 });
  assert.equal((await execute('continueTask', continuePayload(taskId))).execute, true);
});
test('wait: an answer window that lapsed expires the queued task instead of granting', options, async () => {
  const { taskId } = await paused(); await execute('answerWait', answerPayload(), member);
  now += 300000; await heartbeat();
  assert.equal((await execute('continueTask', continuePayload(taskId))).execute, false);
  assert.equal((await taskData(taskId)).state, 'expired');
});
for (const operation of ['edit', 'recall']) {
  test(`wait: an answer ${operation === 'edit' ? 'edited' : 'recalled'} before continue is SOURCE_CHANGED`, options, async () => {
    const { TeamMessagesStore } = await import('../src/team-messages.mjs');
    const { taskId } = await paused(), answered = await execute('answerWait', answerPayload(), member); await heartbeat();
    await new TeamMessagesStore(db, () => now).execute(member, { action: 'mutate', teamId, generation: 1, conversationId,
      messageId: answered.messageId, changeId: `mutate_${operation}`, operation, text: operation === 'edit' ? 'Porto' : '' });
    await assert.rejects(execute('continueTask', continuePayload(taskId)), { code: 'SOURCE_CHANGED' });
    assert.equal((await taskData(taskId)).state, 'queued'); assert.equal((await taskData(taskId)).epoch, 1);
  });
}
test('wait: answerWait authorization and closure refusals', options, async () => {
  const { taskId } = await paused();
  const addOther = () => chatRef().update({ memberUids: ['member', 'other', 'owner'], participants: ['user:member', 'user:other', 'shared_bot:sharedbot_01'] });
  const restore = async () => (await chatRef().get()).data();
  const before = await restore();
  // A Team member outside the Chat, and a Chat participant who is not the recipient.
  await assert.rejects(execute('answerWait', answerPayload(), other), { code: 'CONVERSATION_REQUIRED' });
  await addOther();
  await assert.rejects(execute('answerWait', answerPayload(), other), { code: 'WAIT_RECIPIENT_REQUIRED' });
  await chatRef().update({ memberUids: before.memberUids, participants: before.participants });
  // Same UID with a new membership generation is not the recorded recipient.
  await db.doc(`teams/${teamId}/members/${member.uid}`).update({ generation: 2 });
  await assert.rejects(store.execute(member, { ...command('answerWait', answerPayload()), generation: 2 }), { code: 'WAIT_RECIPIENT_REQUIRED' });
  await db.doc(`teams/${teamId}/members/${member.uid}`).update({ generation: 1 });
  // The recipient who left the Chat cannot answer.
  await chatRef().update({ memberUids: ['owner'], participants: ['shared_bot:sharedbot_01'] });
  await assert.rejects(execute('answerWait', answerPayload(), member), { code: 'CONVERSATION_REQUIRED' });
  await chatRef().update({ memberUids: before.memberUids, participants: before.participants });
  await assert.rejects(execute('answerWait', answerPayload(undefined, { waitId: 'wait_unknown' }), member), { code: 'WAIT_REQUIRED' });
  await assert.rejects(execute('answerWait', answerPayload(undefined, { messageId: 'bot_answer_1' }), member), { code: 'INVALID_INPUT' });
  assert.equal(await messageCount(), 2); assert.equal((await taskData(taskId)).state, 'waiting');
  // Closed waits: expired, answered once, cancelled.
  now += 3600000;
  await assert.rejects(execute('answerWait', answerPayload(), member), { code: 'WAIT_CLOSED' });
});
test('wait: second answer, replay and message-id reuse', options, async () => {
  await paused();
  const payload = answerPayload(), first = await execute('answerWait', payload, member);
  assert.deepEqual(await execute('answerWait', payload, member, 'replay_command_01'), first);
  assert.deepEqual(await execute('answerWait', payload, member), first);
  assert.equal(await messageCount(), 3);
  await assert.rejects(execute('answerWait', { ...payload, text: 'Porto' }, member), { code: 'MESSAGE_ID_CONFLICT' });
  await assert.rejects(execute('answerWait', answerPayload(), member), { code: 'WAIT_CLOSED' });
  assert.equal(await messageCount(), 3);
  const answer = (await chatRef().collection('messages').doc(first.messageId).get()).data();
  assert.equal(answer.replyTo.startsWith('bot_'), true); assert.equal(answer.waitId, 'wait_0001'); assert.equal(answer.sourceTaskId, first.taskId);
  assert.equal(answer.author.uid, member.uid); assert.equal(answer.taskIds, undefined); assert.equal(answer.mentions, undefined);
});
test('wait: answerWait after cancel is closed', options, async () => {
  const { taskId } = await paused();
  assert.deepEqual(await execute('cancelTask', { taskId }, member), { taskId, state: 'cancelled' });
  await assert.rejects(execute('answerWait', answerPayload(), member), { code: 'WAIT_CLOSED' });
  assert.equal((await waitData('wait_0001')).state, 'revoked');
});
test('wait: answerWait consumes no quota, creates no task and never admits a mentioning answer', options, async () => {
  await paused();
  const quotas = async () => JSON.stringify((await db.collection(`teams/${teamId}/requestQuotas`).get()).docs.map(doc => [doc.id, doc.data()]).sort());
  const tasksBefore = (await db.collection(`teams/${teamId}/tasks`).get()).size, usedBefore = await quotas();
  await db.doc(`teams/${teamId}/sharedBots/sharedbot_01`).update({ quotas: { team: 1, member: 1, bot: 1 } });
  await execute('answerWait', answerPayload(undefined, { text: '@Travel assistant Lisbon please' }), member);
  assert.equal((await db.collection(`teams/${teamId}/tasks`).get()).size, tasksBefore);
  assert.equal(await quotas(), usedBefore);
});
test('wait: pause requires the durable-wait capability and a matching live permit', options, async () => {
  const started = await start(), taskId = started.task.taskId;
  await assert.rejects(execute('pauseTask', pausePayload(started)), { code: 'DURABLE_WAIT_UNSUPPORTED' });
  await enableWait();
  await assert.rejects(execute('pauseTask', pausePayload(started, { permitId: 'wrong_permit_01' })), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(execute('pauseTask', pausePayload(started, { pausedAt: now + 1 })), { code: 'TASK_EXPIRED' });
  await assert.rejects(execute('pauseTask', pausePayload(started, { pausedAt: started.task.deadline + 1 })), { code: 'TASK_EXPIRED' });
  await assert.rejects(execute('pauseTask', pausePayload(started, { expiresInMs: 59999 })));
  await assert.rejects(execute('pauseTask', pausePayload(started, { question: 'x'.repeat(2001) })));
  assert.equal((await taskData(taskId)).state, 'running'); assert.equal(await messageCount(), 1);
  assert.equal((await db.collection(`teams/${teamId}/waits`).get()).size, 0);
});
test('wait: pause behind a failing audience gate behaves like a refused result and publishes no question', options, async () => {
  await enableWait();
  const started = await start(), taskId = started.task.taskId;
  const refused = await execute('pauseTask', pausePayload(started, { outputAudience: { mode: 'owner' }, question: 'PRIVATE-QUESTION-DO-NOT-SHARE' }));
  assert.deepEqual(refused, { taskId, state: 'authorization_required', waitId: 'wait_0001', messageId: null, expiresAt: null });
  assert.equal((await db.collection(`teams/${teamId}/waits`).get()).size, 0);
  const messages = (await chatRef().collection('messages').get()).docs.map(doc => doc.data());
  assert.equal(messages.length, 2); assert.ok(!JSON.stringify(messages).includes('PRIVATE-QUESTION'));
  assert.ok(!(await chatRef().get()).data().lastMessage.preview.includes('PRIVATE'));
  assert.equal((await taskData(taskId)).state, 'authorization_required'); assert.equal((await lockRefFor().get()).exists, false);
  assert.deepEqual(await execute('pauseTask', pausePayload(started, { outputAudience: { mode: 'owner' }, question: 'PRIVATE-QUESTION-DO-NOT-SHARE' })), refused);
  assert.equal((await messageCount()), 2);
});
test('wait: pause with an extra Chat reader outside the input audience is refused', options, async () => {
  await enableWait();
  const started = await start(), ref = db.doc(`teams/${teamId}/sharedBots/sharedbot_01`), bot = (await ref.get()).data();
  await ref.update({ grants: bot.grants.map(g => ({ ...g, usePolicy: { mode: 'listed', members: [{ uid: member.uid, generation: 1 }] } })) });
  await chatRef().update({ memberUids: [member.uid, other.uid] });
  const refused = await execute('pauseTask', pausePayload(started));
  assert.equal(refused.state, 'authorization_required'); assert.equal((await db.collection(`teams/${teamId}/waits`).get()).size, 0);
});
test('wait: pause replay returns the same response and a conflicting wait is rejected', options, async () => {
  const { started, response } = await paused();
  const again = await execute('pauseTask', pausePayload(started, { pausedAt: now - 0 }));
  assert.deepEqual(again, response); assert.equal(await messageCount(), 2);
  assert.equal((await db.collection(`teams/${teamId}/waits`).get()).size, 1);
  await assert.rejects(execute('pauseTask', pausePayload(started, { question: 'A different question?' })), { code: 'WAIT_CONFLICT' });
  await assert.rejects(execute('pauseTask', pausePayload(started, { checkpointDigest: 'f'.repeat(64) })), { code: 'WAIT_CONFLICT' });
  assert.equal(await messageCount(), 2); assert.equal((await waitData('wait_0001')).checkpointDigest, checkpoint);
});
test('wait: cancel by requester, maintainer and a bystander in every task state', options, async () => {
  const submitOne = async () => (await execute('submit', submit(), member)).tasks[0].taskId;
  // queued
  const queued = await submitOne();
  await assert.rejects(execute('cancelTask', { taskId: queued }, other), { code: 'TASK_CANCEL_FORBIDDEN' });
  assert.deepEqual(await execute('cancelTask', { taskId: queued }, member), { taskId: queued, state: 'cancelled' });
  assert.equal((await taskData(queued)).state, 'cancelled');
  await assert.rejects(execute('cancelTask', { taskId: queued }, owner), { code: 'TASK_FINISHED' });
  assert.equal((await execute('startTask', { ...host, taskId: queued })).execute, false);
  assert.equal((await execute('listHostTasks', host)).tasks.length, 0);
  await assert.rejects(execute('cancelTask', { taskId: 'task_missing_01' }, member), { code: 'TASK_REQUIRED' });
  // queued, cancelled by the maintainer
  const byOwner = await submitOne();
  assert.deepEqual(await execute('cancelTask', { taskId: byOwner }, owner), { taskId: byOwner, state: 'cancelled' });
  // running: flag, idempotent, validate refuses, one completion ends cancelled with no output
  const running = await start(), id = running.task.taskId;
  await assert.rejects(execute('cancelTask', { taskId: id }, other), { code: 'TASK_CANCEL_FORBIDDEN' });
  now += 1000;
  assert.deepEqual(await execute('cancelTask', { taskId: id }, owner), { taskId: id, state: 'running' });
  const flagged = (await taskData(id)).cancelRequestedAt; now += 1000;
  assert.deepEqual(await execute('cancelTask', { taskId: id }, member), { taskId: id, state: 'running' });
  assert.equal((await taskData(id)).cancelRequestedAt, flagged);
  assert.equal((await execute('getTask', { taskId: id }, member)).cancelRequestedAt, flagged);
  await assert.rejects(execute('validateTask', { ...host, taskId: id, permitId: running.permitId, expectedLastChangeSeq: 0 }), { code: 'TASK_CANCELLED' });
  const messages = await messageCount();
  const payload = result(running, { usage: { status: 'complete', inputTokens: 5, outputTokens: 7 } });
  const done = await execute('completeTask', payload);
  assert.deepEqual(done, { taskId: id, state: 'cancelled', replyId: null, artifactId: null });
  const ended = await taskData(id);
  assert.equal(ended.state, 'cancelled'); assert.deepEqual(ended.usage, payload.usage); assert.ok(ended.resultDigest);
  assert.equal(await messageCount(), messages); assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 0);
  assert.equal((await lockRefFor().get()).exists, false);
  assert.deepEqual(await execute('completeTask', payload), done);
  await assert.rejects(execute('completeTask', { ...payload, text: 'different' }), { code: 'TASK_RESULT_CONFLICT' });
  await assert.rejects(execute('cancelTask', { taskId: id }, owner), { code: 'TASK_FINISHED' });
  // waiting, then a bystander, then the maintainer
  await enableWait();
  const second = await start(), waiting = second.task.taskId; now += 1000;
  await execute('pauseTask', pausePayload(second));
  await assert.rejects(execute('cancelTask', { taskId: waiting }, other), { code: 'TASK_CANCEL_FORBIDDEN' });
  assert.deepEqual(await execute('cancelTask', { taskId: waiting }, owner), { taskId: waiting, state: 'cancelled' });
  assert.equal((await waitData('wait_0001')).state, 'revoked'); assert.equal((await lockRefFor().get()).exists, false);
  assert.equal((await execute('continueTask', continuePayload(waiting))).execute, false);
  // terminal (completed)
  const third = await start(), finished = await execute('completeTask', result(third));
  assert.equal(finished.state, 'completed');
  for (const actor of [member, owner, other]) await assert.rejects(execute('cancelTask', { taskId: third.task.taskId }, actor),
    actor === other ? { code: 'TASK_CANCEL_FORBIDDEN' } : { code: 'TASK_FINISHED' });
});
test('wait: a cancelled running task refuses continue and pause', options, async () => {
  await enableWait();
  const started = await start(), taskId = started.task.taskId;
  await execute('cancelTask', { taskId }, member);
  await assert.rejects(execute('pauseTask', pausePayload(started)), { code: 'TASK_CANCELLED' });
  assert.equal((await execute('continueTask', continuePayload(taskId, { reason: 'recovery' }))).execute, false);
  // The host never closed the run: past the deadline it reads as cancelled, not interrupted.
  now = started.task.deadline + 1;
  assert.equal((await execute('getTask', { taskId }, member)).state, 'cancelled');
  await assert.rejects(execute('cancelTask', { taskId }, member), { code: 'TASK_FINISHED' });
});
test('wait: another member request during a wait stays queued, then starts after the first task finishes', options, async () => {
  await enableWait();
  const group = 'chat_group01', groupRef = db.doc(`teams/${teamId}/conversations/${group}`);
  await execute('createConversation', { conversationId: group, kind: 'group', title: 'Group', participantUids: [member.uid, other.uid], sharedBotIds: ['sharedbot_01'] }, member);
  const send = (actor, n) => execute('submit', { conversationId: group, messageId: `group_msg_${n}`, changeId: `group_chg_${n}`, text: 'Hello', targets: ['sharedbot_01'] }, actor);
  const watermark = async () => (await groupRef.get()).data().lastChangeSeq;
  const first = await send(member, 1), firstId = first.tasks[0].taskId;
  const started = await execute('startTask', { ...host, taskId: firstId, expectedLastChangeSeq: await watermark() });
  now += 5000;
  const wait = await execute('pauseTask', pausePayload(started, { expiresInMs: 3600000 }));
  const second = await send(other, 2), secondId = second.tasks[0].taskId;
  assert.equal((await taskData(secondId)).deadline, wait.expiresAt + 300000);
  now += 20 * 60000; await heartbeat();
  assert.equal((await execute('startTask', { ...host, taskId: secondId, expectedLastChangeSeq: await watermark() })).execute, false);
  assert.equal((await execute('getTask', { taskId: secondId }, other)).state, 'queued');
  await execute('answerWait', answerPayload(undefined, { conversationId: group }), member);
  assert.equal((await execute('startTask', { ...host, taskId: secondId, expectedLastChangeSeq: await watermark() })).execute, false);
  const granted = await execute('continueTask', continuePayload(firstId));
  assert.equal(granted.execute, true);
  assert.equal((await execute('startTask', { ...host, taskId: secondId, expectedLastChangeSeq: await watermark() })).execute, false);
  const completion = { ...host, taskId: firstId, permitId: granted.permitId, outcome: 'completed', text: 'Done', finishedAt: now,
    inputGrantIds: granted.inputs.map(g => g.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } };
  assert.equal((await execute('completeTask', completion)).state, 'completed');
  const next = await execute('startTask', { ...host, taskId: secondId, expectedLastChangeSeq: await watermark() });
  assert.equal(next.execute, true); assert.equal(next.task.epoch, 1);
});
test('wait: a request queued behind a running task also outlives the running lock', options, async () => {
  const running = await start();
  const second = await execute('submit', submit(), member);
  assert.equal((await taskData(second.tasks[0].taskId)).deadline, running.task.deadline + 300000);
});
test('wait: start sets the running phase deadline and refuses a task that already ran', options, async () => {
  const receipt = await execute('submit', submit(), member), taskId = receipt.tasks[0].taskId;
  const queued = await taskData(taskId);
  assert.deepEqual([queued.epoch, queued.attemptStartedAt, queued.activeBudgetMs, queued.activeSpentMs, queued.physicalDispatchesSpent, queued.deadline],
    [0, 0, 300000, 0, 0, now + 300000]);
  now += 120000; await heartbeat();
  const started = await execute('startTask', { ...host, taskId });
  assert.equal(started.task.deadline, now + 300000); assert.equal(started.task.epoch, 1);
  const running = await taskData(taskId);
  assert.equal(running.attemptStartedAt, now); assert.deepEqual((await lockRefFor().get()).data(), { taskId, until: now + 300000 });
  await taskRef(taskId).update({ state: 'queued' });
  assert.equal((await execute('startTask', { ...host, taskId })).execute, false);
});

// ---- Operation ledger (M2 Q04) ----
const enableWrites = () => db.doc(`teams/${teamId}/deployments/${host.deploymentId}`).update({ 'capabilities.writeOperations': true });
const operationId = (taskId, step, category) => createHash('sha256').update(JSON.stringify([taskId, taskId, step, category])).digest('hex');
const operationData = async id => (await db.doc(`teams/${teamId}/operations/${id}`).get()).data();
const recordPayload = (started, { step = 'step/t1/c0', category = 'unqueryable', permitId = started.permitId, ...override } = {}) => ({ ...host,
  taskId: started.task.taskId, permitId, operationId: operationId(started.task.taskId, step, category), logicalStepId: step, category,
  tool: { name: 'shared_list.add', version: '1' }, digest: '1'.repeat(64), dispatchedAt: now, ...override });
const settlePayload = (started, recorded, override = {}) => ({ ...host, taskId: started.task.taskId, permitId: started.permitId,
  operationId: recorded.operationId, outcome: 'succeeded', resultDigest: '2'.repeat(64), settledAt: now, ...override });
const recover = (taskId, expectedEpoch = 1) => execute('continueTask', continuePayload(taskId, { reason: 'recovery', expectedEpoch }));

test('operations: the record is the only licence to dispatch, and its identity and digest never change', options, async () => {
  await enableWrites();
  const started = await start(), taskId = started.task.taskId, payload = recordPayload(started);
  await assert.rejects(execute('recordOperation', { ...payload, operationId: '9'.repeat(64) }), { code: 'OPERATION_ID_MISMATCH' });
  await assert.rejects(execute('recordOperation', { ...payload, permitId: 'permit_other' }), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(execute('recordOperation', { ...payload, dispatchedAt: now + 1 }), { code: 'TASK_EXPIRED' });
  assert.equal(await operationData(payload.operationId), undefined);
  const first = await execute('recordOperation', payload, owner, 'command_record_1');
  assert.deepEqual(first, { operationId: payload.operationId, state: 'dispatched', dispatch: true });
  // The same command (a lost response) is answered again; a second request for a non-repeatable operation is not a licence.
  assert.deepEqual(await execute('recordOperation', payload, owner, 'command_record_1'), first);
  assert.deepEqual(await execute('recordOperation', payload), { operationId: payload.operationId, state: 'dispatched', dispatch: false });
  await assert.rejects(execute('recordOperation', { ...payload, digest: '3'.repeat(64) }), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(execute('recordOperation', { ...payload, tool: { name: 'shared_list.add', version: '2' } }), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(execute('recordOperation', { ...payload, approvalRef: 'approval_01' }), { code: 'OPERATION_CONFLICT' });
  assert.deepEqual(await operationData(payload.operationId), { schemaVersion: 1, operationId: payload.operationId, rootTaskId: taskId, taskId,
    logicalStepId: 'step/t1/c0', category: 'unqueryable', tool: { name: 'shared_list.add', version: '1' }, digest: '1'.repeat(64), approvalRef: null,
    sharedBotId: 'sharedbot_01', conversationId, deploymentId: host.deploymentId, hostId: host.hostId, requester: { uid: member.uid, generation: 1 },
    epoch: 1, state: 'dispatched', dispatchedAt: now, recordedAt: now });
  assert.equal((await execute('getTask', { taskId }, member)).unverifiedOperations, 1);
  assert.equal((await taskData(taskId)).operationCount, 1);
  now += 1000;
  await assert.rejects(execute('settleOperation', settlePayload(started, payload, { permitId: 'permit_other' })), { code: 'EXECUTION_PERMIT_REQUIRED' });
  await assert.rejects(execute('settleOperation', settlePayload(started, payload, { operationId: '9'.repeat(64) })), { code: 'OPERATION_REQUIRED' });
  await assert.rejects(execute('settleOperation', settlePayload(started, payload, { settledAt: now - 2000 })), { code: 'OPERATION_CONFLICT' });
  const settled = settlePayload(started, payload, { reference: 'row-17' });
  assert.deepEqual(await execute('settleOperation', settled), { operationId: payload.operationId, state: 'succeeded' });
  assert.deepEqual(await execute('settleOperation', settled), { operationId: payload.operationId, state: 'succeeded' });
  await assert.rejects(execute('settleOperation', { ...settled, outcome: 'failed' }), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(execute('settleOperation', { ...settled, resultDigest: '4'.repeat(64) }), { code: 'OPERATION_CONFLICT' });
  assert.equal((await execute('getTask', { taskId }, member)).unverifiedOperations, undefined);
  // A later attempt that reaches the same step gets the receipt instead of a licence.
  assert.deepEqual(await execute('recordOperation', payload), { operationId: payload.operationId, state: 'succeeded', dispatch: false,
    resultDigest: '2'.repeat(64), reference: 'row-17' });
  assert.equal((await execute('completeTask', result(started))).state, 'completed');
});
test('operations: writes need the deployment capability and a task has a bounded number of operations', options, async () => {
  const started = await start();
  for (const category of ['store_idempotent', 'provider_idempotent', 'queryable', 'unqueryable']) {
    await assert.rejects(execute('recordOperation', recordPayload(started, { category })), { code: 'WRITE_OPERATIONS_UNSUPPORTED' });
  }
  for (let i = 0; i < 64; i += 1) assert.equal((await execute('recordOperation', recordPayload(started, { step: `step/t1/c${i}`, category: 'read' }))).dispatch, true);
  await assert.rejects(execute('recordOperation', recordPayload(started, { step: 'step/t1/c64', category: 'read' })), { code: 'OPERATION_LIMIT' });
  assert.equal((await execute('getTask', { taskId: started.task.taskId }, member)).unverifiedOperations, undefined);
});
test('operations: a recovered attempt may repeat a replay-safe operation; an unqueryable one sends the task to a person', options, async () => {
  await enableWrites();
  const started = await start(), taskId = started.task.taskId;
  const safe = recordPayload(started, { category: 'store_idempotent' });
  assert.equal((await execute('recordOperation', safe)).dispatch, true);
  const second = await recover(taskId);
  assert.equal(second.execute, true);
  await assert.rejects(execute('recordOperation', safe), { code: 'EXECUTION_PERMIT_REQUIRED' });
  now += 1000;
  assert.deepEqual(await execute('recordOperation', { ...safe, permitId: second.permitId, dispatchedAt: now }), { operationId: safe.operationId, state: 'dispatched', dispatch: true });
  assert.equal((await operationData(safe.operationId)).epoch, 2);
  now += 1000;
  const risky = recordPayload(started, { step: 'step/t2/c0', permitId: second.permitId });
  assert.equal((await execute('recordOperation', risky)).dispatch, true);
  assert.deepEqual(await recover(taskId, 2), { execute: false, taskId, state: 'needs_reconciliation' });
  assert.equal((await lockRefFor().get()).exists, false);
  const view = await execute('getTask', { taskId }, member);
  assert.equal(view.state, 'needs_reconciliation'); assert.equal(view.unverifiedOperations, 1);
  assert.equal((await execute('listHostTasks', host)).tasks.length, 0);
  assert.equal((await recover(taskId, 2)).execute, false);
  await assert.rejects(execute('cancelTask', { taskId }, member), { code: 'TASK_FINISHED' });
  await assert.rejects(execute('completeTask', result({ ...started, permitId: second.permitId })), { code: 'TASK_EXPIRED' });
  // Only the Bot's maintainer says what happened.
  const resolve = { taskId, operationId: risky.operationId, resolution: 'failed' };
  await assert.rejects(execute('resolveOperation', resolve, member), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(execute('resolveOperation', { ...resolve, operationId: '9'.repeat(64) }), { code: 'OPERATION_REQUIRED' });
  await assert.rejects(execute('resolveOperation', { ...resolve, operationId: safe.operationId }), { code: 'OPERATION_CONFLICT' });
  now += 5000;
  assert.deepEqual(await execute('resolveOperation', resolve), { taskId, state: 'interrupted', operationId: risky.operationId, operationState: 'failed' });
  assert.deepEqual(await execute('resolveOperation', resolve), { taskId, state: 'interrupted', operationId: risky.operationId, operationState: 'failed' });
  await assert.rejects(execute('resolveOperation', { ...resolve, resolution: 'succeeded' }), { code: 'OPERATION_CONFLICT' });
  const after = await execute('getTask', { taskId }, member);
  assert.equal(after.state, 'interrupted'); assert.equal(after.unverifiedOperations, undefined);
  const listed = await execute('listOperations', { taskId }, member);
  assert.deepEqual(listed.operations.map(o => [o.logicalStepId, o.category, o.state, o.epoch]),
    [['step/t1/c0', 'store_idempotent', 'dispatched', 2], ['step/t2/c0', 'unqueryable', 'failed', 2]]);
  assert.deepEqual(listed.operations[1].resolvedBy, { uid: owner.uid, generation: 1 });
  assert.deepEqual(Object.keys(listed.operations[0]).sort(), ['category', 'dispatchedAt', 'epoch', 'logicalStepId', 'operationId', 'state', 'tool']);
  assert.equal((await execute('listOperations', { taskId })).operations.length, 2);
  await assert.rejects(execute('listOperations', { taskId }, other));
});
test('operations: a queryable operation is settled by the host that still holds the permit before the task continues', options, async () => {
  await enableWrites();
  const started = await start(), taskId = started.task.taskId, payload = recordPayload(started, { category: 'queryable' });
  await execute('recordOperation', payload);
  assert.deepEqual(await recover(taskId), { execute: false, taskId, reconcile: [payload.operationId] });
  assert.equal((await taskData(taskId)).state, 'running'); assert.equal((await taskData(taskId)).epoch, 1);
  // The outside system could not say yet: the task stays blocked, and a later definite answer still closes it.
  assert.equal((await execute('settleOperation', settlePayload(started, payload, { outcome: 'unknown' }))).state, 'unknown');
  assert.equal((await execute('getTask', { taskId }, member)).unverifiedOperations, 1);
  assert.equal((await execute('settleOperation', settlePayload(started, payload, { reference: 'order-9' }))).state, 'succeeded');
  const second = await recover(taskId);
  assert.equal(second.execute, true); assert.equal(second.epoch, 2);
  assert.equal((await execute('completeTask', result({ ...started, permitId: second.permitId }))).state, 'completed');
});
test('operations: a result delivered over an unknown outcome waits for the maintainer and then takes the state it earned', options, async () => {
  await enableWrites();
  const started = await start(), taskId = started.task.taskId, payload = recordPayload(started);
  await execute('recordOperation', payload);
  assert.equal((await execute('settleOperation', settlePayload(started, payload, { outcome: 'unknown' }))).state, 'unknown');
  // An unqueryable unknown cannot be upgraded by the host.
  await assert.rejects(execute('settleOperation', settlePayload(started, payload)), { code: 'OPERATION_CONFLICT' });
  const delivered = await execute('completeTask', result(started));
  assert.equal(delivered.state, 'needs_reconciliation'); assert.ok(delivered.replyId);
  assert.equal((await chatRef().collection('messages').doc(delivered.replyId).get()).data().text, 'Pack your passports.');
  assert.deepEqual(await execute('completeTask', result(started)), delivered);
  assert.equal((await lockRefFor().get()).exists, false);
  assert.deepEqual(await execute('resolveOperation', { taskId, operationId: payload.operationId, resolution: 'succeeded' }),
    { taskId, state: 'completed', operationId: payload.operationId, operationState: 'succeeded' });
  const closed = await taskData(taskId);
  assert.equal(closed.state, 'completed'); assert.equal(closed.unverifiedOperations, 0); assert.equal(closed.reconciledState, undefined); assert.equal(closed.reconciledAt, now);
});
test('operations: cancelling or running out of time never hides an operation that was already sent', options, async () => {
  await enableWrites();
  const started = await start(), taskId = started.task.taskId, payload = recordPayload(started);
  await execute('recordOperation', payload);
  await assert.rejects(execute('resolveOperation', { taskId, operationId: payload.operationId, resolution: 'succeeded' }), { code: 'TASK_RUNNING' });
  assert.equal((await execute('cancelTask', { taskId }, member)).state, 'running');
  await assert.rejects(execute('recordOperation', recordPayload(started, { step: 'step/t2/c0' })), { code: 'TASK_CANCELLED' });
  assert.deepEqual(await execute('recordOperation', payload), { operationId: payload.operationId, state: 'dispatched', dispatch: false });
  const ended = await execute('completeTask', result(started, { outcome: 'failed', document: undefined }));
  assert.equal(ended.state, 'cancelled');
  const view = await execute('getTask', { taskId }, member);
  assert.equal(view.state, 'cancelled'); assert.equal(view.unverifiedOperations, 1);
  assert.equal((await execute('resolveOperation', { taskId, operationId: payload.operationId, resolution: 'succeeded' })).state, 'cancelled');
  assert.equal((await execute('getTask', { taskId }, member)).unverifiedOperations, undefined);
  // A run the host abandoned: the deadline turns it into a task to check, and into an interrupted one once checked.
  const next = await start(), nextId = next.task.taskId, abandoned = recordPayload(next);
  await execute('recordOperation', abandoned);
  now = next.task.deadline;
  assert.equal((await execute('getTask', { taskId: nextId }, member)).state, 'needs_reconciliation');
  assert.equal((await execute('resolveOperation', { taskId: nextId, operationId: abandoned.operationId, resolution: 'failed' })).state, 'interrupted');
  assert.equal((await execute('getTask', { taskId: nextId }, member)).state, 'interrupted');
});

// ---- Shared documents: revisions, conflicts and history (M2 Q07) ----
const revisionData = async (artifactId, revision) => (await db.doc(`teams/${teamId}/artifacts/${artifactId}/revisions/${revision}`).get()).data();
const edit = (artifactId, expectedRevision, body, operation = `edit_${++serial}_op`, actor = member, commandId) =>
  execute('reviseArtifact', { artifactId, expectedRevision, operationId: operation, document: { title: 'Packing list', body } }, actor, commandId);
async function delivered() {
  const started = await start(), done = await execute('completeTask', result(started));
  return { started, taskId: started.task.taskId, artifactId: done.artifactId };
}
test('documents: three edits of one document give one document at revision 4 that every reader sees', options, async () => {
  const { artifactId, taskId } = await delivered();
  assert.deepEqual(await revisionData(artifactId, 1), { schemaVersion: 1, artifactId, revision: 1, parentRevision: null,
    operationId: createHash('sha256').update(JSON.stringify([taskId, 'document'])).digest('hex'),
    digest: (await execute('readArtifact', { artifactId }, member)).digest, document: { title: 'Packing list', body: 'Pack your passports.' },
    author: { kind: 'shared_bot', sharedBotId: 'sharedbot_01' }, sourceTaskId: taskId, createdAt: now });
  for (const [i, body] of ['Passports, chargers.', 'Passports, chargers, adapters.', 'Passports, chargers, adapters, tickets.'].entries()) {
    now += 1000;
    const saved = await edit(artifactId, i + 1, body);
    assert.equal(saved.revision, i + 2); assert.match(saved.digest, /^[a-f0-9]{64}$/u);
  }
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 1);
  for (const reader of [member, owner]) {
    const head = await execute('readArtifact', { artifactId }, reader);
    assert.equal(head.revision, 4); assert.equal(head.currentRevision, 4); assert.equal(head.document.body, 'Passports, chargers, adapters, tickets.');
  }
  const first = await execute('readArtifact', { artifactId, revision: 1 }, member);
  assert.equal(first.document.body, 'Pack your passports.'); assert.equal(first.revision, 1); assert.equal(first.currentRevision, 4);
  await assert.rejects(execute('readArtifact', { artifactId, revision: 9 }, member), { code: 'ARTIFACT_REVISION_REQUIRED' });
  const history = await execute('listArtifactRevisions', { artifactId }, member);
  assert.equal(history.currentRevision, 4);
  assert.deepEqual(history.revisions.map(r => [r.revision, r.parentRevision, r.author.kind, r.sourceTaskId ?? null]),
    [[4, 3, 'user', null], [3, 2, 'user', null], [2, 1, 'user', null], [1, null, 'shared_bot', taskId]]);
  assert.deepEqual(history.revisions[0].author, { kind: 'user', uid: member.uid, generation: 1 });
  assert.equal(history.revisions[0].document, undefined);
});
test('documents: two edits made from the same revision never overwrite each other', options, async () => {
  const { artifactId } = await delivered();
  const outcomes = await Promise.allSettled([edit(artifactId, 1, 'Version A'), edit(artifactId, 1, 'Version B')]);
  const saved = outcomes.filter(o => o.status === 'fulfilled'), refused = outcomes.filter(o => o.status === 'rejected');
  assert.equal(saved.length, 1); assert.equal(refused.length, 1); assert.equal(refused[0].reason.code, 'ARTIFACT_REVISION_CONFLICT');
  const head = await execute('readArtifact', { artifactId }, member);
  assert.equal(head.revision, 2); assert.ok(['Version A', 'Version B'].includes(head.document.body));
  // The loser reads the current revision and saves onto it.
  assert.equal((await edit(artifactId, 2, 'Merged version')).revision, 3);
  await assert.rejects(edit(artifactId, 1, 'Late'), { code: 'ARTIFACT_REVISION_CONFLICT' });
  assert.equal((await execute('readArtifact', { artifactId }, member)).document.body, 'Merged version');
});
test('documents: a repeated edit returns the revision it made; an edit needs a change, a participant and a reader', options, async () => {
  const { artifactId } = await delivered();
  const saved = await edit(artifactId, 1, 'Passports and visas.', 'edit_operation_1', member, 'command_edit_1');
  assert.deepEqual(await edit(artifactId, 1, 'Passports and visas.', 'edit_operation_1', member, 'command_edit_1'), saved);
  // The response was lost and the client asks again with a new command: same operation, same revision, nothing new.
  assert.deepEqual(await edit(artifactId, 1, 'Passports and visas.', 'edit_operation_1'), saved);
  assert.equal((await execute('readArtifact', { artifactId }, member)).revision, 2);
  await assert.rejects(edit(artifactId, 1, 'Something else.', 'edit_operation_1'), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(edit(artifactId, 2, 'Passports and visas.', 'edit_operation_1'), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(edit(artifactId, 2, 'Passports and visas.'), { code: 'ARTIFACT_UNCHANGED' });
  // The Bot's owner reads this Bot DM without taking part in it; an outsider reads nothing.
  await assert.rejects(edit(artifactId, 2, 'Owner edit', undefined, owner), { code: 'CONVERSATION_REQUIRED' });
  for (const action of ['readArtifact', 'listArtifactRevisions']) await assert.rejects(execute(action, { artifactId }, other), { code: 'ARTIFACT_READ_REQUIRED' });
  await assert.rejects(edit(artifactId, 2, 'Outsider edit', undefined, other), { code: 'ARTIFACT_READ_REQUIRED' });
  await assert.rejects(execute('reviseArtifact', { artifactId, expectedRevision: 2, operationId: 'edit_operation_2', document: { title: ' ', body: 'x' } }, member));
  assert.equal((await execute('readArtifact', { artifactId }, member)).revision, 2);
});
test('documents: a Bot revises the document it read, and a revision of a stale copy is not saved', options, async () => {
  const { artifactId } = await delivered();
  const second = await start(), revised = result(second, { text: 'Added the adapters.', document: { title: 'Packing list', body: 'Passports, adapters.' },
    revises: { artifactId, expectedRevision: 1 } });
  const done = await execute('completeTask', revised);
  assert.deepEqual({ state: done.state, artifactId: done.artifactId }, { state: 'completed', artifactId });
  assert.deepEqual(await execute('completeTask', revised), done);
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, 1);
  const head = await execute('readArtifact', { artifactId }, member);
  assert.equal(head.revision, 2); assert.equal(head.document.body, 'Passports, adapters.');
  const stored = await revisionData(artifactId, 2);
  assert.deepEqual({ parent: stored.parentRevision, author: stored.author, source: stored.sourceTaskId },
    { parent: 1, author: { kind: 'shared_bot', sharedBotId: 'sharedbot_01' }, source: second.task.taskId });
  assert.equal((await chatRef().collection('messages').doc(done.replyId).get()).data().artifactId, artifactId);
  // A member edits while the Bot works from revision 2: the Bot's revision is refused, the reply says so and nothing is saved.
  const third = await start();
  await edit(artifactId, 2, 'Passports, adapters, tickets.');
  const late = await execute('completeTask', result(third, { text: 'Added sunscreen.', document: { title: 'Packing list', body: 'Passports, adapters, sunscreen.' },
    revises: { artifactId, expectedRevision: 2 } }));
  assert.deepEqual({ state: late.state, artifactId: late.artifactId }, { state: 'failed', artifactId: null });
  const notice = (await chatRef().collection('messages').doc(late.replyId).get()).data();
  assert.notEqual(notice.text, 'Added sunscreen.'); assert.equal(notice.artifactId, undefined);
  const kept = await execute('readArtifact', { artifactId }, member);
  assert.equal(kept.revision, 3); assert.equal(kept.document.body, 'Passports, adapters, tickets.');
  // A revision of a document of another Chat, an unknown document, or without a document is refused the same way or by the contract.
  const fourth = await start();
  const unknown = await execute('completeTask', result(fourth, { revises: { artifactId: 'artifact_unknown', expectedRevision: 1 } }));
  assert.deepEqual({ state: unknown.state, artifactId: unknown.artifactId }, { state: 'failed', artifactId: null });
  const fifth = await start();
  await assert.rejects(execute('completeTask', result(fifth, { document: undefined, revises: { artifactId, expectedRevision: 3 } })));
});

// ---- Shared list (M2 Q06) ----
const canonical = value => Array.isArray(value) ? value.map(canonical) : value !== null && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const digestOf = value => createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');
const groceries = { title: 'Groceries', description: 'What the household needs to buy.' };
let publication = 1;
/** Publish the Bot again, with or without its list, on a host that does or does not record writes. */
async function republish({ sharedList = groceries, writeOperations = true, durableWait = false, ...override } = {}) {
  const base = publish(); publication += 1;
  const published = await execute('publishSharedBot', { ...base, expectedRevision: publication - 1, ...(sharedList ? { sharedList } : {}),
    deployment: { ...base.deployment, revision: publication, capabilities: { durableWait, writeOperations, delegation: false } }, ...override });
  await execute('heartbeatDeployment', { ...host, ready: true });
  return published;
}
const add = text => ({ op: 'add', text });
const memberWrite = (operationId, expectedDataRevision, changes, actor = member) =>
  execute('writeList', { sharedBotId: 'sharedbot_01', operationId, expectedDataRevision, changes }, actor);
const readList = (actor = member) => execute('readList', { sharedBotId: 'sharedbot_01' }, actor);
const listHistory = (beforeRevision, actor = member) =>
  execute('listHistory', { sharedBotId: 'sharedbot_01', ...(beforeRevision === undefined ? {} : { beforeRevision }) }, actor);
const by = actor => ({ kind: 'user', uid: actor.uid, generation: 1 });
/** Record a task's list write in the ledger, as the host does before it sends the write. */
async function recordWrite(started, step, write, permitId = started.permitId, approvalRef) {
  const payload = recordPayload(started, { step, category: 'store_idempotent', permitId, tool: { name: 'TeamListWrite', version: '1' }, digest: digestOf(write),
    ...(approvalRef ? { approvalRef } : {}) });
  return { payload, recorded: await execute('recordOperation', payload) };
}
const taskWrite = (started, operationId, write, permitId = started.permitId) =>
  execute('writeTaskList', { ...host, taskId: started.task.taskId, permitId, operationId, ...write });

test('list: one list per Bot that members change in turn; an outdated change is refused and nothing is duplicated', options, async () => {
  publication = 1;
  await assert.rejects(readList(), { code: 'LIST_REQUIRED' });
  await republish();
  assert.deepEqual((await execute('listSharedBots', {}, member)).bots[0].sharedList, { title: 'Groceries' });
  assert.deepEqual(await readList(), { sharedBotId: 'sharedbot_01', ...groceries, dataRevision: 0, items: [], updatedAt: now });
  now += 1000;
  assert.deepEqual(await memberWrite('operation_01', 0, [add('  Milk ')]), { sharedBotId: 'sharedbot_01', dataRevision: 1 });
  // A request whose answer was lost is answered again and adds nothing.
  assert.deepEqual(await memberWrite('operation_01', 0, [add('  Milk ')]), { sharedBotId: 'sharedbot_01', dataRevision: 1 });
  await assert.rejects(memberWrite('operation_01', 0, [add('Bread')]), { code: 'OPERATION_CONFLICT' });
  await assert.rejects(memberWrite('operation_01', 0, [add('  Milk ')], other), { code: 'OPERATION_CONFLICT' });
  // Someone who still looks at revision 0 saves nothing.
  await assert.rejects(memberWrite('operation_02', 0, [add('Bread')], other), { code: 'LIST_REVISION_CONFLICT' });
  await assert.rejects(memberWrite('operation_03', 1, [{ op: 'remove', itemId: 'item_9999' }], other), { code: 'LIST_ITEM_REQUIRED' });
  await assert.rejects(memberWrite('operation_04', 1, [add('Bread'), { op: 'update', itemId: 'item_9999', done: true }], other), { code: 'LIST_ITEM_REQUIRED' });
  assert.deepEqual((await readList(other)).items, [{ itemId: 'item_0001', text: 'Milk', done: false }]);
  assert.equal((await memberWrite('operation_05', 1, [add('Bread'), { op: 'update', itemId: 'item_0001', done: true }], other)).dataRevision, 2);
  assert.equal((await memberWrite('operation_06', 2, [{ op: 'remove', itemId: 'item_0001' }, { op: 'update', itemId: 'item_0002', text: 'Rye bread' }], owner)).dataRevision, 3);
  const list = await readList();
  assert.deepEqual({ dataRevision: list.dataRevision, items: list.items }, { dataRevision: 3, items: [{ itemId: 'item_0002', text: 'Rye bread', done: false }] });
  // Every saved write is a revision that says who made it and what each item was before; refused writes left none.
  assert.deepEqual(await listHistory(undefined, other), { sharedBotId: 'sharedbot_01', dataRevision: 3, hasMore: false, revisions: [
    { dataRevision: 3, author: by(owner), createdAt: now, changes: [
      { op: 'remove', itemId: 'item_0001', before: { text: 'Milk', done: true } },
      { op: 'update', itemId: 'item_0002', text: 'Rye bread', before: { text: 'Bread', done: false } }] },
    { dataRevision: 2, author: by(other), createdAt: now, changes: [
      { op: 'add', itemId: 'item_0002', text: 'Bread' }, { op: 'update', itemId: 'item_0001', done: true, before: { text: 'Milk', done: false } }] },
    { dataRevision: 1, author: by(member), createdAt: now, changes: [{ op: 'add', itemId: 'item_0001', text: 'Milk' }] }] });
  await assert.rejects(execute('listHistory', { sharedBotId: 'sharedbot_01', beforeRevision: 0 }, member));
  for (const invalid of [[], [add('')], [add('x'.repeat(501))], [{ op: 'update', itemId: 'item_0002' }], [{ op: 'clear' }], Array.from({ length: 21 }, () => add('x'))]) {
    await assert.rejects(memberWrite(`operation_bad_${++serial}`, 3, invalid));
  }
  // Publishing again keeps the items; a publication without the list hides them without deleting them.
  await republish({ sharedList: { title: 'Shopping', description: 'Renamed.' } });
  assert.deepEqual({ title: (await readList()).title, items: (await readList()).items }, { title: 'Shopping', items: list.items });
  await republish({ sharedList: null });
  await assert.rejects(readList(), { code: 'LIST_REQUIRED' });
  await assert.rejects(memberWrite('operation_07', 3, [add('Eggs')]), { code: 'LIST_REQUIRED' });
  await republish({ usePolicy: { mode: 'listed', members: [{ uid: member.uid, generation: 1 }] } });
  assert.deepEqual((await readList()).items, list.items);
  await assert.rejects(readList(other), { code: 'LIST_REQUIRED' });
  await assert.rejects(listHistory(undefined, other), { code: 'LIST_REQUIRED' });
  assert.equal((await readList(owner)).dataRevision, 3);
  const filled = Array.from({ length: 20 }, (_, i) => add(`Item ${i}`));
  let revision = 3;
  for (let i = 0; i < 24; i += 1) revision = (await memberWrite(`operation_fill_${i}`, revision, filled)).dataRevision;
  await assert.rejects(memberWrite('operation_full', revision, filled), { code: 'LIST_LIMIT' });
  // The whole list is one answer to the Bot, so it is bounded in bytes too: long items fill it sooner.
  revision = (await memberWrite('operation_empty', revision, (await readList()).items.slice(0, 20).map(item => ({ op: 'remove', itemId: item.itemId })))).dataRevision;
  const long = Array.from({ length: 19 }, () => add('x'.repeat(500)));
  for (let i = 0; i < 2; i += 1) revision = (await memberWrite(`operation_long_${i}`, revision, long)).dataRevision;
  const short = (await readList()).items.slice(0, 20).map(item => ({ op: 'update', itemId: item.itemId, text: 'y'.repeat(500) }));
  await assert.rejects(memberWrite('operation_long_full', revision, short), { code: 'LIST_LIMIT' });
  assert.equal((await readList()).items.length, 499);
  // History comes a page at a time, newest first.
  const first = await listHistory();
  assert.equal(first.hasMore, true);
  assert.deepEqual(first.revisions.map(r => r.dataRevision), Array.from({ length: 20 }, (_, i) => revision - i));
  const second = await listHistory(first.revisions.at(-1).dataRevision);
  assert.deepEqual(second.revisions.map(r => r.dataRevision), Array.from({ length: revision - 20 }, (_, i) => revision - 20 - i));
  assert.equal(second.hasMore, false);
  // Only the latest 200 revisions are kept.
  const toggled = (await readList()).items[0].itemId;
  for (let i = 0; i < 200; i += 1) revision = (await memberWrite(`operation_toggle_${i}`, revision, [{ op: 'update', itemId: toggled, done: i % 2 === 0 }])).dataRevision;
  const kept = (await db.collection('teams').doc(teamId).collection('lists').doc('sharedbot_01').collection('revisions').get()).docs.map(doc => doc.data().dataRevision);
  assert.deepEqual([kept.length, Math.min(...kept), Math.max(...kept)], [200, revision - 199, revision]);
  let pages = 0, before;
  for (let page = await listHistory(); ; page = await listHistory(before)) { pages += 1; before = page.revisions.at(-1).dataRevision; if (!page.hasMore) break; }
  assert.deepEqual([pages, before], [10, revision - 199]);
});
test('list: a task writes through its ledger operation; the change and the outcome are saved together and only once', options, async () => {
  publication = 1;
  const plain = await start();
  assert.equal(Object.hasOwn(plain, 'sharedList'), false);
  await execute('completeTask', result(plain));
  await republish({ writeOperations: false });
  const unsupported = await start();
  assert.equal(Object.hasOwn(unsupported, 'sharedList'), false);
  await assert.rejects(execute('readTaskList', { ...host, taskId: unsupported.task.taskId, permitId: unsupported.permitId }), { code: 'LIST_REQUIRED' });
  await execute('completeTask', result(unsupported));
  await republish();
  await memberWrite('operation_01', 0, [add('Milk')]);
  const started = await start(), taskId = started.task.taskId;
  assert.deepEqual(started.sharedList, groceries);
  assert.deepEqual(await execute('readTaskList', { ...host, taskId, permitId: started.permitId }), { dataRevision: 1, items: [{ itemId: 'item_0001', text: 'Milk', done: false }] });
  await assert.rejects(execute('readTaskList', { ...host, taskId, permitId: 'permit_other' }), { code: 'EXECUTION_PERMIT_REQUIRED' });
  const write = { expectedDataRevision: 1, changes: [add('Bread'), { op: 'update', itemId: 'item_0001', done: true }] };
  // Without its ledger record the write is not accepted, and a record for other parameters does not cover it.
  await assert.rejects(taskWrite(started, operationId(taskId, `${taskId}/t0/c0`, 'store_idempotent'), write), { code: 'OPERATION_REQUIRED' });
  const { payload, recorded } = await recordWrite(started, `${taskId}/t0/c0`, write);
  assert.equal(recorded.dispatch, true);
  await assert.rejects(taskWrite(started, payload.operationId, { ...write, changes: [add('Wine')] }), { code: 'OPERATION_REQUIRED' });
  now += 1000;
  const items = [{ itemId: 'item_0001', text: 'Milk', done: true }, { itemId: 'item_0002', text: 'Bread', done: false }];
  assert.deepEqual(await taskWrite(started, payload.operationId, write), { applied: true, dataRevision: 2, items });
  const settled = await operationData(payload.operationId);
  assert.deepEqual({ state: settled.state, resultDigest: settled.resultDigest, settledAt: settled.settledAt },
    { state: 'succeeded', resultDigest: digestOf({ applied: true, dataRevision: 2 }), settledAt: now });
  // The answer was lost: the same write again changes nothing, and the ledger gives a receipt instead of a licence.
  assert.deepEqual(await taskWrite(started, payload.operationId, write), { applied: true, dataRevision: 2, items });
  assert.equal((await execute('recordOperation', payload)).dispatch, false);
  assert.equal((await readList()).dataRevision, 2);
  assert.equal((await execute('getTask', { taskId }, member)).unverifiedOperations, undefined);
  // A write made from an outdated read saves nothing and is settled as failed.
  const stale = { expectedDataRevision: 1, changes: [add('Eggs')] }, second = await recordWrite(started, `${taskId}/t1/c0`, stale);
  assert.deepEqual(await taskWrite(started, second.payload.operationId, stale), { applied: false, error: 'DATA_REVISION_CONFLICT', dataRevision: 2, items });
  assert.equal((await operationData(second.payload.operationId)).state, 'failed');
  // The Bot's revision names the request it was made for; the failed one left none.
  const botRevisions = (await listHistory()).revisions;
  assert.deepEqual(botRevisions.map(r => r.dataRevision), [2, 1]);
  assert.deepEqual(botRevisions[0].author, { kind: 'shared_bot', sharedBotId: 'sharedbot_01', taskId, requester: { uid: member.uid, generation: 1 } });
  assert.deepEqual(await taskWrite(started, second.payload.operationId, stale), { applied: false, error: 'DATA_REVISION_CONFLICT', dataRevision: 2, items });
  // The worker died after the record: the next attempt repeats the same operation and it is applied once.
  const third = { expectedDataRevision: 2, changes: [add('Eggs')] }, pending = await recordWrite(started, `${taskId}/t2/c0`, third);
  const again = await recover(taskId);
  assert.equal(again.execute, true); assert.deepEqual(again.sharedList, groceries);
  await assert.rejects(taskWrite(started, pending.payload.operationId, third), { code: 'EXECUTION_PERMIT_REQUIRED' });
  now += 1000;
  assert.equal((await execute('recordOperation', { ...pending.payload, permitId: again.permitId, dispatchedAt: now })).dispatch, true);
  assert.equal((await taskWrite(started, pending.payload.operationId, third, again.permitId)).dataRevision, 3);
  assert.equal((await taskWrite(started, pending.payload.operationId, third, again.permitId)).dataRevision, 3);
  // What the Bot saved is what the members see.
  assert.deepEqual((await readList(other)).items.map(item => item.text), ['Milk', 'Bread', 'Eggs']);
  // A cancelled task changes nothing more.
  const last = { expectedDataRevision: 3, changes: [add('Wine')] }, late = await recordWrite(started, `${taskId}/t3/c0`, last, again.permitId);
  await execute('cancelTask', { taskId }, member);
  await assert.rejects(taskWrite(started, late.payload.operationId, last, again.permitId), { code: 'TASK_CANCELLED' });
  assert.equal((await readList()).dataRevision, 3);
});

// M2 Q05: a list change of a run that its requester has to approve first.
const asking = { ...groceries, approval: 'requester' };
/** The pause a host sends when its worker stopped at a list change that needs approval. */
function approvalPause(started, step, write, override = {}) {
  const { question: _question, ...base } = pausePayload(started);
  return { ...base, kind: 'tool_approval', approval: { operationId: operationId(started.task.taskId, step, 'store_idempotent'), logicalStepId: step, ...write }, ...override };
}
const decide = (n, decision, actor = member, waitId = 'wait_0001') =>
  execute('decideWait', { conversationId, waitId, messageId: `decide_${n}`, changeId: `decidechg_${n}`, decision }, actor);
const messageData = async id => (await chatRef().collection('messages').doc(id).get()).data();

test('approval: a run changes the list only after its requester approved exactly that change', options, async () => {
  publication = 1;
  await republish({ durableWait: true });
  const free = await start(), freeStep = `${free.task.taskId}/t0/c0`, bread = { expectedDataRevision: 0, changes: [add('Bread')] };
  // A list that asks nobody takes no approval request.
  await assert.rejects(execute('pauseTask', approvalPause(free, freeStep, bread)), { code: 'APPROVAL_UNSUPPORTED' });
  await execute('completeTask', result(free));
  await republish({ sharedList: asking, durableWait: true });
  assert.deepEqual((await execute('listSharedBots', {}, member)).bots[0].sharedList, { title: 'Groceries', approval: 'requester' });
  await memberWrite('operation_01', 0, [add('Milk')]);
  const started = await start(), taskId = started.task.taskId, step = `${taskId}/t0/c0`;
  assert.deepEqual(started.sharedList, asking);
  const write = { expectedDataRevision: 1, changes: [add(' Bread '), { op: 'remove', itemId: 'item_0001' }, { op: 'update', itemId: 'item_0404', done: true }] };
  // Nothing is recorded, so nothing can be sent, before the approval exists.
  await assert.rejects(recordWrite(started, step, write), { code: 'APPROVAL_REQUIRED' });
  await assert.rejects(recordWrite(started, step, write, started.permitId, 'wait_0001'), { code: 'APPROVAL_REQUIRED' });
  await assert.rejects(execute('pauseTask', approvalPause(started, step, write, { approval: { operationId: 'e'.repeat(64), logicalStepId: step, ...write } })), { code: 'OPERATION_ID_MISMATCH' });
  now += 1000;
  const waiting = await execute('pauseTask', approvalPause(started, step, write));
  assert.equal(waiting.state, 'waiting');
  assert.deepEqual(await execute('pauseTask', approvalPause(started, step, write)), waiting);
  const asked = await messageData(waiting.messageId);
  // The member sees the items as they read now; an item that does not exist is shown by its id.
  assert.deepEqual({ text: asked.text, waitId: asked.waitId, approval: asked.approval }, { text: 'Asks to change the list “Groceries” (3 changes).', waitId: 'wait_0001',
    approval: { title: 'Groceries', changes: [{ op: 'add', text: 'Bread' }, { op: 'remove', itemId: 'item_0001', item: 'Milk' }, { op: 'update', itemId: 'item_0404', done: true, item: null }] } });
  const view = await execute('getTask', { taskId }, member);
  assert.deepEqual([view.state, view.wait.kind, view.wait.recipient.uid], ['waiting', 'tool_approval', member.uid]);
  const stored = await waitData('wait_0001');
  assert.deepEqual([stored.operationId, stored.operationDigest], [operationId(taskId, step, 'store_idempotent'), digestOf(write)]);
  // An approval is not an answer, and only the member the run works for may give it.
  await assert.rejects(execute('answerWait', answerPayload(), member), { code: 'WAIT_KIND_MISMATCH' });
  await assert.rejects(decide(1, 'approve', other), { code: 'CONVERSATION_REQUIRED' });
  // A participant of the Chat who is not the requester sees the request and cannot decide it.
  const chat = (await chatRef().get()).data();
  await chatRef().update({ memberUids: ['member', 'other', 'owner'], participants: ['user:member', 'user:other', 'shared_bot:sharedbot_01'] });
  await assert.rejects(decide(1, 'approve', other), { code: 'WAIT_RECIPIENT_REQUIRED' });
  await chatRef().update({ memberUids: chat.memberUids, participants: chat.participants });
  assert.equal((await waitData('wait_0001')).state, 'pending');
  const decided = await decide(2, 'approve');
  assert.deepEqual(await decide(2, 'approve'), decided);
  await assert.rejects(decide(3, 'deny'), { code: 'WAIT_CLOSED' });
  const reply = await messageData('decide_2');
  assert.deepEqual([reply.text, reply.decision, reply.waitId, reply.replyTo], ['Approved', 'approve', 'wait_0001', waiting.messageId]);
  const queued = (await execute('listHostTasks', host)).tasks.find(task => task.taskId === taskId);
  assert.equal(queued.resume.decision, 'approve');
  const resumed = await execute('continueTask', continuePayload(taskId));
  assert.equal(resumed.execute, true); assert.equal(resumed.wait.decision, 'approve');
  // The approval covers this operation with these parameters, nothing else.
  for (const [at, changed] of [[step, { ...write, changes: [add('Wine')] }], [step, { ...write, expectedDataRevision: 2 }], [`${taskId}/t0/c1`, write]]) {
    await assert.rejects(recordWrite(started, at, changed, resumed.permitId, 'wait_0001'), { code: 'APPROVAL_REQUIRED' });
  }
  await assert.rejects(recordWrite(started, step, write, resumed.permitId), { code: 'APPROVAL_REQUIRED' });
  const fine = { expectedDataRevision: 1, changes: write.changes.slice(0, 2) };
  await assert.rejects(recordWrite(started, step, fine, resumed.permitId, 'wait_0001'), { code: 'APPROVAL_REQUIRED' });
  const { payload, recorded } = await recordWrite(started, step, write, resumed.permitId, 'wait_0001');
  assert.equal(recorded.dispatch, true);
  // What was approved names an item that is gone: the write is refused whole and the list is as it was.
  assert.deepEqual(await taskWrite(started, payload.operationId, write, resumed.permitId),
    { applied: false, error: 'LIST_ITEM_REQUIRED', dataRevision: 1, items: [{ itemId: 'item_0001', text: 'Milk', done: false }] });
  await execute('completeTask', result({ ...started, permitId: resumed.permitId }));
  // A second request, approved and unchanged since: saved once.
  const next = await start(), nextId = next.task.taskId, nextStep = `${nextId}/t0/c0`, change = { expectedDataRevision: 1, changes: [add('Bread'), { op: 'remove', itemId: 'item_0001' }] };
  now += 1000;
  await execute('pauseTask', approvalPause(next, nextStep, change, { waitId: 'wait_0002' }));
  await decide(4, 'approve', member, 'wait_0002');
  const go = await execute('continueTask', continuePayload(nextId));
  // The approval of another task's request opens nothing here.
  await assert.rejects(recordWrite(next, nextStep, change, go.permitId, 'wait_0001'), { code: 'APPROVAL_REQUIRED' });
  const approved = await recordWrite(next, nextStep, change, go.permitId, 'wait_0002');
  assert.deepEqual(await taskWrite(next, approved.payload.operationId, change, go.permitId), { applied: true, dataRevision: 2, items: [{ itemId: 'item_0002', text: 'Bread', done: false }] });
  assert.equal((await operationData(approved.payload.operationId)).approvalRef, 'wait_0002');
});
test('approval: a declined, expired, cancelled or revoked request saves nothing', options, async () => {
  publication = 1;
  await republish({ sharedList: asking, durableWait: true });
  const change = { expectedDataRevision: 0, changes: [add('Bread')] };
  async function asked(waitId, expiresInMs = 3600000) {
    const started = await start(), step = `${started.task.taskId}/t0/c0`; now += 1000;
    assert.equal((await execute('pauseTask', approvalPause(started, step, change, { waitId, expiresInMs }))).state, 'waiting');
    return { started, step, taskId: started.task.taskId };
  }
  // Declined: the task goes on so the Bot can say so, and the ledger takes no record under the denied request.
  const declined = await asked('wait_0001');
  await decide(1, 'deny');
  assert.deepEqual([(await waitData('wait_0001')).state, (await messageData('decide_1')).text], ['denied', 'Declined']);
  await assert.rejects(decide(2, 'approve'), { code: 'WAIT_CLOSED' });
  const resumed = await execute('continueTask', continuePayload(declined.taskId));
  assert.equal(resumed.wait.decision, 'deny');
  await assert.rejects(recordWrite(declined.started, declined.step, change, resumed.permitId, 'wait_0001'), { code: 'APPROVAL_REQUIRED' });
  await execute('completeTask', result({ ...declined.started, permitId: resumed.permitId }));
  // Not decided in time.
  const late = await asked('wait_0002', 60000); now += 61000;
  await assert.rejects(decide(3, 'approve', member, 'wait_0002'), { code: 'WAIT_CLOSED' });
  assert.equal((await execute('getTask', { taskId: late.taskId }, member)).state, 'expired');
  // Cancelled while it waits.
  const cancelled = await asked('wait_0003');
  await execute('cancelTask', { taskId: cancelled.taskId }, member);
  await assert.rejects(decide(4, 'approve', member, 'wait_0003'), { code: 'WAIT_CLOSED' });
  assert.equal((await waitData('wait_0003')).state, 'revoked');
  // The member lost the Bot while the request was open: their approval no longer counts.
  const revoked = await asked('wait_0004');
  await republish({ sharedList: asking, durableWait: true, usePolicy: { mode: 'listed', members: [{ uid: other.uid, generation: 1 }] } });
  await assert.rejects(decide(5, 'approve', member, 'wait_0004'), { code: 'TASK_REVOKED' });
  assert.equal((await readList(other)).dataRevision, 0);
  assert.deepEqual((await readList(other)).items, []);
  assert.equal(revoked.taskId.length > 0, true);
});

// ---- Shared knowledge: keep, revise, revoke (M3-1) ----
const knowledge = (inputs, expectedRevision, override = {}) => {
  const base = publish();
  return execute('publishSharedBot', { ...base, expectedRevision, inputs, deployment: { ...base.deployment, revision: expectedRevision + 1, ...override } });
};
const grantsOf = async () => Object.fromEntries((await db.doc(`teams/${teamId}/sharedBots/sharedbot_01`).get()).data().grants.map(grant => [grant.resourceId, grant.revision]));
test('knowledge: a published resource keeps its identity; the same revision is the same bytes and changed bytes are the next revision', options, async () => {
  const persona = input('persona', 'persona_01'), file = input('file', 'travel_file'), notes = input('file', 'notes_file'), changed = 'b'.repeat(64);
  const refused = { code: 'INPUT_REVISION_CONFLICT' };
  await assert.rejects(knowledge([persona, { ...file, digest: changed }], 1), refused);
  await assert.rejects(knowledge([persona, { ...file, digest: changed, revision: 3 }], 1), refused);
  await assert.rejects(knowledge([persona, { ...file, revision: 2 }], 1), refused);
  await assert.rejects(knowledge([persona, { ...file, kind: 'skill' }], 1), refused);
  await assert.rejects(knowledge([persona, file, { ...notes, revision: 2 }], 1), refused);
  assert.deepEqual(await grantsOf(), { persona_01: 1, travel_file: 1 });
  // One file is revised, one is added, the role is unchanged.
  const second = await knowledge([persona, { ...file, digest: changed, revision: 2 }, notes], 1);
  assert.equal(second.revision, 2); assert.equal(second.grantIds.length, 3);
  assert.deepEqual(await grantsOf(), { persona_01: 1, travel_file: 2, notes_file: 1 });
  // Leaving a resource out revokes it: the next publication has no grant for it.
  assert.equal((await knowledge([persona, notes], 2)).revision, 3);
  assert.deepEqual(await grantsOf(), { persona_01: 1, notes_file: 1 });
  // Shared again, it is a new resource: it does not continue the revisions of the revoked one.
  await assert.rejects(knowledge([persona, notes, { ...file, digest: changed, revision: 2 }], 3), refused);
  assert.equal((await knowledge([persona, notes, file], 3)).revision, 4);
});
test('knowledge: a task that began with a revoked source cannot go on or publish its result, and a new task is not given the source', options, async () => {
  const started = await start(), taskId = started.task.taskId;
  assert.deepEqual(started.inputs.map(grant => grant.resourceId).sort(), ['persona_01', 'travel_file']);
  await knowledge([input('persona', 'persona_01')], 1);
  await heartbeat();
  const current = (await chatRef().get()).data().lastChangeSeq;
  await assert.rejects(execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq: current }), { code: 'TASK_REVOKED' });
  await assert.rejects(recover(taskId), { code: 'TASK_REVOKED' });
  // What the run already drew from the source does not reach the Chat.
  const ended = await execute('completeTask', result(started, { text: 'FROM-THE-REVOKED-FILE', document: { title: 'Notes', body: 'FROM-THE-REVOKED-FILE' } }));
  assert.equal(ended.state, 'authorization_required'); assert.equal(ended.artifactId, null);
  assert.ok(!JSON.stringify((await chatRef().collection('messages').get()).docs.map(doc => doc.data())).includes('FROM-THE-REVOKED-FILE'));
  const next = await start();
  assert.deepEqual(next.inputs.map(grant => grant.resourceId), ['persona_01']);
  assert.equal((await execute('completeTask', result(next))).state, 'completed');
});
