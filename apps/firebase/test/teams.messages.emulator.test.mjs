import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails } from '@firebase/rules-unit-testing';
import { doc, getDoc, setDoc } from 'firebase/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { createTeamVisualizations } from '../src/team-visualizations.mjs';
import { reservationMs } from '../src/team-visualization-storage.mjs';
import { maxPackageBytes } from '../src/team-visualization-contract.mjs';
import { markupPackage, putFixtureParts } from './visualization-fixture.mjs';
import { InboxStore } from '../src/inbox-store.mjs';
import { onTeamMessageCreated } from '../src/team-message-push.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0', teamId = 'messages-team', conversationId = 'messages-chat';
const owner = { uid: 'owner', name: 'Owner' }, alice = { uid: 'alice', name: 'Alice' }, hidden = { uid: 'hidden', name: 'Hidden' };
let app, db, environment, teams, messages, now;
const scope = { teamId, generation: 1, conversationId };
const command = (change = {}) => ({ action: 'mutate', ...scope, messageId: 'message01', changeId: 'change001', operation: 'create', text: 'Synthetic message', ...change });
const execute = (value, actor = owner) => messages.execute(actor, value);
const sync = (after = 0, actor = alice, override = {}) => execute({ action: 'sync', ...scope, after, ...override }, actor);
const conversationRef = () => db.doc(`teams/${teamId}/conversations/${conversationId}`);
const pointer = async () => (await conversationRef().get()).data().lastMessage;
const stored = async id => (await conversationRef().collection('messages').doc(id).get()).data();
const points = value => Array.from(value).length;
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':'); assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port), rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'teams-message-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore(); now = Date.now(); teams = new TeamsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Synthetic message team' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite001', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const user of [alice, hidden]) await teams.execute(user, { action: 'join', code, requestId: `join-${user.uid}` });
  await execute({ action: 'create', ...scope, kind: 'dm', title: 'Owner and Alice', participantUids: ['owner', 'alice'] });
});
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('channel visualization publishes once with all grants, isolates viewer state and revokes on recall', options, async () => {
  const channel = { ...scope, conversationId: 'visualization-chat' };
  const destination = { team: teamId, conversation: channel.conversationId };
  await execute({ action: 'create', ...channel, kind: 'channel', title: 'Orders', participantUids: [owner.uid, alice.uid] });
  const path = `teams/${teamId}/conversations/${channel.conversationId}`;
  const blobs = new Map();
  let blockUpload = false;
  let onUpload;
  const bucket = { file(key) { return {
    async save(bytes, args) {
      assert.equal(args.preconditionOpts.ifGenerationMatch, 0);
      if (blockUpload) throw new Error('Temporary storage outage');
      if (blobs.has(key)) throw Object.assign(new Error('Exists'), { code: 412 });
      blobs.set(key, Buffer.from(bytes));
      await onUpload?.();
    },
    async download() { return [blobs.get(key)]; },
    async delete() { blobs.delete(key); },
  }; } };
  const orders = db.doc('datasetV2/orders');
  await orders.set({ owner: { kind: 'user', id: owner.uid }, deleted: false });
  let missing = true;
  const core = async (_token, data) => {
    const { command, dataset } = data;
    if (command.action === 'visualization') return { alias: command.alias, operation: data.operation };
    assert.equal(command.action, 'prepareShare');
    const { share } = command;
    const grant = `grant_${share.visualization.binding}`;
    const reference = share.reference ?? { kind: 'dataset', dataset };
    if (!missing || share.mode !== 'pinned') {
      await db.doc(`datasetV2/${dataset}/grants/${grant}`).set({
        status: 'pending', destination, audience: { alice: 1, owner: 1 }, issuer: owner.uid,
        mode: share.mode, reference, message: share.visualization.message,
        visualization: share.visualization, textDigest: share.textDigest, parent: data.via ?? null,
      });
    }
    return { grant, reference, messageId: share.visualization.message };
  };
  const service = createTeamVisualizations({ db, bucket, core, channels: `${teamId}/${channel.conversationId}`, clock: () => now });
  const usage = async () => (await db.doc(`teams/${teamId}/privateSettings/visualizations`).get()).data().bytes;
  const call = (value, actor = owner) => service.execute(actor, 'test-token', { ...channel, ...value });
  const sample = markupPackage('Sales chart', '<p>Orders</p>');
  const publish = async input => {
    const plan = await call({ ...input, stage: 'prepare' });
    if (plan.status !== 'uploading') return plan;
    await putFixtureParts(plan, sample.chunks, call);
    return call({ ...input, stage: 'commit' });
  };
  await assert.rejects(call({ action: 'audience' }, hidden), { code: 'DATA_ACCESS_REQUIRED' });
  const { audienceDigest } = await call({ action: 'audience' });
  const publication = { action: 'publish', operation: 'share_orders', audienceDigest,
    package: sample.package,
    bindings: [
      { kind: 'share', dataset: 'orders', alias: 'live', mode: 'edit' },
      { kind: 'share', dataset: 'orders', alias: 'pinned', mode: 'pinned',
        reference: { kind: 'snapshot', dataset: 'orders', snapshot: 'month-end' } },
    ],
  };
  await assert.rejects(publish(publication), { code: 'DATA_SHARE_CHANGED' });
  assert.equal((await db.collection(path + '/messages').get()).size, 0);
  assert.equal(blobs.size, 0);
  missing = false;
  const [delivered, duplicate] = await Promise.all([publish(publication), publish(publication)]);
  assert.deepEqual(duplicate, delivered);
  assert.deepEqual(await publish(publication), delivered);
  assert.equal((await db.collection(path + '/messages').get()).size, 1);
  assert.ok(await usage() > 0);
  assert.equal(blobs.size, 1);
  const { messageId: message, reference: { version } } = delivered;
  const stored = (await db.doc(`${path}/messages/${message}`).get()).data();
  assert.equal(stored.kind, 'visualization');
  assert.equal(stored.visualization.bindings.length, 2);
  assert.equal((await db.doc(`teamVisualizations/${message}/versions/${version}`).get()).data().status, 'active');
  const locator = { message, version };
  const verify = cards => call({ action: 'checkBatch', cards }, alice);
  assert.deepEqual(await verify([locator, { ...locator, message: 'viz_missing' }, locator]), { active: [true, false, true] });
  const opened = await call({ ...locator, action: 'open' }, alice);
  assert.deepEqual(opened.package, sample.package);
  assert.deepEqual(Buffer.from((await call({ ...locator, action: 'readPart', index: 0 }, alice)).data, 'base64'), sample.chunks[0]);
  const state = { modelContent: { scale: 2 }, privateContent: [[1, 2], [3, 4]] };
  await call({ ...locator, action: 'state', expected: 0, state }, alice);
  assert.deepEqual((await call({ ...locator, action: 'open' }, alice)).state, { version: 1, ...state });
  assert.equal((await call({ ...locator, action: 'open' })).state.version, 0);
  await assert.rejects(call({ ...locator, action: 'state', expected: 0, state }, alice), { code: 'DATA_CONFLICT' });
  await assert.rejects(call({ ...locator, action: 'open' }, hidden), { code: 'DATA_ACCESS_REQUIRED' });
  await orders.update({ deleted: true });
  await assert.rejects(publish({ ...publication, operation: 'deleted-data', bindings: [
    { kind: 'reuse', ...stored.visualization.bindings[0] },
  ] }), { code: 'DATA_SHARE_CHANGED' });
  await orders.update({ deleted: false });
  await db.doc(path).update({ memberUids: [owner.uid, alice.uid, hidden.uid] });
  assert.notEqual((await call({ action: 'audience' })).audienceDigest, audienceDigest);
  assert.deepEqual(await publish(publication), delivered);
  assert.equal((await db.collection(path + '/messages').get()).size, 1);
  await execute(command({ ...channel, messageId: message, changeId: 'recall-orders', operation: 'recall', text: '' }));
  await assert.rejects(call({ ...locator, action: 'open' }, alice), { code: 'DATA_ACCESS_REQUIRED' });
  await assert.rejects(call({ ...locator, action: 'readPart', index: 0 }, alice), { code: 'DATA_ACCESS_REQUIRED' });
  await assert.rejects(call({ ...locator, action: 'data', alias: 'live', request: { action: 'read' } }, alice), { code: 'DATA_ACCESS_REQUIRED' });
  assert.deepEqual(await verify([locator]), { active: [false] });
  await service.sweep();
  await service.sweep();
  assert.equal((await db.doc(`teamVisualizations/${message}/versions/${version}`).collection('viewerState').get()).size, 0);
  assert.equal(blobs.size, 0);
  assert.equal(await usage(), 0);
  assert.deepEqual(await publish(publication), delivered);

  await teams.execute(hidden, { action: 'leave', teamId, generation: 1 });
  assert.deepEqual((await call({ action: 'audience' })).audience, { alice: 1, owner: 1 });
  const pending = { ...publication, bindings: [], operation: 'abandoned-upload',
    audienceDigest: (await call({ action: 'audience' })).audienceDigest };
  blockUpload = true;
  await assert.rejects(publish(pending), /Temporary storage outage/);
  assert.ok(await usage() > 0);
  now += reservationMs;
  await service.sweep();
  assert.equal(await usage(), 0);
  blockUpload = false;
  await assert.rejects(publish(pending), { code: 'DATA_SHARE_CHANGED' });

  const sharedOrders = db.doc('datasetV2/shared-orders');
  const parent = sharedOrders.collection('grants').doc('parent-orders');
  const parentReference = { kind: 'dataset', dataset: sharedOrders.id };
  await sharedOrders.set({ owner: { kind: 'user', id: alice.uid }, deleted: false });
  await parent.set({ status: 'active', audience: { alice: 1, owner: 1 }, issuer: alice.uid,
    mode: 'live', reference: parentReference, destination, reshare: true, message: 'received-orders' });
  await db.doc(path + '/messages/received-orders').set({ author: { kind: 'user', uid: alice.uid },
    resourceGrant: parent.id, resourceRef: parentReference });
  onUpload = () => parent.update({ status: 'revoked' });
  await assert.rejects(publish({ ...pending, operation: 'ancestor-revoked', bindings: [
    { kind: 'share', dataset: sharedOrders.id, via: parent.id, mode: 'live', alias: 'orders' },
  ] }), { code: 'DATA_SHARE_CHANGED' });
  onUpload = undefined;
  assert.equal(blobs.size, 1, 'authorization changed after the reservation and object upload');
  assert.equal((await db.collection(path + '/messages').get()).docs.filter(doc => doc.data().visualization).length, 0);
  now += reservationMs;
  await service.sweep();
  assert.equal(blobs.size, 0);

  const final = await publish({ ...pending, operation: 'fresh-upload' });
  assert.equal(final.status, 'published');
  const finalCard = { message: final.messageId, version: final.reference.version };
  await call({ ...finalCard, action: 'state', expected: 0, state }, alice);
  await teams.execute(owner, { action: 'delete', teamId });
  await service.sweep();
  await service.sweep();
  assert.equal(blobs.size, 0);
  assert.equal((await db.doc(`teamVisualizations/${finalCard.message}/versions/${finalCard.version}`).collection('viewerState').get()).size, 0);
});

test('channel visualization settings default to 1 MiB and permit a complete 50 MiB package only in the configured channel', options, async () => {
  const channel = { ...scope, conversationId: 'large-visualization' };
  const other = { ...scope, conversationId: 'small-visualization' };
  for (const target of [channel, other]) await execute({ ...target, action: 'create', kind: 'channel',
    title: target.conversationId, participantUids: [owner.uid, alice.uid] });
  const files = new Map();
  const bucket = { file(key) { return {
    async save(bytes) { files.set(key, bytes); },
    async download() { return [files.get(key)]; },
    async delete() { files.delete(key); },
  }; } };
  const visualizations = createTeamVisualizations({ db, bucket, channels: '*', clock: () => now });
  const handler = createTeamsHandler({ store: teams, visualizations, verifyToken: async token => token === alice.uid ? alice : owner });
  const http = (body, actor = owner, target = channel) => handler(new Request('https://fixture.test/v1/teams/visualizations', {
    method: 'POST', headers: { authorization: 'Bearer ' + actor.uid }, body: JSON.stringify({ ...target, ...body }),
  }));
  async function call(body, actor, target) {
    const response = await http(body, actor, target);
    const result = await response.json();
    assert.equal(response.status, 200, JSON.stringify(result));
    return result;
  }
  assert.deepEqual(await call({ action: 'settings' }), { limitMiB: 1, revision: 0, canEdit: true });
  assert.deepEqual(await call({ action: 'settings' }, alice), { limitMiB: 1, revision: 0, canEdit: false });
  assert.equal((await http({ action: 'setLimit', limitMiB: 50, expectedRevision: 0 }, alice)).status, 403);
  assert.equal((await http({ action: 'setLimit', limitMiB: 51, expectedRevision: 0 })).status, 400);
  const markup = Buffer.alloc(maxPackageBytes, 0x20);
  markup.write('<p>Orders at the limit</p>');
  const content = markupPackage('Large visualization', markup);
  const { audienceDigest } = await call({ action: 'audience' });
  const send = { action: 'publish', stage: 'prepare', operation: 'fifty-mib', audienceDigest, package: content.package };
  assert.equal((await http(send)).status, 413);
  assert.deepEqual(await call({ action: 'setLimit', limitMiB: 50, expectedRevision: 0 }), { limitMiB: 50, revision: 1, canEdit: true });
  assert.deepEqual(await call({ action: 'settings' }, owner, other), { limitMiB: 1, revision: 0, canEdit: true });
  assert.equal((await http({ ...send, package: { ...content.package, htmlBytes: maxPackageBytes + 1 } })).status, 400);
  const plan = await call(send);
  assert.equal(plan.missing.length, 25);
  assert.equal((await http({ ...send, stage: 'commit' })).status, 409);
  await putFixtureParts({ ...plan, missing: [0] }, content.chunks, call);
  const resumed = await call(send);
  assert.deepEqual(resumed.missing, plan.missing.slice(1));
  await putFixtureParts(resumed, content.chunks, call);
  assert.deepEqual((await call(send)).missing, []);
  const published = await call({ ...send, stage: 'commit' });
  const card = { message: published.messageId, version: published.reference.version };
  assert.deepEqual((await call({ action: 'open', ...card }, alice)).package, content.package);
  for (const index of plan.missing) {
    const part = await call({ action: 'readPart', ...card, index }, alice);
    assert.deepEqual(Buffer.from(part.data, 'base64'), content.chunks[index]);
  }
  assert.equal(files.size, 25);
  assert.equal((await http({ action: 'setLimit', limitMiB: 5, expectedRevision: 0 })).status, 409);
  const pending = { ...send, operation: 'pending-before-limit-change' };
  await call(pending);
  await call({ action: 'setLimit', limitMiB: 1, expectedRevision: 1 });
  assert.equal((await http({ ...pending, stage: 'commit' })).status, 413);
  assert.equal((await call({ action: 'open', ...card }, alice)).package.htmlBytes, maxPackageBytes);
});

test('T2: concurrent duplicate sends have one identity, seq and immutable receipt', options, async () => {
  const values = await Promise.all(Array.from({ length: 8 }, () => execute(command())));
  assert.ok(values.every(value => value.changeSeq === 1));
  const page = await sync(); assert.equal(page.conversation.lastSeq, 1); assert.equal(page.conversation.lastChangeSeq, 1); assert.equal(page.messages.length, 1); assert.equal(page.changes.length, 1);
  assert.equal(page.changes[0].generation, 1);
  await assert.rejects(execute(command({ text: 'Different payload' })), { code: 'MESSAGE_ID_CONFLICT' });
  await assert.rejects(execute(command({ changeId: 'other-change' })), { code: 'MESSAGE_ID_CONFLICT' });
});
test('T2: retry evidence survives later editing and recall without retaining recalled text', options, async () => {
  await execute(command());
  await execute(command({ operation: 'edit', changeId: 'change002', text: 'Edited synthetic body' }));
  assert.equal((await execute(command())).changeSeq, 1);
  await execute(command({ operation: 'recall', changeId: 'change003', text: '' }));
  assert.equal((await execute(command())).changeSeq, 1);
  const page = await sync(); assert.equal(page.messages[0].text, ''); assert.equal(page.messages[0].recalled, true); assert.equal(page.conversation.lastSeq, 1); assert.equal(page.conversation.lastChangeSeq, 3);
  assert.ok(page.changes.every(change => !('text' in change)));
  await assert.rejects(execute(command({ operation: 'edit', changeId: 'change004', text: 'Revival' })), { code: 'MESSAGE_RECALLED' });
  await assert.rejects(execute(command({ operation: 'recall', changeId: 'change003', text: 'Not blank' }))); // Strict recall schema.
});
test('group mentions preserve a spaced name, route attention by ID and keep send receipts stable', options, async () => {
  const groupId = 'mentions-chat', key = `team:${teamId}:${groupId}`;
  const group = { ...scope, conversationId: groupId };
  const prefixed = { uid: 'user:alice', name: 'Other Alice' };
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'mention-invite', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(prefixed, { action: 'join', code, requestId: 'mention-join' });
  await execute({ action: 'create', ...group, kind: 'group', title: 'Mentions', participantUids: ['owner', 'alice', prefixed.uid] });
  const mention = { userId: 'alice', offset: 3, length: '@Mary Jane'.length };
  const send = command({ ...group, text: '👋 @Mary Jane, could you review?', mentionEntities: [mention] });
  await assert.rejects(execute({ ...send, mentionEntities: [{ ...mention, offset: 2 }] }));
  await assert.rejects(execute({ ...send, mentionEntities: [{ ...mention, userId: 'hidden' }] }), { code: 'MENTION_TARGET_REQUIRED' });
  const first = await execute(send);
  assert.deepEqual(await execute(send), first);
  await assert.rejects(execute({ ...send, mentionEntities: [] }), { code: 'MESSAGE_ID_CONFLICT' });
  const read = async () => (await sync(0, alice, { conversationId: groupId })).messages[0];
  assert.deepEqual((await read()).mentionEntities, [mention]);
  assert.deepEqual((await read()).mentions, ['user:alice']);
  assert.deepEqual((await read()).mentionRecipientUids, ['alice']);
  const inbox = new InboxStore(db, () => now);
  await inbox.fold(alice.uid, { kind: 'team', sourceId: teamId, conversationId: groupId });
  await inbox.fold(prefixed.uid, { kind: 'team', sourceId: teamId, conversationId: groupId });
  assert.equal((await inbox.execute(alice.uid, { action: 'get', key })).item.mentionCount, 1);
  assert.equal((await inbox.execute(prefixed.uid, { action: 'get', key })).item.mentionCount, 0);
  assert.deepEqual((await db.collection('users/alice/inboxNotices').get()).docs.map(doc => doc.data().kind), ['mention']);
  assert.equal((await db.collection(`users/${prefixed.uid}/inboxNotices`).get()).size, 0);
  const countEvent = db.doc(`teams/${teamId}/conversations/${groupId}/inboxCountEvents/${send.messageId}`);
  assert.deepEqual((await countEvent.get()).data().mentions, ['alice']);

  await execute({ ...send, operation: 'edit', changeId: 'edit-mention', mentionEntities: [] });
  assert.deepEqual((await read()).mentionEntities, []);
  assert.deepEqual((await read()).mentionRecipientUids, ['alice']);

  await teams.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await assert.rejects(execute({ ...send, messageId: 'message02', changeId: 'change002' }), { code: 'MENTION_TARGET_REQUIRED' });
  assert.deepEqual(await execute(send), first, 'receipt stays idempotent after removal');
  await execute(command({ ...group, messageId: 'message03', changeId: 'change003' }));
});

test('legacy and current edits preserve historical mentions, but cannot add departed targets', options, async () => {
  const group = { ...scope, conversationId: 'history-mentions' };
  await execute({ action: 'create', ...group, kind: 'group', title: 'Mention history', participantUids: [owner.uid, alice.uid, hidden.uid] });
  const mention = { userId: alice.uid, offset: 0, length: 10 };
  const send = command({ ...group, text: '@Mary Jane, old wording', mentionEntities: [mention] });
  await execute(send);
  const get = async () => (await sync(0, owner, group)).messages[0];
  const legacyEdit = { ...command(group), operation: 'edit', text: '👋 ' + send.text, changeId: 'edit-legacy' };
  await execute(legacyEdit);
  const shifted = { ...mention, offset: 3 };
  assert.deepEqual((await get()).mentionEntities, [shifted]);
  await teams.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await teams.execute(owner, { action: 'remove', teamId, uid: hidden.uid, generation: 1 });
  await execute({ ...legacyEdit, text: '👋 @Mary Jane, revised wording', changeId: 'edit-current', mentionEntities: [shifted] });
  assert.deepEqual((await get()).mentionEntities, [shifted]);
  await assert.rejects(execute({ ...legacyEdit, changeId: 'edit-new-target', mentionEntities: [{ ...shifted, userId: hidden.uid }] }), { code: 'MENTION_TARGET_REQUIRED' });
  await execute({ ...legacyEdit, changeId: 'edit-clear', mentionEntities: [] });
  assert.deepEqual((await get()).mentionEntities, []);
  assert.deepEqual((await get()).mentionRecipientUids, [alice.uid]);
  await assert.rejects(execute({ ...legacyEdit, changeId: 'edit-reintroduce', mentionEntities: [shifted] }), { code: 'MENTION_TARGET_REQUIRED' });
});

test('T2: mutation catch-up pages edits/recalls older than the reconnect cursor in order', options, async () => {
  for (let i = 1; i <= 105; i++) await execute(command({ messageId: `message-${String(i).padStart(3, '0')}`, changeId: `create-${String(i).padStart(3, '0')}`, text: `Synthetic ${i}` }));
  const before = await sync(); assert.equal(before.changes.length, 100); assert.equal(before.hasMore, true);
  now += 600000; // Ten-minute logical offline period, distinct from wall-clock browser acceptance.
  await execute(command({ messageId: 'message-001', changeId: 'edit-old-001', operation: 'edit', text: 'Edited during offline' }));
  await execute(command({ messageId: 'message-002', changeId: 'recall-old-002', operation: 'recall', text: '' }));
  const after = await sync(100);
  assert.deepEqual(after.changes.map(value => value.seq), [101, 102, 103, 104, 105, 106, 107]);
  assert.equal(after.messages.find(value => value.id === 'message-001').text, 'Edited during offline');
  assert.equal(after.messages.find(value => value.id === 'message-002').text, '');
  assert.equal(after.conversation.lastSeq, 105); assert.equal(after.hasMore, false);
  assert.equal((await sync(107)).changes.length, 0);
});
test('T2: read watermarks are monotonic and mutation replay never increases unread', options, async () => {
  for (let i = 1; i <= 3; i++) await execute(command({ messageId: `message0${i}`, changeId: `change00${i}` }));
  await execute({ action: 'read', ...scope, lastReadSeq: 2 }, alice);
  await execute({ action: 'read', ...scope, lastReadSeq: 1 }, alice);
  await execute(command({ messageId: 'message01', changeId: 'edit-old-001', operation: 'edit', text: 'new' }));
  await execute(command({ messageId: 'message02', changeId: 'recall-old-002', operation: 'recall', text: '' }));
  const [c] = (await execute({ action: 'list', teamId, generation: 1 }, alice)).conversations;
  assert.equal(c.lastSeq - c.lastReadSeq, 1);
  await assert.rejects(execute({ action: 'read', ...scope, lastReadSeq: 4 }, alice), { code: 'INVALID_READ_CURSOR' });
});
test('T2: every operation is denied after removal and old generation cannot replay after rejoin', options, async () => {
  const oldSend = command({ messageId: 'alice-msg', changeId: 'alice-change' }); await execute(oldSend, alice);
  await execute({ action: 'read', ...scope, lastReadSeq: 1 }, alice);
  await teams.execute(owner, { action: 'remove', teamId, uid: alice.uid, generation: 1 });
  await assert.rejects(sync(), { code: 'MEMBERSHIP_REQUIRED' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite002', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(alice, { action: 'join', code, requestId: 'alice-join2' });
  for (const value of [oldSend, { action: 'read', ...scope, lastReadSeq: 1 }, { action: 'sync', ...scope, after: 1 }, { action: 'list', teamId, generation: 1 }]) await assert.rejects(execute(value, alice), { code: 'STALE_MEMBERSHIP' });
  await assert.rejects(execute({ action: 'create', ...scope, conversationId: 'old-create01', kind: 'dm', title: 'Stale', participantUids: ['owner', 'alice'] }, alice), { status: 409 });
  const fresh = await sync(0, alice, { generation: 2 }); assert.equal(fresh.conversation.lastReadSeq, 0);
  await assert.rejects(execute({ ...oldSend, generation: 2 }, alice), { code: 'MESSAGE_ID_CONFLICT' });
});
test('T2: ACLs, author identity, reply target and payload boundaries are enforced', options, async () => {
  assert.equal((await execute({ action: 'list', teamId, generation: 1 }, hidden)).conversations.length, 0);
  await assert.rejects(sync(0, hidden), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute(command(), hidden), { code: 'CONVERSATION_REQUIRED' });
  await execute(command());
  await assert.rejects(execute(command({ operation: 'edit', changeId: 'alice-change', text: 'Impersonated' }), alice), { code: 'MESSAGE_AUTHOR_REQUIRED' });
  await assert.rejects(execute(command({ messageId: 'message02', changeId: 'change002', replyTo: 'missing-reply' })), { code: 'REPLY_REQUIRED' });
  await execute(command({ messageId: 'message02', changeId: 'change002', text: '中'.repeat(2730), replyTo: 'message01' }));
  await assert.rejects(execute(command({ messageId: 'message03', changeId: 'change003', text: '中'.repeat(2731) })));
  await assert.rejects(sync(999), { code: 'INVALID_CHANGE_CURSOR' });
  const client = environment.authenticatedContext(alice.uid).firestore();
  await assertFails(setDoc(doc(client, `teams/${teamId}/conversations/${conversationId}/reads/alice`), { generation: 99, lastReadSeq: 1 }));
});
test('T2: HTTP messages route authenticates, rejects old constructor and ignores caller ACL injection', options, async () => {
  const handler = createTeamsHandler({ store: teams, verifyToken: async token => { if (token !== 'synthetic-owner') throw new Error(); return owner; }, createCustomToken: async () => '' });
  const request = (path, body, token = 'synthetic-owner') => handler(new Request(`https://fixture.local${path}`, { method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: JSON.stringify(body) }));
  assert.equal((await request('/v1/teams/messages', command(), 'wrong')).status, 401);
  assert.equal((await request('/v1/teams/messages', command({ memberUids: ['hidden'] }))).status, 400);
  assert.equal((await request('/v1/teams', { action: 'conversation', teamId, conversationId, kind: 'dm', title: 'legacy', participants: ['user:owner', 'user:alice'] })).status, 400);
  assert.equal((await request('/v1/teams/messages', command())).status, 200);
});
test('T2: push excludes sender, deduplicates attempts and carries no plaintext message', options, async () => {
  await execute(command());
  const original = (await db.doc(`teams/${teamId}/conversations/${conversationId}/messages/message01`).get()).data();
  const sent = [], expiry = { toMillis: () => now + 3600000 };
  const device = { id: 'synthetic-device', uid: 'alice', revision: 1, fcmInstallationId: 'synthetic-fid', expiresAt: expiry, locale: 'zh-CN', appId: 'com.lingxi.code', platform: 'android' };
  const pushStore = { async active(uid) { return uid === 'alice' ? [device] : []; }, device() { return { async get() { return { data: () => device }; } }; } };
  const input = { db, pushStore, messaging: { async send(value) { sent.push(value); } }, teamId, conversationId, messageId: 'message01', original, clock: () => now };
  await Promise.all([onTeamMessageCreated(input), onTeamMessageCreated(input)]);
  assert.equal(sent.length, 1); assert.equal(sent[0].data.recipientUid, 'alice'); assert.equal(sent[0].data.route, 'team/inbox'); assert.equal(sent[0].data.body, '团队有新消息');
  assert.ok(!JSON.stringify(sent).includes('Synthetic message'));
  await execute(command({ messageId: 'message02', changeId: 'change002' }));
  const latest = (await db.doc(`teams/${teamId}/conversations/${conversationId}/messages/message02`).get()).data();
  await teams.execute(owner, { action: 'remove', teamId, uid: 'alice', generation: 1 });
  await onTeamMessageCreated({ ...input, messageId: 'message02', original: latest }); assert.equal(sent.length, 1);
});
test('lastMessage: creation records the message identity, its author and a one-line preview', options, async () => {
  assert.equal(await pointer(), null); // a new conversation has no message yet
  await execute(command({ text: '  First line\n\n\tsecond\u00a0line \u3000 end  ' }));
  const last = await pointer(), message = await stored('message01');
  assert.deepEqual(Object.keys(last).sort(), ['at', 'author', 'id', 'preview', 'seq']);
  assert.equal(last.id, 'message01'); assert.equal(last.seq, 1); assert.equal(last.preview, 'First line second line end');
  assert.deepEqual(last.author, { kind: 'user', uid: owner.uid }); assert.deepEqual(last.author, message.author);
  assert.equal(last.at.toMillis(), now); assert.equal(last.at.toMillis(), message.createdAt.toMillis());
  assert.equal(message.text, '  First line\n\n\tsecond\u00a0line \u3000 end  '); // the message keeps its original text
  await execute(command({ messageId: 'message02', changeId: 'change002', text: ' \n\t ' }), alice); // whitespace only
  const blank = await pointer();
  assert.equal(blank.preview, ''); assert.equal(blank.id, 'message02'); assert.deepEqual(blank.author, { kind: 'user', uid: alice.uid }); assert.ok(!('recalled' in blank));
  assert.equal((await sync()).conversation.lastSeq, 2);
});
test('lastMessage: long, multi-line and emoji text is cut at 120 code points without splitting a pair', options, async () => {
  await execute(command({ text: `Heading\n\n${'详细说明 '.repeat(100)}` }));
  assert.equal((await pointer()).preview, `Heading ${'详细说明 '.repeat(100)}`.slice(0, 120).trimEnd());
  await execute(command({ messageId: 'message02', changeId: 'change002', text: `${'😀'.repeat(60)}\n${'🙂'.repeat(80)}` }));
  const emoji = (await pointer()).preview;
  assert.equal(emoji, `${'😀'.repeat(60)} ${'🙂'.repeat(59)}`); assert.equal(points(emoji), 120); assert.ok(emoji.isWellFormed());
  await execute(command({ messageId: 'message03', changeId: 'change003', text: `${'😀'.repeat(119)}\n🙂🙂 tail` })); // the cut would end on a space
  assert.equal((await pointer()).preview, '😀'.repeat(119));
  await execute(command({ messageId: 'message04', changeId: 'change004', text: '中'.repeat(2730) })); // 8190 bytes, the largest accepted body
  assert.equal((await pointer()).preview, '中'.repeat(120));
  assert.equal((await stored('message04')).text.length, 2730);
});
test('lastMessage: editing or recalling the newest message refreshes its preview, older messages leave it alone', options, async () => {
  await execute(command()); await execute(command({ messageId: 'message02', changeId: 'change002', text: 'Newest body' }));
  const before = await pointer(); assert.equal(before.id, 'message02');
  await execute(command({ operation: 'edit', changeId: 'change003', text: 'Edited older body' })); // message01
  assert.deepEqual(await pointer(), before);
  await execute(command({ operation: 'recall', changeId: 'change004', text: '' })); // message01
  assert.deepEqual(await pointer(), before);
  await execute(command({ messageId: 'message02', operation: 'edit', changeId: 'change005', text: 'Edited\nnewest   body' }));
  const edited = await pointer();
  assert.deepEqual({ ...edited, preview: before.preview }, before); // only the preview moved
  assert.equal(edited.preview, 'Edited newest body'); assert.ok(!('recalled' in edited));
  assert.equal((await execute(command({ messageId: 'message02', changeId: 'change002', text: 'Newest body' }))).changeSeq, 2); // a retried create does not undo the edit
  assert.deepEqual(await pointer(), edited);
  await execute(command({ messageId: 'message02', operation: 'recall', changeId: 'change006', text: '' }));
  const recalled = await pointer();
  assert.deepEqual({ ...recalled, preview: edited.preview, recalled: undefined }, { ...edited, recalled: undefined });
  assert.equal(recalled.preview, ''); assert.equal(recalled.recalled, true);
  assert.ok(!JSON.stringify(await conversationRef().get().then(snapshot => snapshot.data())).includes('Edited newest'));
  const page = await sync(); assert.equal(page.conversation.lastSeq, 2); assert.equal(page.conversation.lastChangeSeq, 6);
  await execute(command({ messageId: 'message03', changeId: 'change007', text: 'After the recall' })); // a later message replaces the recalled pointer
  const next = await pointer();
  assert.equal(next.id, 'message03'); assert.equal(next.seq, 3); assert.equal(next.preview, 'After the recall'); assert.ok(!('recalled' in next));
});
test('lastMessage: edits of the newest message keep the sender and are truncated like a new message', options, async () => {
  await execute(command({ messageId: 'message02', changeId: 'change002', text: 'Alice speaks' }), alice);
  await execute(command({ messageId: 'message02', operation: 'edit', changeId: 'change003', text: `${'x'.repeat(200)} y` }), alice);
  const last = await pointer();
  assert.equal(last.preview, 'x'.repeat(120)); assert.deepEqual(last.author, { kind: 'user', uid: alice.uid }); assert.equal(last.id, 'message02');
  await assert.rejects(execute(command({ messageId: 'message02', operation: 'edit', changeId: 'change004', text: 'Hijacked' })), { code: 'MESSAGE_AUTHOR_REQUIRED' });
  assert.equal((await pointer()).preview, 'x'.repeat(120)); // a rejected edit leaves the pointer alone
});

test('channel management delegates without giving a team owner message access and redeems one current invite', options, async () => {
  const room = { ...scope, conversationId: 'morning-room' };
  const call = (op, args = {}, actor = owner) => execute({ action: 'channelSettings', teamId, generation: 1,
    request: { op, ...(op === 'list' ? {} : { conversationId: room.conversationId }), ...args } }, actor);
  await execute({ action: 'create', ...room, kind: 'channel', title: 'Morning', participantUids: [alice.uid, hidden.uid] }, alice);
  await assert.rejects(execute({ action: 'history', ...room }), { code: 'CONVERSATION_REQUIRED' });
  assert.deepEqual((await call('list')).channels, [{ id: room.conversationId, title: 'Morning' }]);
  await call('admin', { uid: alice.uid, enabled: true });
  await assert.rejects(call('admin', { uid: hidden.uid, enabled: true }, alice), { code: 'CHANNEL_ADMIN_REQUIRED' });
  await call('profile', { title: 'Morning Meeting', description: 'Agenda and notes' }, alice);
  await call('remove', { uid: hidden.uid }, alice);
  await assert.rejects(execute({ action: 'history', ...room }, hidden), { code: 'CONVERSATION_REQUIRED' });
  const [first, second] = await Promise.all([call('createInvite', {}, alice), call('createInvite', {}, alice)]);
  assert.equal(first.code, second.code);
  await assert.rejects(call('redeem', first, { uid: 'outsider' }), { code: 'CHANNEL_TEAM_REQUIRED' });
  await Promise.all([call('redeem', first, hidden), call('redeem', first, hidden)]);
  assert.deepEqual((await db.doc(`teams/${teamId}/conversations/${room.conversationId}`).get()).data().participants, ['user:alice', 'user:hidden']);
  await call('revokeInvite', first, alice);
  await assert.rejects(call('redeem', second, hidden), { code: 'CHANNEL_INVITE_INVALID' });
  assert.deepEqual((await execute({ action: 'history', ...room }, hidden)).messages, []);
  await call('admin', { uid: alice.uid, enabled: false });
  await assert.rejects(call('posting', { posting: { mode: 'chat', senders: 'all', selected: [], replies: 'all' } }, alice), { code: 'CHANNEL_ADMIN_REQUIRED' });
  await assert.rejects(execute({ action: 'history', ...room }), { code: 'CONVERSATION_REQUIRED' });
});

test('posts keep history and enforce separate sender and reply rules at commit', options, async () => {
  const room = { ...scope, conversationId: 'posting-room' };
  await execute({ action: 'create', ...room, kind: 'channel', title: 'Channel', participantUids: [owner.uid, alice.uid] });
  const config = (mode, senders, replies) => execute({ action: 'channelSettings', teamId, generation: 1, request: {
    op: 'posting', conversationId: room.conversationId, posting: { mode, senders, selected: senders === 'selected' ? ['user:owner'] : [], replies }
  } });
  const send = (id, actor, channelMode, replyTo) => execute(command({ ...room, messageId: `message-${id}`, changeId: `change-${id}`, channelMode, replyTo }), actor);
  await send('old', alice, 'chat');
  await send('quote', owner, 'chat', 'message-old');
  await config('posts', 'selected', 'all');
  await assert.rejects(send('stale', owner, 'chat'), { code: 'CHANNEL_MODE_CHANGED' });
  await assert.rejects(send('denied', alice, 'posts'), { code: 'CHANNEL_POSTING_DENIED' });
  await send('post', owner, 'posts');
  await send('reply', alice, 'posts', 'message-post');
  await send('nested', alice, 'posts', 'message-reply');
  await config('posts', 'selected', 'authors');
  await assert.rejects(send('restricted', alice, 'posts', 'message-post'), { code: 'CHANNEL_POSTING_DENIED' });
  await config('posts', 'all', 'none');
  await assert.rejects(send('closed', owner, 'posts', 'message-post'), { code: 'CHANNEL_POSTING_DENIED' });
  await config('chat', 'all', 'all');
  await assert.rejects(send('archived', owner, 'chat', 'message-post'), { code: 'CHANNEL_MODE_CHANGED' });
  await send('new', alice, 'chat');
  const { messages: history } = await execute({ action: 'history', ...room });
  const post = await execute({ action: 'postHistory', ...room, postId: 'message-post' }, alice);
  assert.equal(post.root.id, 'message-post');
  assert.deepEqual(post.replies.map(item => item.id), ['message-reply', 'message-nested']);
  assert.equal(post.next, null);
  await assert.rejects(execute({ action: 'postHistory', ...room, postId: 'message-post' }, hidden), { code: 'CONVERSATION_REQUIRED' });
  assert.deepEqual(history.map(m => [m.id, m.channelPost ?? false, m.postId ?? null, m.replyTo ?? null]), [
    ['message-old', false, null, null], ['message-quote', false, null, 'message-old'], ['message-post', true, null, null],
    ['message-reply', false, 'message-post', 'message-post'], ['message-nested', false, 'message-post', 'message-reply'], ['message-new', false, null, null],
  ]);
});

test('bot dependency blocks removal until resolved and Bot identities use the same posting policy', options, async () => {
  const room = { ...scope, conversationId: 'bot-guard-room' };
  await execute({ action: 'create', ...room, kind: 'channel', title: 'Channel', participantUids: [owner.uid, alice.uid] });
  const botId = 'minutes-bot', principal = `shared_bot:${botId}`;
  const ref = db.doc(`teams/${teamId}/conversations/${room.conversationId}`);
  await ref.update({ participants: ['user:alice', 'user:owner', principal] });
  await db.doc(`teams/${teamId}/sharedBots/${botId}`).set({ name: 'Minutes Bot', identity: { maintainer: { uid: alice.uid, generation: 1 } } });
  const call = (request, actor = owner) => execute({ action: 'channelSettings', teamId, generation: 1, request: { ...request, conversationId: room.conversationId } }, actor);
  const { mayPost } = await import('../src/channel-settings.mjs');
  await call({ op: 'posting', posting: { mode: 'posts', senders: 'selected', selected: [principal], replies: 'none' } });
  const c = (await ref.get()).data();
  assert.equal(mayPost(c, principal), true);
  assert.equal(mayPost(c, principal, true), false);
  assert.equal(mayPost(c, 'user:alice'), false);
  assert.deepEqual(await call({ op: 'remove', uid: alice.uid }), { blockedBy: [{ id: botId, name: 'Minutes Bot' }] });
  assert.ok((await ref.get()).data().memberUids.includes(alice.uid));
  await call({ op: 'detachBot', botId }, alice);
  await call({ op: 'remove', uid: alice.uid });
  assert.ok(!(await ref.get()).data().memberUids.includes(alice.uid));
});

test('post likes are reversible and views unique across retries, members and older posts', options, async () => {
  const room = { ...scope, conversationId: 'post-metrics' }, postId = 'original-post';
  await execute({ action: 'create', ...room, kind: 'channel', title: 'Updates', participantUids: [owner.uid, alice.uid] });
  await execute({ action: 'channelSettings', teamId, generation: room.generation, request: { op: 'posting',
    conversationId: room.conversationId, posting: { mode: 'posts', senders: 'selected', selected: ['user:owner'], replies: 'all' } } });
  const post = fields => command({ ...room, channelMode: 'posts', ...fields });
  await execute(post({ messageId: postId, changeId: 'publish-post' }));
  const interact = (actor, liked) => execute({ action: 'postInteraction', ...room, postId, ...(liked !== undefined ? { liked } : {}) }, actor);
  await Promise.all([interact(alice, true), interact(alice, true), interact(owner)]);
  assert.deepEqual(await interact(alice), { liked: true, stats: { replies: 0, likes: 1, views: 2, revision: 2 } });
  assert.deepEqual(await interact(alice, false), { liked: false, stats: { replies: 0, likes: 0, views: 2, revision: 3 } });
  await assert.rejects(interact(hidden, true), { code: 'CONVERSATION_REQUIRED' });
  const client = environment.authenticatedContext(alice.uid).firestore();
  const path = `teams/${teamId}/conversations/${room.conversationId}/messages/${postId}`;
  assert.equal((await getDoc(doc(client, path + '/postViewers/' + alice.uid))).data().liked, false);
  await assertFails(getDoc(doc(client, path + '/postViewers/' + owner.uid)));
  await assertFails(setDoc(doc(client, path + '/postViewers/' + alice.uid), { liked: true }));
  await assertFails(setDoc(doc(client, path), { postStats: { likes: 99 } }, { merge: true }));
  await execute(post({ messageId: 'reply-one', changeId: 'reply-one-change', replyTo: postId }), alice);
  await execute(post({ messageId: 'reply-two', changeId: 'reply-two-change', replyTo: 'reply-one' }), alice);
  await execute(post({ messageId: 'reply-one', changeId: 'recall-reply', operation: 'recall', text: '' }), alice);
  assert.equal((await interact(owner)).stats.replies, 1);
  const root = db.doc(`teams/${teamId}/conversations/${room.conversationId}/messages/${postId}`);
  const { postStats, ...legacy } = (await root.get()).data();
  await root.set(legacy);
  await root.collection('postViewers').doc(alice.uid).delete();
  await root.collection('postViewers').doc(owner.uid).delete();
  assert.deepEqual((await interact(alice)).stats, { replies: 1, likes: 0, views: 1, revision: 1 });
  await execute(post({ messageId: postId, changeId: 'recall-post', operation: 'recall', text: '' }));
  await assert.rejects(interact(alice, true), { code: 'CHANNEL_POST_REQUIRED' });
});
