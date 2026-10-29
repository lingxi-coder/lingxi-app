import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { initializeTestEnvironment, assertFails, assertSucceeds } from '@firebase/rules-unit-testing';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore as adminFirestore } from 'firebase-admin/firestore';
import { collection, deleteDoc, doc, getDoc, getDocs, orderBy, query,
  runTransaction, serverTimestamp, setDoc, updateDoc, where, writeBatch } from 'firebase/firestore';
import { createTeamConversation } from '../src/team-conversations.mjs';

const projectId = 'demo-lingxi-teams-t0';
const emulator = process.env.FIRESTORE_EMULATOR_HOST;
const options = { skip: !emulator, timeout: 30_000 };
const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
const bot = { ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', status: 'online', use: 'listed', uids: ['alice'] };
let environment, adminApp, admin;
const user = uid => environment.authenticatedContext(uid).firestore();
const ref = (db, suffix) => doc(db, `teams/team-test/${suffix}`);
const conversation = (id = 'ordinary') => ({ teamId: 'team-test', conversationId: id, callerUid: 'alice', kind: 'dm', title: 'Fixture', participants: ['user:alice', 'user:bob'] });
const create = (input = conversation()) => createTeamConversation(admin, input);

before(async () => {
  if (!emulator) return;
  const [host, port] = emulator.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host), 'tests must use a local emulator');
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  adminApp = initializeApp({ projectId }, 'teams-rule-tests');
  admin = adminFirestore(adminApp);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore();
  const batch = admin.batch();
  batch.set(admin.doc('teams/team-test'), { name: 'Synthetic team', state: 'active' });
  for (const uid of ['alice', 'bob', 'owner', 'hidden']) batch.set(admin.doc(`teams/team-test/members/${uid}`), { role: 'member', generation: 1 });
  batch.set(admin.doc(`teams/team-test/bots/${botKey}`), bot);
  await batch.commit();
});
after(async () => {
  await environment?.cleanup();
  await admin?.terminate();
  if (adminApp) await deleteApp(adminApp);
});

function mutation(writer, db, current, { operation = 'create', mid = 'message01', changeId = 'change001', text = 'Synthetic message', author = { kind: 'user', uid: 'alice' }, changeSeq = current.lastChangeSeq + 1, seq = current.lastSeq + 1, cid = 'ordinary', textDigest = createHash('sha256').update(operation === 'recall' ? '' : text).digest('hex') } = {}, parts = ['conversation', 'message', 'change']) {
  const parent = `conversations/${cid}`;
  if (parts.includes('conversation')) writer.update(ref(db, parent), { lastChangeSeq: changeSeq, lastChange: { id: changeId, seq: changeSeq },
    ...(operation === 'create' ? { lastSeq: seq, lastMessage: { id: mid, seq, at: serverTimestamp() } } : {}) });
  if (parts.includes('message')) {
    const target = ref(db, `${parent}/messages/${mid}`);
    if (operation === 'create') writer.set(target, { seq, author, kind: 'text', text, createdAt: serverTimestamp(), changeSeq, changeId });
    else writer.update(target, { text: operation === 'recall' ? '' : text, editedAt: serverTimestamp(), changeSeq, changeId, ...(operation === 'recall' ? { recalled: true } : {}) });
  }
  if (parts.includes('change')) writer.set(ref(db, `${parent}/changes/${changeId}`), { seq: changeSeq, messageId: mid, operation, actorUid: author.uid, at: serverTimestamp(), textDigest });
}
async function send(db, args = {}, parts) {
  const current = (await getDoc(ref(db, `conversations/${args.cid ?? 'ordinary'}`))).data();
  const batch = writeBatch(db);
  mutation(batch, db, current, args, parts);
  return batch.commit();
}

test('teams: only default opts into Team rules', () => {
  const config = JSON.parse(readFileSync(new URL('../../../firebase.json', import.meta.url), 'utf8'));
  assert.equal(config.firestore.find(db => db.database === '(default)').rules, 'infra/firebase/firestore.default.rules');
  for (const database of ['model-catalog', 'push-notifications']) assert.equal(config.firestore.find(db => db.database === database).rules, 'infra/firebase/firestore.rules');
});
test('teams: clients cannot create a conversation or inject a hidden reader', options, async () => {
  const alice = user('alice');
  for (const data of [
    { kind: 'dm', participants: ['user:alice', 'user:bob'], memberUids: ['alice', 'bob', 'hidden'] },
    { kind: 'bot_dm', participants: ['user:alice', botKey], memberUids: ['alice'] },
  ]) await assertFails(setDoc(ref(alice, 'conversations/injected'), { ...data, title: '', lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null }));
  await create();
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary'), { memberUids: ['alice', 'bob', 'hidden'] }));
  await assertFails(getDoc(ref(user('hidden'), 'conversations/ordinary')));
});
test('teams: trusted creation derives exact ACL, includes Bot owner and checks allowlist', options, async () => {
  const dm = await create();
  assert.deepEqual(dm.memberUids, ['alice', 'bob']);
  const botDm = await create({ ...conversation('bot-chat'), kind: 'bot_dm', participants: ['user:alice', botKey] });
  assert.deepEqual(botDm.memberUids, ['alice', 'owner']);
  assert.equal(botDm.botKey, botKey);
  await assertSucceeds(getDoc(ref(user('owner'), 'conversations/bot-chat')));
  await assert.rejects(create({ ...conversation('bad-chat'), callerUid: 'bob', kind: 'bot_dm', participants: ['user:bob', botKey] }), { status: 403 });
  await assert.rejects(create({ ...conversation(), memberUids: ['hidden'] }), { status: 400 });
  await assert.rejects(create({ ...conversation('invalid1'), participants: ['user:alice', 'user:alice'] }), { status: 400 });
  await assert.rejects(create({ ...conversation('invalid2'), participants: ['user:alice', 'user:missing'] }), { status: 403 });
});
test('teams: trusted creation retries are idempotent, identity changes conflict', options, async () => {
  const [a, b] = await Promise.all([create(), create()]);
  assert.deepEqual(a, b);
  await assert.rejects(create({ ...conversation(), title: 'Different request' }), { status: 409 });
  await admin.doc('teams/team-test/members/alice').delete();
  await assert.rejects(create(), { status: 403 });
});
test('teams: unauthenticated, nonmember, removed member and deleted team cannot access history', options, async () => {
  await create();
  const alice = user('alice');
  await send(alice);
  for (const db of [environment.unauthenticatedContext().firestore(), user('outsider')]) await assertFails(getDoc(ref(db, 'conversations/ordinary/messages/message01')));
  await admin.doc('teams/team-test/members/alice').delete();
  await assertFails(getDoc(ref(alice, 'conversations/ordinary/messages/message01')));
  await assertFails(send(user('bob'), { author: { kind: 'user', uid: 'alice' }, mid: 'message02', changeId: 'change002' }));
  await admin.doc('teams/team-test').delete();
  await assertFails(getDoc(ref(user('bob'), 'conversations/ordinary/messages/message01')));
});
test('teams: one real client transaction commits all three documents', options, async () => {
  await create();
  const alice = user('alice');
  await assertSucceeds(runTransaction(alice, async transaction => {
    const current = await transaction.get(ref(alice, 'conversations/ordinary'));
    mutation(transaction, alice, current.data());
  }));
  assert.equal((await getDoc(ref(alice, 'conversations/ordinary'))).data().lastSeq, 1);
  assert.equal((await getDoc(ref(alice, 'conversations/ordinary/changes/change001'))).data().seq, 1);
});
test('teams: no orphan cursor, message or change receipt, skipped seq or duplicate ID', options, async () => {
  await create();
  const alice = user('alice');
  for (const parts of [['conversation'], ['message'], ['change'], ['conversation', 'message'], ['conversation', 'change'], ['message', 'change']]) await assertFails(send(alice, {}, parts));
  await assertFails(send(alice, { seq: 2 }));
  await assertFails(send(alice, { changeSeq: 2 }));
  await assertFails(send(alice, { textDigest: '0'.repeat(64) }));
  await send(alice);
  await assertFails(send(alice, { mid: 'message01', changeId: 'change002' }));
  await assertFails(send(alice, { mid: 'message02', changeId: 'change001' }));
  assert.equal((await getDoc(ref(alice, 'conversations/ordinary'))).data().lastSeq, 1);
});
test('teams: Bot authorship cannot be forged or used outside the conversation', options, async () => {
  await create();
  await assertFails(send(user('alice'), { author: { kind: 'bot', uid: 'alice', botKey } }));
  await create({ ...conversation('bot-chat'), kind: 'bot_dm', participants: ['user:alice', botKey] });
  await assertSucceeds(send(user('owner'), { cid: 'bot-chat', author: { kind: 'bot', uid: 'owner', botKey } }));
});
test('teams: changing Bot allowlist denies existing private-thread writes immediately', options, async () => {
  await create({ ...conversation('bot-chat'), kind: 'bot_dm', participants: ['user:alice', botKey] });
  const alice = user('alice');
  await assertSucceeds(send(alice, { cid: 'bot-chat' }));
  await assertSucceeds(updateDoc(ref(user('owner'), `bots/${botKey}`), { uids: [] }));
  await assertFails(send(alice, { cid: 'bot-chat', mid: 'message02', changeId: 'change002' }));
  await assertSucceeds(send(user('owner'), { cid: 'bot-chat', mid: 'message02', changeId: 'change002', author: { kind: 'bot', uid: 'owner', botKey } }));
});
test('teams: offline catch-up includes edits and permanent recalls without adding unread', options, async () => {
  await create();
  const alice = user('alice'), bob = user('bob');
  await send(alice);
  await send(alice, { operation: 'edit', text: 'Edited', changeId: 'change002' });
  await send(alice, { operation: 'recall', changeId: 'change003' });
  const changes = await getDocs(query(collection(bob, 'teams/team-test/conversations/ordinary/changes'), where('seq', '>', 1), orderBy('seq')));
  assert.deepEqual(changes.docs.map(change => [change.data().seq, change.data().operation]), [[2, 'edit'], [3, 'recall']]);
  const message = (await getDoc(ref(bob, 'conversations/ordinary/messages/message01'))).data();
  assert.equal(message.text, ''); assert.equal(message.recalled, true); assert.equal(message.changeSeq, 3);
  const current = (await getDoc(ref(bob, 'conversations/ordinary'))).data();
  assert.equal(current.lastSeq, 1); assert.equal(current.lastChangeSeq, 3);
  await assertFails(send(alice, { operation: 'edit', text: 'Revived', changeId: 'change004' }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/messages/message01'), { recalled: false }));
});
test('teams: another author cannot edit/recall and change receipts are immutable', options, async () => {
  await create();
  const alice = user('alice'), bob = user('bob');
  await send(alice);
  await assertFails(send(bob, { operation: 'edit', author: { kind: 'user', uid: 'bob' }, changeId: 'change002' }));
  await assertFails(send(bob, { operation: 'recall', author: { kind: 'user', uid: 'bob' }, changeId: 'change002' }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/changes/change001'), { operation: 'recall' }));
  await assertFails(deleteDoc(ref(alice, 'conversations/ordinary/changes/change001')));
});
test('teams: self-only server-time heartbeat cannot change membership or generation', options, async () => {
  const alice = user('alice');
  await assertSucceeds(updateDoc(ref(alice, 'members/alice'), { lastActiveAt: serverTimestamp() }));
  await assertFails(updateDoc(ref(user('bob'), 'members/alice'), { lastActiveAt: serverTimestamp() }));
  await assertFails(updateDoc(ref(alice, 'members/alice'), { role: 'owner', lastActiveAt: serverTimestamp() }));
  await assertFails(updateDoc(ref(alice, 'members/alice'), { generation: 2, lastActiveAt: serverTimestamp() }));
  await assertFails(updateDoc(ref(alice, 'members/alice'), { lastActiveAt: new Date(0) }));
});
test('teams: text limit is UTF-8 bytes and read watermarks never move backwards', options, async () => {
  await create();
  const alice = user('alice');
  await assertFails(send(alice, { text: '中'.repeat(2731) }));
  await assertSucceeds(send(alice, { text: '中'.repeat(2730) }));
  await assertSucceeds(setDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 1 }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 0 }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 2 }));
  await assertFails(setDoc(ref(user('bob'), 'conversations/ordinary/reads/alice'), { lastReadSeq: 1 }));
});
test('teams: unimplemented App, task, invite and epoch writes remain denied', options, async () => {
  const alice = user('alice');
  for (const path of ['appGrants/example_alice', 'appOpens/example01', 'tasks/example01', 'invites/example01', 'membershipEpochs/alice']) {
    await assertFails(setDoc(ref(alice, path), { uid: 'alice' }));
    await assertFails(getDoc(ref(alice, path)));
  }
});
test('teams: both configured non-Team rule files compile and deny client access', options, async () => {
  const config = JSON.parse(readFileSync(new URL('../../../firebase.json', import.meta.url), 'utf8'));
  const [host, port] = emulator.split(':');
  for (const name of ['model-catalog', 'push-notifications']) {
    const rules = config.firestore.find(database => database.database === name).rules;
    // Emulator named DBs default to open and cannot be configured by firebase-tools.
    // Exercise each mapped rule file in a separate default test database instead.
    const isolated = await initializeTestEnvironment({ projectId: `demo-teams-deny-${name}`, firestore: { host, port: Number(port),
      rules: readFileSync(new URL(`../../../${rules}`, import.meta.url), 'utf8') } });
    try {
      const db = isolated.authenticatedContext('alice').firestore();
      await assertFails(setDoc(doc(db, 'teams/team-test'), { name: 'Not permitted' }));
      await assertFails(getDoc(doc(db, 'teams/team-test')));
    } finally { await isolated.cleanup(); }
  }
});
