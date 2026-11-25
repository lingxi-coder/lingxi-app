import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails } from '@firebase/rules-unit-testing';
import { doc, setDoc } from 'firebase/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { createTeamVisualizations } from '../src/team-visualizations.mjs';
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
  const bucket = { file(key) { return {
    async save(bytes, args) {
      assert.equal(args.preconditionOpts.ifGenerationMatch, 0);
      if (blobs.has(key)) throw Object.assign(new Error('Exists'), { code: 412 });
      blobs.set(key, Buffer.from(bytes));
    },
    async download() { return [blobs.get(key)]; },
  }; } };
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
        visualization: share.visualization, textDigest: share.textDigest,
      });
    }
    return { grant, reference, messageId: share.visualization.message };
  };
  const service = createTeamVisualizations({ db, bucket, core, channels: `${teamId}/${channel.conversationId}` });
  const call = (value, actor = owner) => service.execute(actor, 'test-token', { ...channel, ...value });
  await assert.rejects(call({ action: 'audience' }, hidden), { code: 'DATA_ACCESS_REQUIRED' });
  const { audienceDigest } = await call({ action: 'audience' });
  const publication = { action: 'publish', operation: 'share_orders', audienceDigest,
    bundle: { title: 'Sales chart', html: '<p>Orders</p>' },
    bindings: [
      { kind: 'share', dataset: 'orders', alias: 'live', mode: 'edit' },
      { kind: 'share', dataset: 'orders', alias: 'pinned', mode: 'pinned',
        reference: { kind: 'snapshot', dataset: 'orders', snapshot: 'month-end' } },
    ],
  };
  await assert.rejects(call(publication), { code: 'DATA_SHARE_CHANGED' });
  assert.equal((await db.collection(path + '/messages').get()).size, 0);
  assert.equal(blobs.size, 0);
  missing = false;
  const delivered = await call(publication);
  assert.deepEqual(await call(publication), delivered);
  assert.equal(blobs.size, 1);
  const { messageId: message, reference: { version } } = delivered;
  const stored = (await db.doc(`${path}/messages/${message}`).get()).data();
  assert.equal(stored.kind, 'visualization');
  assert.equal(stored.visualization.bindings.length, 2);
  assert.equal((await db.doc(`datasetPages/${message}/versions/${version}`).get()).data().status, 'active');
  const locator = { message, version };
  const opened = await call({ ...locator, action: 'open' }, alice);
  assert.equal(opened.bundle.html, publication.bundle.html);
  const state = { modelContent: 'filter', privateContent: 42 };
  await call({ ...locator, action: 'state', expected: 0, state }, alice);
  assert.deepEqual((await call({ ...locator, action: 'open' }, alice)).state, { version: 1, ...state });
  assert.equal((await call({ ...locator, action: 'open' })).state.version, 0);
  await assert.rejects(call({ ...locator, action: 'state', expected: 0, state }, alice), { code: 'DATA_CONFLICT' });
  await assert.rejects(call({ ...locator, action: 'open' }, hidden), { code: 'DATA_ACCESS_REQUIRED' });
  await execute(command({ ...channel, messageId: message, changeId: 'recall-orders', operation: 'recall', text: '' }));
  await assert.rejects(call({ ...locator, action: 'open' }, alice), { code: 'DATA_ACCESS_REQUIRED' });
  await assert.rejects(call({ ...locator, action: 'data', alias: 'live', request: { action: 'read' } }, alice), { code: 'DATA_ACCESS_REQUIRED' });
  assert.deepEqual(await call(publication), delivered);
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
