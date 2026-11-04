import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { initializeTestEnvironment, assertFails, assertSucceeds } from '@firebase/rules-unit-testing';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore as adminFirestore } from 'firebase-admin/firestore';
import { collection, deleteDoc, deleteField, doc, getDoc, getDocs, orderBy, query,
  runTransaction, serverTimestamp, setDoc, updateDoc, where, writeBatch } from 'firebase/firestore';
import { createTeamConversation } from '../src/team-conversations.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { messagePreview } from '../src/team-message-preview.mjs';

const projectId = 'demo-lingxi-teams-t0';
const emulator = process.env.FIRESTORE_EMULATOR_HOST;
const options = { skip: !emulator, timeout: 30_000 };
const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
const bot = { ownerUid: 'owner', ownerGeneration: 1, hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', revision: 1, enabled: true, use: 'listed', uids: ['alice'] };
let environment, adminApp, admin;
const user = uid => environment.authenticatedContext(uid).firestore();
const ref = (db, suffix) => doc(db, `teams/team-test/${suffix}`);
const conversation = (id = 'ordinary') => ({ teamId: 'team-test', conversationId: id, callerUid: 'alice', callerGeneration: 1, kind: 'dm', title: 'Fixture', participants: ['user:alice', 'user:bob'] });
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

const defined = value => Object.fromEntries(Object.entries(value).filter(([, field]) => field !== undefined));
// `last` overrides lastMessage fields on create (undefined drops a field). `pointer` overrides the dotted lastMessage
// updates of an edit/recall: by default the newest message gets the refreshed preview / the recall blanking; null writes none.
function mutation(writer, db, current, { operation = 'create', mid = 'message01', changeId = 'change001', text = 'Synthetic message', author = { kind: 'user', uid: 'alice' }, changeSeq = current.lastChangeSeq + 1, seq = current.lastSeq + 1, cid = 'ordinary', generation = 1, textDigest = createHash('sha256').update(operation === 'recall' ? '' : text).digest('hex'),
  last = {}, pointer } = {}, parts = ['conversation', 'message', 'change']) {
  const parent = `conversations/${cid}`;
  const newest = current.lastMessage?.id === mid;
  const kept = pointer !== undefined ? pointer ?? {} : !newest ? {} : operation === 'recall'
    ? { 'lastMessage.preview': '', 'lastMessage.recalled': true } : { 'lastMessage.preview': messagePreview(text) };
  if (parts.includes('conversation')) writer.update(ref(db, parent), { lastChangeSeq: changeSeq, lastChange: { id: changeId, seq: changeSeq },
    ...(operation === 'create' ? { lastSeq: seq, lastMessage: defined({ id: mid, seq, at: serverTimestamp(), author, preview: messagePreview(text), ...last }) } : kept) });
  if (parts.includes('message')) {
    const target = ref(db, `${parent}/messages/${mid}`);
    if (operation === 'create') writer.set(target, { seq, author, kind: 'text', text, createdAt: serverTimestamp(), changeSeq, changeId });
    else writer.update(target, { text: operation === 'recall' ? '' : text, editedAt: serverTimestamp(), changeSeq, changeId, ...(operation === 'recall' ? { recalled: true } : {}) });
  }
  if (parts.includes('change')) writer.set(ref(db, `${parent}/changes/${changeId}`), { seq: changeSeq, messageId: mid, operation, actorUid: author.uid, generation, at: serverTimestamp(), textDigest });
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
  assert.equal(config.firestore.find(db => db.database === '(default)').indexes, 'infra/firebase/firestore.teams.indexes.json');
  assert.equal(config.firestore.find(db => db.database === 'model-catalog').indexes, 'infra/firebase/firestore.indexes.json');
  const indexes = JSON.parse(readFileSync(new URL('../../../infra/firebase/firestore.teams.indexes.json', import.meta.url), 'utf8'));
  assert.ok(indexes.indexes.some(index => index.collectionGroup === 'triggers' && index.queryScope === 'COLLECTION'
    && JSON.stringify(index.fields) === JSON.stringify([{ fieldPath: 'botKey', order: 'ASCENDING' }, { fieldPath: 'seq', order: 'ASCENDING' }])));
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
  await assertFails(updateDoc(ref(user('owner'), `bots/${botKey}`), { uids: [] }));
  await admin.doc(`teams/team-test/bots/${botKey}`).update({ uids: [], revision: 2 });
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
  await assertSucceeds(setDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 1, generation: 1 }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 0 }));
  await assertFails(updateDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 2 }));
  await assertFails(setDoc(ref(user('bob'), 'conversations/ordinary/reads/alice'), { lastReadSeq: 1, generation: 1 }));
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

test('teams: SDK message receipts and reads must match the current membership generation', options, async () => {
  await create(); const alice = user('alice');
  await admin.doc('teams/team-test/members/alice').update({ generation: 2 });
  await assertFails(send(alice));
  await assertSucceeds(send(alice, { generation: 2 }));
  await assertFails(setDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 1, generation: 1 }));
  await assertSucceeds(setDoc(ref(alice, 'conversations/ordinary/reads/alice'), { lastReadSeq: 1, generation: 2 }));
});

const lastOf = async (db, cid = 'ordinary') => (await getDoc(ref(db, `conversations/${cid}`))).data().lastMessage;
test('teams: lastMessage mirrors the created message with its author and a bounded preview', options, async () => {
  await create();
  const alice = user('alice');
  for (const last of [
    { preview: undefined }, { author: undefined }, { preview: 'x'.repeat(121) }, { preview: 7 }, { preview: null },
    { author: { kind: 'user', uid: 'bob' } }, { author: { kind: 'bot', uid: 'alice', botKey } }, { author: { kind: 'user', uid: 'alice', botKey } },
    { author: 'alice' }, { recalled: true }, { unexpected: 'field' },
  ]) await assertFails(send(alice, { last }));
  await assertSucceeds(send(alice, { text: 'Line one\n\n   line two' }));
  const stored = await lastOf(alice);
  assert.equal(stored.preview, 'Line one line two'); assert.deepEqual(stored.author, { kind: 'user', uid: 'alice' }); assert.ok(!('recalled' in stored));
});
test('teams: a Bot-authored lastMessage must carry the same Bot identity as its message', options, async () => {
  await create({ ...conversation('bot-chat'), kind: 'bot_dm', participants: ['user:alice', botKey] });
  const owner = user('owner'), author = { kind: 'bot', uid: 'owner', botKey };
  await assertFails(send(owner, { cid: 'bot-chat', author, last: { author: { kind: 'bot', uid: 'owner' } } })); // identity dropped
  await assertFails(send(owner, { cid: 'bot-chat', author, last: { author: { kind: 'user', uid: 'owner' } } })); // posing as the person
  await assertFails(send(owner, { cid: 'bot-chat', author, last: { author: { kind: 'bot', uid: 'owner', botKey: `bot:owner:${'x'.repeat(22)}:${'b'.repeat(22)}` } } }));
  await assertSucceeds(send(owner, { cid: 'bot-chat', author, text: 'Bot says hello' }));
  const stored = await lastOf(owner, 'bot-chat');
  assert.deepEqual(stored.author, author); assert.equal(stored.preview, 'Bot says hello');
});
test('teams: the preview limit is 120 code points, matching the backend cut', options, async () => {
  await create();
  const alice = user('alice');
  // string.size() counts UTF-16 units, so the rule uses an RE2 repetition: a preview of 120 emoji is valid, 121 is not.
  for (const preview of ['中'.repeat(121), '😀'.repeat(121), `${'a'.repeat(120)}😀`, 'a'.repeat(300), `${'a'.repeat(60)}\n${'😀'.repeat(61)}`]) await assertFails(send(alice, { last: { preview } }));
  await assertSucceeds(send(alice, { text: '中'.repeat(120) })); // 360 UTF-8 bytes
  await assertSucceeds(send(alice, { mid: 'message02', changeId: 'change002', text: '😀'.repeat(130) })); // 120 code points, 240 UTF-16 units
  assert.equal(Array.from((await lastOf(alice)).preview).length, 120);
  await assertSucceeds(send(alice, { mid: 'message03', changeId: 'change003', text: `${'a'.repeat(119)}😀😀` })); // 120 code points, 121 units
  await assertSucceeds(send(alice, { mid: 'message04', changeId: 'change004', text: ' \n\t ' })); // whitespace-only text previews as ''
  assert.equal((await lastOf(alice)).preview, '');
});
test('teams: editing the newest message may only refresh its preview and older messages never move the pointer', options, async () => {
  await create();
  const alice = user('alice');
  await send(alice); await send(alice, { mid: 'message02', changeId: 'change002', text: 'Newest body' });
  await assertFails(send(alice, { operation: 'edit', mid: 'message01', changeId: 'change003', text: 'Edit old', pointer: { 'lastMessage.preview': 'Edit old' } }));
  await assertFails(send(alice, { operation: 'edit', mid: 'message01', changeId: 'change003', text: 'Edit old', pointer: { 'lastMessage.preview': '' } }));
  await assertSucceeds(send(alice, { operation: 'edit', mid: 'message01', changeId: 'change003', text: 'Edit old' })); // the pointer stays on message02
  assert.equal((await lastOf(alice)).id, 'message02'); assert.equal((await lastOf(alice)).preview, 'Newest body');
  const edit = (pointer, changeId = 'change004') => send(alice, { operation: 'edit', mid: 'message02', changeId, text: 'Edited newest', pointer });
  for (const pointer of [
    { 'lastMessage.preview': 'x'.repeat(121) }, { 'lastMessage.preview': 5 }, { 'lastMessage.preview': deleteField() },
    { 'lastMessage.preview': 'ok', 'lastMessage.seq': 9 }, { 'lastMessage.preview': 'ok', 'lastMessage.id': 'message01' },
    { 'lastMessage.preview': 'ok', 'lastMessage.at': new Date(0) }, { 'lastMessage.preview': 'ok', 'lastMessage.recalled': true },
    { 'lastMessage.preview': 'ok', 'lastMessage.author': { kind: 'user', uid: 'bob' } }, { 'lastMessage.preview': 'ok', 'lastMessage.extra': 1 },
  ]) await assertFails(edit(pointer));
  await assertSucceeds(edit(undefined, 'change004'));
  assert.equal((await lastOf(alice)).preview, 'Edited newest');
  await assertSucceeds(edit(null, 'change005')); // a stale excerpt cannot be proven wrong, so leaving it is accepted
  assert.equal((await lastOf(alice)).preview, 'Edited newest');
  await assertSucceeds(edit({ 'lastMessage.preview': 'x'.repeat(120) }, 'change006'));
});
test('teams: recalling the newest message must blank its preview and mark the pointer recalled', options, async () => {
  await create();
  const alice = user('alice');
  await send(alice); await send(alice, { mid: 'message02', changeId: 'change002', text: 'Secret detail' });
  await assertFails(send(alice, { operation: 'recall', mid: 'message01', changeId: 'change003', pointer: { 'lastMessage.preview': '', 'lastMessage.recalled': true } }));
  await assertSucceeds(send(alice, { operation: 'recall', mid: 'message01', changeId: 'change003' })); // older: pointer untouched
  assert.equal((await lastOf(alice)).preview, 'Secret detail');
  const recall = pointer => send(alice, { operation: 'recall', mid: 'message02', changeId: 'change004', pointer });
  for (const pointer of [
    null, { 'lastMessage.recalled': true }, { 'lastMessage.preview': '' }, { 'lastMessage.preview': 'Secret detail', 'lastMessage.recalled': true },
    { 'lastMessage.preview': '', 'lastMessage.recalled': false }, { 'lastMessage.preview': '', 'lastMessage.recalled': 'yes' },
    { 'lastMessage.preview': '', 'lastMessage.recalled': true, 'lastMessage.seq': 1 }, { 'lastMessage.preview': '', 'lastMessage.recalled': true, 'lastMessage.extra': 1 },
  ]) await assertFails(recall(pointer));
  await assertSucceeds(recall(undefined));
  const last = await lastOf(alice);
  assert.equal(last.preview, ''); assert.equal(last.recalled, true); assert.equal(last.id, 'message02'); assert.equal(last.seq, 2);
  assert.deepEqual(last.author, { kind: 'user', uid: 'alice' });
  await assertFails(send(alice, { mid: 'message03', changeId: 'change005', last: { recalled: true } })); // the marker never rides on a new message
  await assertSucceeds(send(alice, { mid: 'message03', changeId: 'change005', text: 'Fresh' }));
  const next = await lastOf(alice);
  assert.equal(next.preview, 'Fresh'); assert.ok(!('recalled' in next));
});
test('teams: a pointer that is not whole is never completed or recalled by a client write; the next message replaces it', options, async () => {
  const partial = async (cid, pointer) => {
    await create(conversation(cid));
    const parent = `teams/team-test/conversations/${cid}`;
    await admin.doc(`${parent}/messages/message01`).set({ seq: 1, author: { kind: 'user', uid: 'alice' }, kind: 'text', text: 'Partial', createdAt: new Date(), changeSeq: 1, changeId: 'change001' });
    await admin.doc(`${parent}/changes/change001`).set({ seq: 1, messageId: 'message01', operation: 'create', actorUid: 'alice', generation: 1, at: new Date(), textDigest: createHash('sha256').update('Partial').digest('hex') });
    await admin.doc(parent).update({ lastSeq: 1, lastChangeSeq: 1, lastChange: { id: 'change001', seq: 1 }, lastMessage: pointer });
  };
  const alice = user('alice'), at = new Date();
  await partial('no-author', { id: 'message01', seq: 1, at, preview: 'Partial' });
  await partial('no-preview', { id: 'message01', seq: 1, at, author: { kind: 'user', uid: 'alice' } });
  await partial('no-extras', { id: 'message01', seq: 1, at });
  for (const cid of ['no-author', 'no-preview', 'no-extras']) {
    await assertFails(send(alice, { cid, operation: 'edit', mid: 'message01', changeId: 'change002', text: 'Edited' })); // would refresh the preview of a partial pointer
    await assertFails(send(alice, { cid, operation: 'recall', mid: 'message01', changeId: 'change002', pointer: null })); // a recall must blank the preview, so it cannot leave one out
    await assertFails(send(alice, { cid, operation: 'recall', mid: 'message01', changeId: 'change002' }));
    await assertFails(send(alice, { cid, operation: 'edit', mid: 'message01', changeId: 'change002', text: 'Edited', pointer: { 'lastMessage.author': { kind: 'user', uid: 'alice' }, 'lastMessage.preview': 'Edited' } })); // nor completed
    await assertSucceeds(send(alice, { cid, mid: 'message02', changeId: 'change003' })); // a new message writes the whole pointer
    assert.deepEqual(Object.keys(await lastOf(alice, cid)).sort(), ['at', 'author', 'id', 'preview', 'seq']);
  }
});
test('teams: the trusted HTTP route and the rules-checked SDK write the same lastMessage shape and continue each other', options, async () => {
  await create();
  const alice = user('alice'), store = new TeamMessagesStore(admin);
  const http = (operation, messageId, changeId, text) => store.execute({ uid: 'alice' }, { action: 'mutate', teamId: 'team-test', generation: 1, conversationId: 'ordinary', messageId, changeId, operation, text });
  const keys = async () => Object.keys(await lastOf(alice)).sort();
  await http('create', 'message01', 'change001', 'First via HTTP');
  const shape = await keys();
  assert.deepEqual(shape, ['at', 'author', 'id', 'preview', 'seq']);
  await assertSucceeds(send(alice, { mid: 'message02', changeId: 'change002', text: 'Second via SDK' })); // the SDK follows an HTTP-written pointer
  assert.deepEqual(await keys(), shape); assert.deepEqual((await lastOf(alice)).author, { kind: 'user', uid: 'alice' });
  await http('edit', 'message02', 'change003', 'Second, edited\nover HTTP'); // HTTP follows an SDK-written pointer
  assert.equal((await lastOf(alice)).preview, 'Second, edited over HTTP'); assert.deepEqual(await keys(), shape);
  await http('create', 'message03', 'change004', 'Third via HTTP');
  await assertSucceeds(send(alice, { operation: 'edit', mid: 'message03', changeId: 'change005', text: 'Third, edited by SDK' })); // rules accept edits of an HTTP-written pointer
  assert.equal((await lastOf(alice)).preview, 'Third, edited by SDK');
  await assertSucceeds(send(alice, { operation: 'recall', mid: 'message03', changeId: 'change006' }));
  const recalled = await lastOf(alice);
  assert.equal(recalled.preview, ''); assert.equal(recalled.recalled, true); assert.deepEqual(Object.keys(recalled).sort(), [...shape, 'recalled'].sort());
  await http('create', 'message04', 'change007', 'Fourth via HTTP'); // the next message replaces the recalled pointer
  assert.deepEqual(await keys(), shape);
});
