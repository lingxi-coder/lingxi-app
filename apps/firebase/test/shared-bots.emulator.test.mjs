import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
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
