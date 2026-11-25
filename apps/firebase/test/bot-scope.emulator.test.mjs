// Bot v3 B4a: what a shared Bot may use of its owner's Desktop in a Team: the scope it published, what a Chat narrows it to, and what the Data service lets through.
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { createBotDataRecords } from '../src/lingxi-data-bot-records.mjs';
import { emptyScope } from '../src/bot-scope.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-bot-scope', teamId = 'team_scope01', botId = 'sharedbot_01', receiverId = 'sharedbot_02';
const dm = 'chat_scope_dm1', group = 'chat_scope_grp';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const receiverHost = { deploymentId: 'deployment_02', hostId: 'host_0002', accountEpoch: 1 };
const sold = 'dataset_01', counted = 'dataset_02', unlisted = 'dataset_03';
const row = (dataset, more = {}) => ({ source: 'orders', dataset, write: false, reportReview: false, ...more });
/** Two Local Apps, a dataset the Bot may write and open reviews in, one it may only read. */
const published = { localApps: [{ id: 'calendar', name: 'Calendar' }, { id: 'notes', name: 'Notes' }],
  datasets: [row(sold, { write: true, reportReview: true }), row(counted)] };
let app, db, teams, store, messages, now, serial = 0, realFetch;
const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner, commandId) => store.execute(actor, command(action, payload, commandId));
const publication = (sharedBotId, deploymentHost, revision, scope) => ({ sharedBotId, expectedRevision: revision - 1,
  name: sharedBotId === botId ? 'Travel assistant' : 'Receiving assistant', description: '', usePolicy: { mode: 'all' },
  deployment: { ...deploymentHost, sharedBotId, revision, owner: { uid: owner.uid, generation: 1 }, instanceId: `instance_${sharedBotId}`,
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: false, writeOperations: true, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 }, ...(scope ? { scope } : {}) });
const botData = async (id = botId) => (await db.doc(`teams/${teamId}/sharedBots/${id}`).get()).data();
const chatData = async (id = dm) => (await db.doc(`teams/${teamId}/conversations/${id}`).get()).data();
const card = async (id = botId, actor = member) => (await execute('listSharedBots', {}, actor)).bots.find(entry => entry.sharedBotId === id);
/** Publish the Bot again, with this scope or without one, and bring its host back online. */
async function publishScope(scope, id = botId, deploymentHost = host) {
  await execute('publishSharedBot', publication(id, deploymentHost, (await botData(id)).identity.revision + 1, scope));
  await execute('heartbeatDeployment', { ...deploymentHost, ready: true });
}
const narrow = (scope, expectedRevision, { conversationId = dm, sharedBotId = botId, actor = member, commandId } = {}) =>
  execute('setConversationBotScope', { conversationId, sharedBotId, expectedRevision, scope }, actor, commandId);
async function ask(conversationId, actor, id = ++serial) {
  const receipt = await execute('submit', { conversationId, messageId: `message_${id}`, changeId: `change_${id}`, text: 'Check my orders', targets: [botId] }, actor);
  return { taskId: receipt.tasks[0].taskId, expectedLastChangeSeq: receipt.changeSeq };
}

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'bot-scope-tests'); db = getFirestore(app); realFetch = globalThis.fetch;
});
beforeEach(async () => {
  if (!db) return;
  globalThis.fetch = realFetch;
  for (const name of ['teams', 'users', 'lingxiDataBindings']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 24); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Scope test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  await execute('publishSharedBot', publication(botId, host, 1));
  await execute('heartbeatDeployment', { ...host, ready: true });
  await execute('createConversation', { conversationId: dm, kind: 'bot_dm', title: 'Orders', participantUids: [member.uid], sharedBotIds: [botId] }, member);
});
after(async () => { if (realFetch) globalThis.fetch = realFetch; await db?.terminate(); if (app) await deleteApp(app); });

test('publishing: the scope is stored as ids, names and flags, is none unless given, and each publication replaces the last', options, async () => {
  assert.deepEqual((await botData()).scope, emptyScope, 'a Bot published without a scope shares nothing');
  assert.equal((await card()).scope, undefined, 'and its entry in the directory says nothing of one');

  await publishScope({ localApps: [{ id: 'calendar', name: '  Calendar ' }], datasets: [row(sold, { write: true })] });
  const stored = { localApps: [{ id: 'calendar', name: 'Calendar' }], datasets: [row(sold, { write: true })] };
  assert.deepEqual((await botData()).scope, stored);
  assert.deepEqual((await card()).scope, stored);
  assert.equal((await botData()).identity.revision, 2, 'the scope is part of the publication: it moves the revision like the inputs do');

  await publishScope(published);
  assert.deepEqual((await botData()).scope, published);
  await publishScope(undefined);
  assert.deepEqual((await botData()).scope, emptyScope, 'what an earlier publication shared is not kept by a later one that leaves it out');
  assert.equal((await card()).scope, undefined);
});

test('publishing: a scope that is not ids, names and flags, or is repeated or too long, is refused and nothing changes', options, async () => {
  const bad = [
    { localApps: [{ id: 'calendar', name: 'Calendar', path: '/Users/owner/Apps/calendar' }], datasets: [] },
    { localApps: [], datasets: [{ ...row(sold), token: 'secret' }] },
    { localApps: [{ id: 'calendar', name: 'Calendar' }, { id: 'calendar', name: 'Again' }], datasets: [] },
    { localApps: [], datasets: [row(sold), row(sold, { source: 'other' })] },
    { localApps: Array.from({ length: 65 }, (_, i) => ({ id: `app${i}`, name: 'App' })), datasets: [] },
    { localApps: [], datasets: [{ source: 'orders', dataset: sold, write: 'yes', reportReview: false }] },
    { localApps: [{ id: 'calendar', name: '' }], datasets: [] },
    { localApps: [] },
  ];
  for (const scope of bad) await assert.rejects(execute('publishSharedBot', publication(botId, host, 2, scope)), { name: 'ZodError' }, JSON.stringify(scope).slice(0, 80));
  assert.equal((await botData()).identity.revision, 1);
  assert.deepEqual((await botData()).scope, emptyScope);
});

test('narrowing: a member taking part narrows what one Bot of the Chat may use, within what it published, as a whole value that can be lifted', options, async () => {
  await publishScope(published);
  assert.equal((await chatData()).botScopes, undefined, 'a Chat starts with no narrowing');
  assert.deepEqual(await narrow({ localApps: ['notes'], datasets: [{ source: 'orders', dataset: counted }] }, 0), { revision: 1 });
  assert.deepEqual((await chatData()).botScopes, { [botId]: { revision: 1, localApps: ['notes'], datasets: [{ source: 'orders', dataset: counted }] } });

  // Whoever wrote last wins only if they saw the last value.
  await assert.rejects(narrow({}, 0), { code: 'BOT_SCOPE_REVISION_CONFLICT' });
  // The Chat asks for no more than the Bot published, and a dataset is its source and its id together.
  await assert.rejects(narrow({ localApps: ['mail'] }, 1), { code: 'SCOPE_NOT_PUBLISHED' });
  await assert.rejects(narrow({ datasets: [{ source: 'elsewhere', dataset: sold }] }, 1), { code: 'SCOPE_NOT_PUBLISHED' });
  await assert.rejects(narrow({ datasets: [{ source: 'orders', dataset: unlisted }] }, 1), { code: 'SCOPE_NOT_PUBLISHED' });
  assert.equal((await chatData()).botScopes[botId].revision, 1, 'a refused narrowing changes nothing');
  // A kind that is left out does not narrow, a list that is empty allows none of it.
  assert.deepEqual(await narrow({ localApps: [] }, 1), { revision: 2 });
  assert.deepEqual((await chatData()).botScopes[botId], { revision: 2, localApps: [] });
  assert.deepEqual(await narrow({ localApps: [], datasets: [] }, 2), { revision: 3 });

  // Lifting is its own revision, so a caller that saw the narrowing cannot write it back over a later value.
  assert.deepEqual(await narrow(null, 3), { revision: 4 });
  assert.deepEqual((await chatData()).botScopes, { [botId]: { revision: 4 } });
  await assert.rejects(narrow({ localApps: ['notes'] }, 3), { code: 'BOT_SCOPE_REVISION_CONFLICT' });
  assert.deepEqual(await narrow({ localApps: ['calendar'] }, 4), { revision: 5 });
  assert.deepEqual((await chatData()).botScopes[botId], { revision: 5, localApps: ['calendar'] }, 'a later narrowing does not carry the lists of an earlier one');
});

test('narrowing: only a Bot of the Chat, only by a member taking part, and a command that was already carried out is answered, not applied twice', options, async () => {
  await publishScope(published);
  await execute('publishSharedBot', publication(receiverId, receiverHost, 1, published));
  await execute('heartbeatDeployment', { ...receiverHost, ready: true });
  // The second Bot is published but not in this Chat.
  await assert.rejects(narrow(null, 0, { sharedBotId: receiverId }), { code: 'BOT_PARTICIPANT_REQUIRED' });
  await assert.rejects(narrow(null, 0, { sharedBotId: 'sharedbot_99' }), { code: 'BOT_PARTICIPANT_REQUIRED' });
  // A member of the Team who is not in the Chat, and the Bot's maintainer, who reads the Chat but does not take part in it, may not.
  await assert.rejects(narrow(null, 0, { actor: other }), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(narrow(null, 0, { actor: owner }), { code: 'CONVERSATION_REQUIRED' });
  assert.equal((await chatData()).botScopes, undefined);

  const same = 'command_replay_01', first = await narrow({ localApps: ['notes'] }, 0, { commandId: same });
  assert.deepEqual(first, { revision: 1 });
  assert.deepEqual(await narrow({ localApps: ['notes'] }, 0, { commandId: same }), first, 'the lost answer is answered again');
  assert.equal((await chatData()).botScopes[botId].revision, 1);
  await assert.rejects(narrow({ localApps: ['calendar'] }, 0, { commandId: same }), { code: 'COMMAND_ID_CONFLICT' });
  // The scope of one Bot leaves the others of the Chat alone.
  await execute('createConversation', { conversationId: group, kind: 'group', title: 'Both', participantUids: [owner.uid, member.uid], sharedBotIds: [botId, receiverId] }, owner);
  assert.deepEqual(await narrow({ localApps: [] }, 0, { conversationId: group, sharedBotId: receiverId, actor: owner }), { revision: 1 });
  assert.deepEqual((await chatData(group)).botScopes, { [receiverId]: { revision: 1, localApps: [] } });
  assert.deepEqual(await narrow(null, 0, { conversationId: group, actor: owner }), { revision: 1 });
  assert.deepEqual((await chatData(group)).botScopes, { [receiverId]: { revision: 1, localApps: [] }, [botId]: { revision: 1 } });
});

test('narrowing: the Chat listing members read carries each Bot narrowing, and only members of the Chat read it', options, async () => {
  await publishScope(published);
  assert.equal((await messages.execute(member, { action: 'list', teamId, generation: 1 })).conversations[0].botScopes, undefined);
  await narrow({ localApps: ['calendar'], datasets: [] }, 0);
  const listed = (await messages.execute(member, { action: 'list', teamId, generation: 1 })).conversations;
  assert.deepEqual(listed.map(chat => [chat.id, chat.botScopes]), [[dm, { [botId]: { revision: 1, localApps: ['calendar'], datasets: [] } }]]);
  assert.deepEqual((await messages.execute(other, { action: 'list', teamId, generation: 1 })).conversations, []);
});

test('a run is given the scope its Bot published cut down by the Chat, and follows a narrowing made while it runs; a republication ends it', options, async () => {
  await publishScope(published);
  const { taskId, expectedLastChangeSeq } = await ask(dm, member);
  assert.deepEqual((await execute('prepareTask', { ...host, taskId })).scope, published);
  const started = await execute('startTask', { ...host, taskId, expectedLastChangeSeq });
  const validate = () => execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq });
  assert.deepEqual((await validate()).scope, published);

  await narrow({ localApps: ['notes'], datasets: [{ source: 'orders', dataset: counted }] }, 0);
  assert.deepEqual((await validate()).scope, { localApps: [{ id: 'notes', name: 'Notes' }], datasets: [row(counted)] }, 'the flags are the published ones');
  await narrow({ localApps: [], datasets: [] }, 1);
  assert.deepEqual((await validate()).scope, emptyScope);
  await narrow(null, 2);
  assert.deepEqual((await validate()).scope, published);

  // A run belongs to the publication it began with, so its scope is never swapped under it; the next run takes the new one.
  // A narrowing that named what the new publication no longer has leaves just what is still published.
  await narrow({ localApps: ['calendar', 'notes'] }, 3);
  const smaller = { localApps: [{ id: 'calendar', name: 'Calendar' }], datasets: [] };
  await publishScope(smaller);
  await assert.rejects(validate(), { code: 'TASK_REVOKED' });
  await execute('completeTask', { ...host, taskId, permitId: started.permitId, outcome: 'completed', text: 'Orders checked.', finishedAt: now,
    inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } });
  const next = await ask(dm, member), nextStarted = await execute('startTask', { ...host, taskId: next.taskId, expectedLastChangeSeq: next.expectedLastChangeSeq });
  assert.ok(nextStarted.execute);
  assert.deepEqual((await execute('validateTask', { ...host, taskId: next.taskId, permitId: nextStarted.permitId, expectedLastChangeSeq: next.expectedLastChangeSeq })).scope,
    smaller, 'a narrowing never adds: what is no longer published is just gone');
});

test('a Bot published without a scope has none in any Chat, whatever the Chat once narrowed it to', options, async () => {
  await publishScope(published);
  await narrow({ localApps: ['notes'] }, 0);
  await publishScope(undefined);
  const { taskId, expectedLastChangeSeq } = await ask(dm, member);
  const started = await execute('startTask', { ...host, taskId, expectedLastChangeSeq });
  assert.deepEqual((await execute('validateTask', { ...host, taskId, permitId: started.permitId, expectedLastChangeSeq })).scope, emptyScope);
  await assert.rejects(narrow({ localApps: ['notes'] }, 1), { code: 'SCOPE_NOT_PUBLISHED' }, 'nor can a Chat ask for what was never published');
});

// ---- LingXi Data: a live grant is necessary and no longer enough ----
const reviewless = 'dataset_04';
/** `published` plus a dataset the Bot may write but not open report reviews in. */
const dataScope = { ...published, datasets: [...published.datasets, row(reviewless, { write: true })] };
/**
 * A group Chat of the owner and a member with the Bot, a binding and a live edit grant (with report review) for four datasets, and one running task.
 * The Data service is a stub that records what reaches it. Only the Bot's scope decides which of the four it may use.
 */
async function dataFixture({ withoutScope = false, receiver = false } = {}) {
  await publishScope(withoutScope ? undefined : dataScope);
  if (receiver) await execute('publishSharedBot', publication(receiverId, receiverHost, 1));
  await execute('createConversation', { conversationId: group, kind: 'group', title: 'Orders', participantUids: [owner.uid, member.uid],
    sharedBotIds: receiver ? [botId, receiverId] : [botId] }, owner);
  const grant = (sharedBotId, botRevision, dataset, more = {}) => db.doc(`teams/${teamId}/sharedBots/${sharedBotId}/dataGrants/${dataset}`).set({ version: 1, access: 'edit',
    botRevision, principalUid: `lx_data_bot_${sharedBotId}_${dataset}`, requestDigest: `digest_${dataset}`, conversationId: group,
    issuedBy: { uid: owner.uid, generation: 1 }, reportReview: true, ...more });
  for (const dataset of [sold, counted, unlisted, reviewless]) {
    await db.doc(`lingxiDataBindings/${dataset}`).set({ kind: 'team', teamId, conversationId: group, audience: [{ uid: owner.uid, generation: 1, access: 'manage' }] });
    await grant(botId, (await botData()).identity.revision, dataset);
  }
  const sent = [];
  globalThis.fetch = async (_url, init) => {
    const { command: operation } = JSON.parse(init.body); sent.push(operation);
    return { ok: true, json: async () => operation.action === 'put'
      ? { dataset: operation.dataset, record: operation.record, revision: 1, schemaVersion: 1 } : { rows: [] } };
  };
  const records = createBotDataRecords({ db, endpoint: 'https://data.example', secret: 'secret', clock: () => now,
    projectActor: async (_uid, _target, recheck, send) => { await recheck(); return send(); }, botToken: async uid => `token_${uid}` });
  const { taskId, expectedLastChangeSeq } = await ask(group, owner);
  const started = await execute('startTask', { ...host, taskId, expectedLastChangeSeq });
  const proof = { ...host, teamId, generation: 1, taskId, permitId: started.permitId, expectedLastChangeSeq };
  const offers = () => records.offers(owner.uid, { action: 'botDataOffers', ...proof });
  const operate = (operation, stepId) => records.operate(owner.uid, { action: 'botDataOperation', ...proof, accessVersion: 1, operation, ...(stepId ? { stepId } : {}) });
  const put = (dataset, stepId = 'step_0001') => operate({ action: 'put', dataset, record: 'record_0001', expectedRevision: 0, schemaVersion: 1, value: { title: 'Order' } }, stepId);
  return { records, proof, sent, offers, operate, put, grant };
}
const denied = { code: 'DATA_ACCESS_REQUIRED', status: 403 };
const narrowGroup = (scope, expectedRevision, sharedBotId = botId) => narrow(scope, expectedRevision, { conversationId: group, sharedBotId, actor: owner });

test('data offers: a dataset is offered only if the Bot published it and the Chat leaves it, with the published flags', options, async () => {
  const { offers } = await dataFixture();
  // Four datasets have a live grant that allows editing and report review. The Bot published three: with both, read-only, and without review.
  assert.deepEqual(await offers(), { datasets: [{ dataset: sold, version: 1, write: true, reportReview: true }, { dataset: counted, version: 1, write: false },
    { dataset: reviewless, version: 1, write: true }] });

  await narrowGroup({ datasets: [{ source: 'orders', dataset: counted }, { source: 'orders', dataset: reviewless }] }, 0);
  assert.deepEqual(await offers(), { datasets: [{ dataset: counted, version: 1, write: false }, { dataset: reviewless, version: 1, write: true }] }, 'narrowing reaches the run that is going');
  await narrowGroup({ datasets: [] }, 1);
  assert.deepEqual(await offers(), { datasets: [] });
  await narrowGroup(null, 2);
  assert.equal((await offers()).datasets.length, 3);
});

test('data offers: a Bot published without a scope is offered nothing, grant or not', options, async () => {
  const { offers, operate, put, sent } = await dataFixture({ withoutScope: true });
  assert.deepEqual(await offers(), { datasets: [] });
  await assert.rejects(operate({ action: 'describe', dataset: sold }), denied);
  await assert.rejects(put(sold), denied);
  assert.deepEqual(sent, [], 'the Data service was never reached');
});

test('data operations: outside the published and narrowed datasets, or beyond the published flags, the Data service is never reached', options, async () => {
  const { operate, put, sent } = await dataFixture();
  assert.deepEqual(await operate({ action: 'describe', dataset: sold }), { rows: [] });
  assert.deepEqual(await operate({ action: 'describe', dataset: counted }), { rows: [] });
  assert.equal(sent.length, 2);
  // A grant for a dataset the Bot never published is not used, in any operation.
  await assert.rejects(operate({ action: 'describe', dataset: unlisted }), denied);
  await assert.rejects(operate({ action: 'list', dataset: unlisted, limit: 5 }), denied);
  await assert.rejects(operate({ action: 'get', reference: { dataset: unlisted, record: 'record_0001', revision: 1, schemaVersion: 1 } }), denied);
  await assert.rejects(put(unlisted), denied);
  // A write needs the published write flag, whatever the grant says.
  await assert.rejects(put(counted), denied);
  assert.equal(sent.length, 2);
  assert.deepEqual(await put(sold), { dataset: sold, record: 'record_0001', revision: 1, schemaVersion: 1 });
  assert.equal(sent.length, 3);
  // A report review needs its own published flag: these are refused before any report is read.
  const review = { action: 'openReportReview', release: 'a'.repeat(64), partition: 'p', group: 'g' };
  await assert.rejects(operate({ ...review, dataset: reviewless }, 'step_0002'), denied, 'published with write but without review');
  await assert.rejects(operate({ ...review, dataset: counted }, 'step_0003'), denied);
  await assert.rejects(operate({ ...review, dataset: unlisted }, 'step_0004'), denied);

  // And the Chat can take a dataset back while the run goes on.
  await narrowGroup({ datasets: [{ source: 'orders', dataset: counted }] }, 0);
  await assert.rejects(operate({ action: 'describe', dataset: sold }), denied);
  await assert.rejects(put(sold, 'step_0005'), denied);
  assert.deepEqual(await operate({ action: 'describe', dataset: counted }), { rows: [] });
  assert.equal(sent.length, 4);
});

test('data handoff: a record goes only between Bots that may read that dataset in this Chat', options, async () => {
  const { records, proof, grant, sent } = await dataFixture({ receiver: true });
  const reference = { dataset: sold, record: 'record_0001', revision: 1, schemaVersion: 1 };
  const send = () => records.handoff(owner.uid, { action: 'botDataHandoff', ...proof, accessVersion: 1, sharedBotId: receiverId, reference, stepId: 'handoff_01' });
  const handoffs = async () => (await db.collection(`teams/${teamId}/dataHandoffs`).get()).size;
  // The sender may hand `sold` to the second Bot. Each Bot has a grant bound to its own revision.
  await grant(botId, (await botData()).identity.revision, sold, { handoff: { targets: [receiverId], dailyLimit: 4 } });
  const regrant = async () => grant(receiverId, (await botData(receiverId)).identity.revision, sold, { access: 'read', reportReview: false });
  const publishReceiver = async scope => { await publishScope(scope, receiverId, receiverHost); await regrant(); };

  await regrant();
  await assert.rejects(send(), denied, 'the receiver published no scope');
  await publishReceiver({ localApps: [], datasets: [row(counted)] });
  await assert.rejects(send(), denied, 'it published another dataset');
  await publishReceiver({ localApps: [], datasets: [row(sold)] });
  await narrowGroup({ datasets: [] }, 0, receiverId);
  await assert.rejects(send(), denied, 'the Chat narrowed it to nothing');
  await narrowGroup(null, 1, receiverId);
  // The sender's own read is checked on the way: the Chat can take the dataset from the sender instead.
  await narrowGroup({ datasets: [{ source: 'orders', dataset: counted }] }, 0);
  await assert.rejects(send(), denied, 'the Chat narrowed the sender away from it');
  assert.equal(await handoffs(), 0); assert.deepEqual(sent, []);
  await narrowGroup(null, 1);
  const delivered = await send();
  assert.deepEqual(delivered.reference, reference);
  assert.equal((await db.doc(`teams/${teamId}/tasks/${delivered.taskId}`).get()).data().sharedBotId, receiverId);
  assert.equal(await handoffs(), 1);
  assert.equal(sent.length, 1, 'only the sender read the record through the Data service');

  // The receiver's run is about that dataset, so it stands only while the receiver may still use it in this Chat.
  const { lastChangeSeq } = await chatData(group), startReceiver = () => execute('startTask', { ...receiverHost, taskId: delivered.taskId, expectedLastChangeSeq: lastChangeSeq });
  await narrowGroup({ datasets: [] }, 2, receiverId);
  await assert.rejects(startReceiver(), { code: 'TASK_REVOKED' });
  await narrowGroup(null, 3, receiverId);
  assert.equal((await startReceiver()).execute, true);
});
