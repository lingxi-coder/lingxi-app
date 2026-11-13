// M3-3: handing a shared Bot to another member (acceptance E2, E3, E5 and RV-06).
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { createHash } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 120000 };
const projectId = 'demo-lingxi-teams-execution', teamId = 'team_handover', botId = 'sharedbot_01', oldChat = 'chat_before_01', newChat = 'chat_after_001';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const oldHost = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 }, newHost = { deploymentId: 'deployment_02', hostId: 'host_0002', accountEpoch: 1 };
let app, db, teams, store, now, serial = 0;
const sha = value => createHash('sha256').update(value).digest('hex');
const command = (action, payload, generation, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation, action, payload });
const generations = new Map();
const execute = async (action, payload, actor = owner, commandId) => {
  if (action === 'startTask' && !Object.hasOwn(payload, 'expectedLastChangeSeq')) {
    const task = (await db.doc(`teams/${teamId}/tasks/${payload.taskId}`).get()).data();
    payload = { ...payload, expectedLastChangeSeq: (await db.doc(`teams/${teamId}/conversations/${task.conversationId}`).get()).data().lastChangeSeq };
  }
  return store.execute(actor, command(action, payload, generations.get(actor.uid) ?? 1, commandId));
};
const input = (kind, resourceId) => ({ kind, resourceId, revision: 1, digest: sha(resourceId), sourceAudience: { mode: 'team' } });
const deployment = (host, actor, generation = 1) => ({ ...host, sharedBotId: botId, revision: 1, owner: { uid: actor.uid, generation },
  instanceId: `instance_${actor.uid}`, definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
  capabilities: { durableWait: false, writeOperations: true, delegation: false }, enabled: true });
const publish = (override = {}) => ({ sharedBotId: botId, expectedRevision: 0, name: 'Travel assistant', description: 'Shared travel notes',
  usePolicy: { mode: 'all' }, deployment: deployment(oldHost, owner), inputs: [input('persona', 'persona_01'), input('file', 'travel_file')],
  quotas: { team: 100, member: 20, bot: 100 }, ...override });
const submit = (conversationId = oldChat, id = ++serial) => ({ conversationId, messageId: `message_${id}`, changeId: `change_${id}`, text: 'What should we pack?', targets: [botId] });
const result = (host, started, override = {}) => ({ ...host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: 'Pack your passports.', finishedAt: now,
  inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' },
  document: { title: 'Packing list', body: 'Pack your passports.' }, usage: { status: 'missing' }, ...override });
const botData = async () => (await db.doc(`teams/${teamId}/sharedBots/${botId}`).get()).data();
const taskData = async id => (await db.doc(`teams/${teamId}/tasks/${id}`).get()).data();
const chatData = async id => (await db.doc(`teams/${teamId}/conversations/${id}`).get()).data();
const transferData = async id => (await db.doc(`teams/${teamId}/transfers/${id}`).get()).data();
const partCount = async () => (await db.collection(`teams/${teamId}/sharedBots/${botId}/transferParts`).get()).size;
const card = async actor => (await execute('listSharedBots', {}, actor)).bots.find(bot => bot.sharedBotId === botId);
const manifest = [{ resourceId: 'persona_01', revision: 1, digest: sha('persona_01'), kind: 'persona' }, { resourceId: 'travel_file', revision: 1, digest: sha('travel_file'), kind: 'file' }];
/** The published role and files as the maintainer's Desktop sends them: bytes in parts, here two. */
const packageBytes = Buffer.from(JSON.stringify({ documents: [{ resourceId: 'persona_01', text: 'Explicit role 角色' }, { resourceId: 'travel_file', text: 'x'.repeat(4000) }] }));
const parts = [packageBytes.subarray(0, 2000).toString('base64'), packageBytes.subarray(2000).toString('base64')];
async function offer(transferId, to = other, { expectedRevision = 1, toGeneration = 1, actor = owner } = {}) {
  for (const [index, data] of parts.entries()) await execute('putTransferPart', { transferId, sharedBotId: botId, index, data }, actor);
  return execute('offerTransfer', { transferId, sharedBotId: botId, expectedRevision, to: { uid: to.uid, generation: toGeneration }, parts: parts.length, packageDigest: sha(packageBytes) }, actor);
}
const complete = (transferId, actor = other, override = {}, commandId) => execute('completeTransfer', { transferId, deployment: deployment(newHost, actor), inputs: manifest, ...override }, actor, commandId);
async function handOver(transferId = 'transfer_0001') {
  await offer(transferId); await execute('acceptTransfer', { transferId }, other);
  const done = await complete(transferId); await execute('heartbeatDeployment', { ...newHost, ready: true }, other); return done;
}
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'shared-bot-handover-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  await db.recursiveDelete(db.collection('teams')); await db.recursiveDelete(db.collection('users')); generations.clear();
  now = Date.UTC(2026, 10, 12); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Handover test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  await execute('publishSharedBot', publish());
  await execute('heartbeatDeployment', { ...oldHost, ready: true });
  await execute('createConversation', { conversationId: oldChat, kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('handover: the Bot keeps its entry, its audience and its shared results; authority moves to the new maintainer; earlier Chats keep the readers they had (E2, RV-06)', options, async () => {
  // Before: a delivered document, a task that is running and one that is still queued, all in the member's DM with the Bot.
  const first = await execute('submit', submit(), member), firstStarted = await execute('startTask', { ...oldHost, taskId: first.tasks[0].taskId });
  const delivered = await execute('completeTask', result(oldHost, firstStarted)); now += 1000;
  const running = await execute('submit', submit(), member), runningStarted = await execute('startTask', { ...oldHost, taskId: running.tasks[0].taskId });
  const queuedMessage = submit(), queued = await execute('submit', queuedMessage, member);
  const before = { bot: await botData(), chat: await chatData(oldChat), tasks: await Promise.all([first, running, queued].map(receipt => taskData(receipt.tasks[0].taskId))) };
  assert.deepEqual(before.chat.memberUids, ['member', 'owner']);

  // The offer changes nothing for the people who use the Bot, and only its two parties see it.
  const offered = await offer('transfer_0001');
  assert.deepEqual([offered.state, offered.from, offered.to, offered.manifest], ['offered', { uid: 'owner', generation: 1 }, { uid: 'other', generation: 1 }, manifest]);
  assert.equal((await card(member)).transfer, undefined); assert.equal((await card(owner)).transfer.state, 'offered'); assert.equal((await card(other)).transfer.transferId, 'transfer_0001');
  assert.deepEqual((await botData()).identity, before.bot.identity);
  await assert.rejects(execute('readTransferPart', { transferId: 'transfer_0001', index: 0 }, other), { code: 'TRANSFER_ENDED' }, 'nothing is read before accepting');
  const accepted = await execute('acceptTransfer', { transferId: 'transfer_0001' }, other);
  assert.deepEqual([accepted.state, accepted.name, accepted.parts, accepted.packageDigest], ['accepted', 'Travel assistant', 2, sha(packageBytes)]);
  const received = Buffer.concat(await Promise.all([0, 1].map(async index => Buffer.from((await execute('readTransferPart', { transferId: 'transfer_0001', index }, other)).data, 'base64'))));
  assert.ok(received.equals(packageBytes), 'the new maintainer receives the published role and files, byte for byte');
  await assert.rejects(execute('readTransferPart', { transferId: 'transfer_0001', index: 0 }, owner), { code: 'TRANSFER_RECIPIENT_REQUIRED' });
  assert.equal((await card(member)).maintainer.uid, 'owner', 'accepted is not yet handed over');

  const done = await complete('transfer_0001'); await execute('heartbeatDeployment', { ...newHost, ready: true }, other);
  const bot = await botData();
  assert.deepEqual(done, { sharedBotId: botId, revision: 2, grantIds: bot.grants.map(grant => grant.grantId) });
  assert.deepEqual(bot.identity, { ...before.bot.identity, maintainer: { uid: 'other', generation: 1 }, revision: 2, activeDeploymentId: newHost.deploymentId });
  assert.deepEqual([bot.name, bot.description, bot.quotas, bot.sharedList, bot.transfer], [before.bot.name, before.bot.description, before.bot.quotas, before.bot.sharedList, undefined]);
  assert.deepEqual(bot.grants.map(({ grantId, publisher, ...grant }) => grant), before.bot.grants.map(({ grantId, publisher, ...grant }) => grant), 'the same inputs for the same audience');
  assert.ok(bot.grants.every(grant => grant.publisher.uid === 'other'));
  assert.equal(await partCount(), 0, 'the package is not kept'); assert.equal((await transferData('transfer_0001')).state, 'completed');
  const entry = await card(member);
  assert.deepEqual([entry.sharedBotId, entry.name, entry.maintainer.uid, entry.permitted, entry.ready, entry.revision], [botId, 'Travel assistant', 'other', true, true, 2], 'the same entry, now served by the new maintainer');

  // RV-06: the earlier DM and its tasks are what they were. The new maintainer was never one of its readers.
  assert.deepEqual(await chatData(oldChat), before.chat);
  for (const [index, receipt] of [first, running, queued].entries()) {
    const task = await taskData(receipt.tasks[0].taskId);
    assert.deepEqual([task.maintainer, task.deploymentId, task.requester, task.botRevision], [{ uid: 'owner', generation: 1 }, oldHost.deploymentId, before.tasks[index].requester, 1]);
  }
  await assert.rejects(execute('listTasks', { conversationId: oldChat }, other), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute('readArtifact', { artifactId: delivered.artifactId }, other), { code: 'ARTIFACT_READ_REQUIRED' });
  await assert.rejects(execute('getTask', { taskId: first.tasks[0].taskId }, other));
  // The shared result and the task references stay usable for the people who had them.
  assert.equal((await execute('readArtifact', { artifactId: delivered.artifactId }, member)).document.body, 'Pack your passports.');
  assert.equal((await execute('readArtifact', { artifactId: delivered.artifactId }, owner)).document.title, 'Packing list');
  assert.equal((await execute('getTask', { taskId: first.tasks[0].taskId }, member)).artifactId, delivered.artifactId);
  assert.equal((await execute('listTasks', { conversationId: oldChat }, member)).tasks.length, 3);

  // The earlier maintainer has no say over the Bot any more, and their computer gets nothing to do.
  await assert.rejects(execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 2 }), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(execute('publishSharedBot', publish({ expectedRevision: 2, deployment: { ...deployment(oldHost, owner), revision: 2 } })), { code: 'MAINTAINER_REQUIRED' });
  for (const action of ['heartbeatDeployment', 'listHostTasks']) await assert.rejects(execute(action, action === 'heartbeatDeployment' ? { ...oldHost, ready: true } : oldHost), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(execute('completeTask', result(oldHost, runningStarted)), { code: 'DEPLOYMENT_REQUIRED' }, 'a run of the earlier deployment is not carried over');
  // Tasks admitted before the switch are not handed to the new deployment, and the same request gives no second task (E3).
  assert.deepEqual((await execute('listHostTasks', newHost, other)).tasks, []);
  assert.deepEqual(await execute('submit', queuedMessage, member), queued);
  assert.equal((await db.collection(`teams/${teamId}/tasks`).get()).size, 3);
  const stateOf = async receipt => (await execute('getTask', { taskId: receipt.tasks[0].taskId }, member)).state;
  now += 301000; assert.deepEqual([await stateOf(running), await stateOf(queued)], ['interrupted', 'queued']);
  now = (await taskData(queued.tasks[0].taskId)).deadline; assert.equal(await stateOf(queued), 'expired', 'it waited its time and nobody took it');

  // The earlier DM takes no new request for the Bot: its readers are the member and the earlier maintainer.
  const messages = (await db.collection(`teams/${teamId}/conversations/${oldChat}/messages`).get()).size;
  await assert.rejects(execute('submit', submit(), member), { code: 'BOT_MAINTAINER_CHANGED' });
  assert.equal((await db.collection(`teams/${teamId}/conversations/${oldChat}/messages`).get()).size, messages);
  // A new DM is created with the new maintainer as its reader, and only the new deployment serves it.
  await execute('heartbeatDeployment', { ...newHost, ready: true }, other);
  const created = await execute('createConversation', { conversationId: newChat, kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member);
  assert.deepEqual(created.readers, ['member', 'other']);
  const fresh = await execute('submit', submit(newChat), member), task = await taskData(fresh.tasks[0].taskId);
  assert.deepEqual([task.maintainer, task.deploymentId, task.botRevision], [{ uid: 'other', generation: 1 }, newHost.deploymentId, 2]);
  const started = await execute('startTask', { ...newHost, taskId: fresh.tasks[0].taskId }, other);
  assert.equal(started.execute, true); await execute('completeTask', result(newHost, started), other);
  assert.equal((await execute('getTask', { taskId: fresh.tasks[0].taskId }, member)).state, 'completed');
  await assert.rejects(execute('listTasks', { conversationId: newChat }, owner), { code: 'CONVERSATION_REQUIRED' }, 'the earlier maintainer does not read the new DM');
});

test('handover: every step has one answer that can be asked for again (E3)', options, async () => {
  // Who may do what.
  await assert.rejects(offer('transfer_self1', owner), { code: 'CURRENT_RECIPIENT_REQUIRED' });
  await assert.rejects(offer('transfer_gone1', { uid: 'stranger' }), { code: 'CURRENT_RECIPIENT_REQUIRED' });
  await assert.rejects(offer('transfer_notme', other, { actor: member }), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(execute('offerTransfer', { transferId: 'transfer_empty', sharedBotId: botId, expectedRevision: 1, to: { uid: 'other', generation: 1 }, parts: 1, packageDigest: sha('x') }), { code: 'TRANSFER_PACKAGE_REQUIRED' });
  await execute('putTransferPart', { transferId: 'transfer_wrong', sharedBotId: botId, index: 0, data: parts[0] });
  await assert.rejects(execute('offerTransfer', { transferId: 'transfer_wrong', sharedBotId: botId, expectedRevision: 1, to: { uid: 'other', generation: 1 }, parts: 1, packageDigest: sha('x') }), { code: 'TRANSFER_PACKAGE_MISMATCH' });

  const offered = await offer('transfer_0001');
  assert.equal(await partCount(), 2, 'parts of an offer that was never made are dropped');
  const again = { transferId: 'transfer_0001', sharedBotId: botId, expectedRevision: 1, to: { uid: 'other', generation: 1 }, parts: parts.length, packageDigest: sha(packageBytes) };
  assert.deepEqual(await execute('offerTransfer', again), offered, 'the same offer again is the same offer');
  await assert.rejects(execute('offerTransfer', { ...again, to: { uid: 'member', generation: 1 } }), { code: 'TRANSFER_ID_CONFLICT' });
  await assert.rejects(execute('putTransferPart', { transferId: 'transfer_0001', sharedBotId: botId, index: 0, data: parts[1] }), { code: 'TRANSFER_SEALED' });
  await assert.rejects(offer('transfer_0002', member), { code: 'TRANSFER_PENDING' });
  for (const action of ['getTransfer', 'acceptTransfer', 'endTransfer']) await assert.rejects(execute(action, { transferId: 'transfer_0001' }, member), { code: 'TRANSFER_PARTY_REQUIRED' });
  await assert.rejects(execute('acceptTransfer', { transferId: 'transfer_0001' }), { code: 'TRANSFER_RECIPIENT_REQUIRED' });
  await assert.rejects(complete('transfer_0001'), { code: 'TRANSFER_ENDED' }, 'not before it was accepted');

  // Accepting twice (the first answer was lost) is accepting once.
  const accepted = await execute('acceptTransfer', { transferId: 'transfer_0001' }, other);
  assert.deepEqual(await execute('acceptTransfer', { transferId: 'transfer_0001' }, other), accepted);
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' })).state, 'accepted');
  // What is published must be exactly what was handed over, from the new maintainer's own deployment.
  await assert.rejects(complete('transfer_0001', other, { inputs: [manifest[0], { ...manifest[1], digest: sha('changed') }] }), { code: 'TRANSFER_PACKAGE_MISMATCH' });
  await assert.rejects(complete('transfer_0001', other, { inputs: [manifest[0]] }), { code: 'TRANSFER_PACKAGE_MISMATCH' });
  await assert.rejects(complete('transfer_0001', other, { deployment: deployment(newHost, owner) }), { code: 'INVALID_DEPLOYMENT' });
  await assert.rejects(complete('transfer_0001', other, { deployment: deployment(oldHost, other) }), { code: 'DEPLOYMENT_REVISION_CONFLICT' });
  assert.equal((await botData()).identity.maintainer.uid, 'owner', 'a failed step leaves the Bot where it was');
  // Completing twice, with the same command or a new one, switches once.
  const done = await complete('transfer_0001', other, {}, 'complete_cmd_1');
  assert.deepEqual(await complete('transfer_0001', other, {}, 'complete_cmd_1'), done);
  assert.deepEqual(await complete('transfer_0001', other, {}, 'complete_cmd_2'), done);
  assert.equal((await botData()).identity.revision, 2);
  await assert.rejects(complete('transfer_0001', other, { deployment: deployment({ ...newHost, deploymentId: 'deployment_03' }, other) }), { code: 'TRANSFER_ENDED' });
  await assert.rejects(execute('endTransfer', { transferId: 'transfer_0001' }), { code: 'TRANSFER_COMPLETED' });
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' })).state, 'completed');
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' }, other)).newRevision, 2);
});

test('handover: taking an offer back, declining it, a new publication, a pause and seven days each end it for good (E3)', options, async () => {
  // Taken back by the maintainer.
  await offer('transfer_0001');
  const taken = await execute('endTransfer', { transferId: 'transfer_0001' });
  assert.equal(taken.state, 'cancelled'); assert.deepEqual(await execute('endTransfer', { transferId: 'transfer_0001' }), taken);
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' }, other)).state, 'cancelled');
  assert.equal(await partCount(), 0); assert.equal((await card(other)).transfer, undefined);
  await assert.rejects(execute('acceptTransfer', { transferId: 'transfer_0001' }, other), { code: 'TRANSFER_ENDED' });
  // Declined by the member it was offered to.
  await offer('transfer_0002');
  assert.equal((await execute('endTransfer', { transferId: 'transfer_0002' }, other)).state, 'declined');
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0002' })).state, 'declined'); assert.equal((await card(owner)).transfer, undefined);
  // Declined after accepting: the Bot stays with its maintainer.
  await offer('transfer_0003'); await execute('acceptTransfer', { transferId: 'transfer_0003' }, other);
  assert.equal((await execute('endTransfer', { transferId: 'transfer_0003' }, other)).state, 'declined');
  await assert.rejects(complete('transfer_0003'), { code: 'TRANSFER_ENDED' }); assert.equal((await botData()).identity.maintainer.uid, 'owner');
  // A new publication or a pause is not what was offered.
  await offer('transfer_0004');
  await execute('publishSharedBot', publish({ expectedRevision: 1, deployment: { ...deployment(oldHost, owner), revision: 2 } }));
  assert.deepEqual([(await transferData('transfer_0004')).state, (await transferData('transfer_0004')).reason, await partCount()], ['cancelled', 'republished', 0]);
  await offer('transfer_0005', other, { expectedRevision: 2 }); await execute('acceptTransfer', { transferId: 'transfer_0005' }, other);
  await execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 2 });
  assert.deepEqual([(await execute('getTransfer', { transferId: 'transfer_0005' }, other)).state, (await botData()).transfer, await partCount()], ['cancelled', undefined, 0]);
  await assert.rejects(complete('transfer_0005'), { code: 'TRANSFER_ENDED' });
  await assert.rejects(offer('transfer_0006', other, { expectedRevision: 3 }), { code: 'BOT_PAUSED' });
  // Seven days.
  await execute('publishSharedBot', publish({ expectedRevision: 3, deployment: { ...deployment(oldHost, owner), revision: 3 } }));
  await offer('transfer_0007', other, { expectedRevision: 4 }); now += 7 * 24 * 60 * 60 * 1000;
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0007' })).state, 'expired');
  await assert.rejects(execute('acceptTransfer', { transferId: 'transfer_0007' }, other), { code: 'TRANSFER_ENDED' });
  assert.equal((await card(owner)).transfer, undefined);
  // An offer that ran out does not stand in the way of the next one.
  assert.equal((await offer('transfer_0008', member, { expectedRevision: 4 })).state, 'offered');
  assert.deepEqual([(await transferData('transfer_0007')).state, (await transferData('transfer_0007')).reason], ['cancelled', 'replaced']);
});

test('handover: a party who leaves the team ends the offer, and joining again does not bring it back (E3)', options, async () => {
  await offer('transfer_0001'); await execute('acceptTransfer', { transferId: 'transfer_0001' }, other);
  await teams.execute(owner, { action: 'remove', teamId, uid: other.uid, generation: 1 });
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' })).state, 'void');
  assert.equal((await execute('getTransfer', { transferId: 'transfer_0001' })).state, 'void', 'asked again, the same');
  assert.equal((await card(owner)).transfer, undefined);
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_02', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  const joined = await teams.execute(other, { action: 'join', code, requestId: 'join_again' }); generations.set(other.uid, joined.generation);
  assert.equal(joined.generation, 2);
  await assert.rejects(execute('acceptTransfer', { transferId: 'transfer_0001' }, other), { code: 'TRANSFER_PARTY_REQUIRED' }, 'the offer was made to the earlier membership');
  await assert.rejects(complete('transfer_0001'), { code: 'TRANSFER_PARTY_REQUIRED' });
  assert.equal((await botData()).identity.maintainer.uid, 'owner');
  // The maintainer offers again, to the membership that exists now.
  assert.equal((await offer('transfer_0002', other, { toGeneration: 2 })).state, 'offered');
  await execute('acceptTransfer', { transferId: 'transfer_0002' }, other);
  const done = await execute('completeTransfer', { transferId: 'transfer_0002', deployment: deployment(newHost, other, 2), inputs: manifest }, other);
  assert.equal(done.revision, 2); assert.deepEqual((await botData()).identity.maintainer, { uid: 'other', generation: 2 });
});

test('handover: maintaining a Bot, owning the team, paying for the model and the data audience change separately (E5)', options, async () => {
  // Data: a Bot for the owner and one member cannot be handed to someone outside that audience, and a handover adds nobody to it.
  await execute('publishSharedBot', publish({ expectedRevision: 1, usePolicy: { mode: 'listed', members: [{ uid: 'owner', generation: 1 }, { uid: 'other', generation: 1 }] },
    deployment: { ...deployment(oldHost, owner), revision: 2 } }));
  await execute('heartbeatDeployment', { ...oldHost, ready: true });
  await assert.rejects(offer('transfer_0001', member, { expectedRevision: 2 }), { code: 'INPUT_GRANT_REQUIRED' });
  await offer('transfer_0002', other, { expectedRevision: 2 }); await execute('acceptTransfer', { transferId: 'transfer_0002' }, other);
  const before = await botData(); await complete('transfer_0002'); await execute('heartbeatDeployment', { ...newHost, ready: true }, other);
  const bot = await botData();
  assert.deepEqual([bot.identity.usePolicy, bot.grants.map(grant => grant.audience), bot.quotas], [before.identity.usePolicy, before.grants.map(grant => grant.audience), before.quotas]);
  assert.equal(await card(member), undefined, 'the member who could not use the Bot still cannot see it');
  await assert.rejects(execute('createConversation', { conversationId: 'chat_member_01', kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member));
  // Maintaining is not owning: the new maintainer is an ordinary member, and the team's owner no longer decides about the Bot.
  const roles = async () => Object.fromEntries((await db.collection(`teams/${teamId}/members`).get()).docs.map(doc => [doc.id, doc.data().role]));
  assert.deepEqual(await roles(), { owner: 'owner', member: 'member', other: 'member' });
  await assert.rejects(execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 3 }), { code: 'MAINTAINER_REQUIRED' });
  await assert.rejects(teams.execute(other, { action: 'role', teamId, uid: member.uid, generation: 1, role: 'admin' }), { code: 'ROLE_REQUIRED' });
  // Paying: a request after the switch is given to the new maintainer's deployment alone.
  await execute('createConversation', { conversationId: 'chat_owner_001', kind: 'bot_dm', title: 'Travel', participantUids: [owner.uid], sharedBotIds: [botId] });
  const asked = await execute('submit', submit('chat_owner_001'), owner), task = await taskData(asked.tasks[0].taskId);
  assert.deepEqual([task.deploymentId, task.maintainer.uid], [newHost.deploymentId, 'other']);
  await assert.rejects(execute('startTask', { ...oldHost, taskId: asked.tasks[0].taskId }), { code: 'DEPLOYMENT_REQUIRED' });
  const started = await execute('startTask', { ...newHost, taskId: asked.tasks[0].taskId }, other); assert.equal(started.execute, true);
  // Owning the team moves by itself too: the member who gets the team gets no Bot with it, reads no Chat they could not read, and hosts nothing.
  const chats = async () => Object.fromEntries((await db.collection(`teams/${teamId}/conversations`).get()).docs.map(doc => [doc.id, doc.data().memberUids]));
  const botBefore = await botData(), chatsBefore = await chats();
  await teams.execute(owner, { action: 'handOver', teamId, uid: member.uid, generation: 1 });
  assert.deepEqual(await roles(), { owner: 'admin', member: 'owner', other: 'member' });
  assert.deepEqual([await botData(), await chats()], [botBefore, chatsBefore]);
  for (const actor of [member, owner]) await assert.rejects(execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 3 }, actor), { code: 'MAINTAINER_REQUIRED' });
  assert.equal(await card(member), undefined, 'the new owner of the team still cannot use the Bot');
  await assert.rejects(execute('listTasks', { conversationId: 'chat_owner_001' }, member), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute('listHostTasks', newHost, member), { code: 'DEPLOYMENT_REQUIRED' });
  // The earlier owner's request and the Bot's maintainer are untouched by it.
  assert.equal((await execute('completeTask', result(newHost, started), other)).state, 'completed');
  // The new maintainer decides about the Bot as any maintainer does.
  assert.equal((await execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 3 }, other)).revision, 4);
});

test('handover: what a task did outside the Chat stays with the maintainer it ran under', options, async () => {
  const asked = await execute('submit', submit(), member), started = await execute('startTask', { ...oldHost, taskId: asked.tasks[0].taskId });
  const taskId = started.task.taskId, step = 'step/t1/c0';
  const recorded = await execute('recordOperation', { ...oldHost, taskId, permitId: started.permitId, operationId: sha(JSON.stringify([taskId, taskId, step, 'unqueryable'])),
    logicalStepId: step, category: 'unqueryable', tool: { name: 'shared_list.add', version: '1' }, digest: '1'.repeat(64), dispatchedAt: now });
  await handOver(); now += 301000;
  assert.equal((await execute('getTask', { taskId: started.task.taskId }, member)).state, 'needs_reconciliation');
  await assert.rejects(execute('listOperations', { taskId: started.task.taskId }, other), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute('resolveOperation', { taskId: started.task.taskId, operationId: recorded.operationId, resolution: 'failed' }, other), { code: 'MAINTAINER_REQUIRED' });
  assert.equal((await execute('listOperations', { taskId: started.task.taskId })).operations.length, 1);
  const resolved = await execute('resolveOperation', { taskId: started.task.taskId, operationId: recorded.operationId, resolution: 'failed' });
  assert.equal(resolved.operationState, 'failed');
});

test('handover: a document made before it stays readable, and a later publication ends it as before', options, async () => {
  const asked = await execute('submit', submit(), member), started = await execute('startTask', { ...oldHost, taskId: asked.tasks[0].taskId });
  const { artifactId } = await execute('completeTask', result(oldHost, started));
  await handOver();
  assert.equal((await execute('readArtifact', { artifactId }, member)).document.title, 'Packing list');
  assert.equal((await botData()).sourceRevision, 1);
  // The new maintainer hands it on again without publishing: still the same inputs.
  await offer('transfer_0002', member, { expectedRevision: 2, actor: other }); await execute('acceptTransfer', { transferId: 'transfer_0002' }, member);
  await execute('completeTransfer', { transferId: 'transfer_0002', deployment: deployment({ ...newHost, deploymentId: 'deployment_03', hostId: 'host_0003' }, member), inputs: manifest }, member);
  assert.equal((await execute('readArtifact', { artifactId }, member)).document.title, 'Packing list'); assert.equal((await botData()).sourceRevision, 1);
  // A new publication changes the inputs: documents made from the earlier ones are no longer served.
  await execute('publishSharedBot', publish({ expectedRevision: 3, deployment: { ...deployment({ ...newHost, deploymentId: 'deployment_03', hostId: 'host_0003' }, member), revision: 2 } }), member);
  assert.equal((await botData()).sourceRevision, undefined);
  await assert.rejects(execute('readArtifact', { artifactId }, member), { code: 'ARTIFACT_READ_REQUIRED' });
});
