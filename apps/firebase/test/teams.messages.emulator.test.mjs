import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails } from '@firebase/rules-unit-testing';
import { doc, setDoc } from 'firebase/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { onTeamMessageCreated } from '../src/team-message-push.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0', teamId = 'messages-team', conversationId = 'messages-chat';
const owner = { uid: 'owner', name: 'Owner' }, alice = { uid: 'alice', name: 'Alice' }, hidden = { uid: 'hidden', name: 'Hidden' };
let app, db, environment, teams, messages, now;
const scope = { teamId, generation: 1, conversationId };
const command = (change = {}) => ({ action: 'mutate', ...scope, messageId: 'message01', changeId: 'change001', operation: 'create', text: 'Synthetic message', ...change });
const execute = (value, actor = owner) => messages.execute(actor, value);
const sync = (after = 0, actor = alice, override = {}) => execute({ action: 'sync', ...scope, after, ...override }, actor);
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
