import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { assertFails, assertSucceeds, initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getFirestore as adminFirestore } from 'firebase-admin/firestore';
import { collection, collectionGroup, deleteDoc, doc, getDoc, getDocs, query, serverTimestamp, setDoc, updateDoc, where, writeBatch } from 'firebase/firestore';
import { FriendsStore, pairKey, pairTeamId } from '../src/friends.mjs';
import { PAIR_CONVERSATION_ID } from '../src/teams.mjs';
import { messagePreview } from '../src/team-message-preview.mjs';

const projectId = 'demo-lingxi-teams-t0';
const emulator = process.env.FIRESTORE_EMULATOR_HOST;
const options = { skip: !emulator, timeout: 60_000 };
const alice = { uid: 'alice', name: 'Alice' }, bob = { uid: 'bob', name: 'Bob' }, carol = { uid: 'carol', name: 'Carol' }, dave = { uid: 'dave', name: 'Dave' };
const sha = value => createHash('sha256').update(value).digest('hex');
let environment, adminApp, admin, friends, serial = 0;
const user = uid => environment.authenticatedContext(uid).firestore();
const anonymous = () => environment.unauthenticatedContext().firestore();
const act = (actor, body) => friends.execute(actor, body);
const rid = () => `request-${String(++serial).padStart(6, '0')}`;
// Admin Auth stand-in: every test user owns a verified address, which is how a request names its target here.
const mail = person => `${person.uid}@example.com`;
const accounts = new Map([alice, bob, carol, dave].map(person => [mail(person), { uid: person.uid, emailVerified: true, displayName: person.name }]));
const auth = { async getUserByEmail(address) {
  const account = accounts.get(address);
  if (!account) throw Object.assign(new Error('There is no user record'), { code: 'auth/user-not-found' });
  return account;
} };
const request = (from, to) => act(from, { action: 'request', via: 'email', email: mail(to), requestId: rid() });
// Friends through a request and an accept, the path every client uses; Alice and Bob are the pair under test.
async function befriend(requester, accepter) {
  await friends.profile(accepter);
  await request(requester, accepter);
  await act(accepter, { action: 'accept', otherUid: requester.uid });
}

before(async () => {
  if (!emulator) return;
  const [host, port] = emulator.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host), 'tests must use a local emulator');
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  adminApp = initializeApp({ projectId }, 'friends-rule-tests');
  admin = adminFirestore(adminApp);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore();
  friends = new FriendsStore(admin, Date.now, { auth });
});
after(async () => {
  await environment?.cleanup();
  await admin?.terminate();
  if (adminApp) await deleteApp(adminApp);
});

test('friends rules: a user reads only their own friends index and block list, and no client can write either', options, async () => {
  await befriend(bob, alice); // alice <-> bob
  await friends.profile(carol); await request(carol, alice); // carol -> alice pending
  await friends.profile(dave); await act(alice, { action: 'block', otherUid: 'dave' });
  const mine = user('alice');
  const cards = await assertSucceeds(getDocs(collection(mine, 'users/alice/friends')));
  assert.deepEqual(cards.docs.map(card => [card.id, card.data().state]).sort(), [['bob', 'friends'], ['carol', 'incoming']]);
  await assertSucceeds(getDoc(doc(mine, 'users/alice/friends/bob')));
  assert.deepEqual((await assertSucceeds(getDocs(collection(mine, 'users/alice/blocks')))).docs.map(block => block.id), ['dave']);
  await assertSucceeds(getDoc(doc(mine, 'users/alice/blocks/dave')));
  // Everyone else, signed in or not, sees neither, even the other end of the same friendship.
  for (const other of [user('bob'), user('carol'), user('dave'), anonymous()]) {
    await assertFails(getDoc(doc(other, 'users/alice/friends/bob')));
    await assertFails(getDocs(collection(other, 'users/alice/friends')));
    await assertFails(getDoc(doc(other, 'users/alice/blocks/dave')));
    await assertFails(getDocs(collection(other, 'users/alice/blocks')));
  }
  // The query-everyone shortcut is closed too.
  await assertFails(getDocs(collectionGroup(mine, 'friends')));
  await assertFails(getDocs(query(collectionGroup(mine, 'blocks'), where('at', '>', new Date(0)))));
  // Writes belong to the backend: a card, a forged friendship or a self-made block is refused for its owner too.
  const own = (path, value) => [setDoc(doc(mine, path), value), updateDoc(doc(mine, path), value), deleteDoc(doc(mine, path))];
  for (const attempt of [...own('users/alice/friends/bob', { state: 'friends' }), ...own('users/alice/friends/mallory', { state: 'friends', teamId: 'f_forged' }),
    ...own('users/alice/blocks/bob', { at: new Date() }), ...own('users/alice/blocks/dave', { at: new Date() })]) await assertFails(attempt);
  await assertFails(setDoc(doc(user('bob'), 'users/alice/friends/bob'), { state: 'friends' }));
  await assertFails(setDoc(doc(user('mallory'), 'users/mallory/friends/alice'), { state: 'friends', teamId: pairTeamId('alice', 'mallory', 1) }));
  assert.equal((await admin.doc('users/alice/friends/bob').get()).data().state, 'friends');
  assert.equal((await admin.doc('users/mallory/friends/alice').get()).exists, false);
});

test('friends rules: a profile opens only for its owner and for a friend whose card says friends — never for a stranger, a pending request, a former friend or a blocker', options, async () => {
  for (const person of [alice, bob, carol, dave]) await friends.profile(person);
  await act(alice, { action: 'profile', handle: 'alice_w' });
  const read = async (reader, owner) => getDoc(doc(user(reader), `profiles/${owner}`));
  const profile = (await assertSucceeds(read('alice', 'alice'))).data();
  assert.deepEqual([profile.displayName, profile.handle], ['Alice', 'alice_w']);
  await assertFails(read('bob', 'alice')); // strangers
  await assertFails(getDoc(doc(anonymous(), 'profiles/alice')));
  // A request, on either end, is not a friendship.
  await request(bob, alice);
  await assertFails(read('bob', 'alice')); await assertFails(read('alice', 'bob'));
  await act(alice, { action: 'accept', otherUid: 'bob' });
  assert.equal((await assertSucceeds(read('bob', 'alice'))).data().displayName, 'Alice');
  assert.equal((await assertSucceeds(read('alice', 'bob'))).data().displayName, 'Bob');
  await assertFails(read('carol', 'alice')); await assertFails(read('carol', 'bob')); // a friend's friend is nobody
  // Only the reader's own card counts: a card in the other direction does not open the reader's access to anyone.
  await assertFails(read('alice', 'carol'));
  // Neither a collection read nor any client write is possible, for the owner as for a friend.
  await assertFails(getDocs(collection(user('alice'), 'profiles')));
  await assertFails(getDocs(query(collection(user('bob'), 'profiles'), where('findByHandle', '==', true))));
  await assertFails(getDocs(query(collectionGroup(user('alice'), 'profiles'))));
  for (const [writer, owner] of [['alice', 'alice'], ['bob', 'alice'], ['carol', 'carol']]) {
    await assertFails(setDoc(doc(user(writer), `profiles/${owner}`), { displayName: 'Changed', findByHandle: true }));
    await assertFails(updateDoc(doc(user(writer), `profiles/${owner}`), { displayName: 'Changed' }));
    await assertFails(deleteDoc(doc(user(writer), `profiles/${owner}`)));
  }
  assert.equal((await admin.doc('profiles/alice').get()).data().displayName, 'Alice');
  // A client-made card cannot open a profile.
  await assertFails(setDoc(doc(user('carol'), 'users/carol/friends/alice'), { state: 'friends' }));
  await assertFails(read('carol', 'alice'));
  // Ending the friendship closes both profiles again — by removal and by block.
  await act(bob, { action: 'remove', otherUid: 'alice' });
  await assertFails(read('bob', 'alice')); await assertFails(read('alice', 'bob'));
  await act(alice, { action: 'redeem', code: (await friends.profile(bob)).friendCode.code, requestId: rid() });
  await assertSucceeds(read('alice', 'bob')); await assertSucceeds(read('bob', 'alice'));
  await act(bob, { action: 'block', otherUid: 'alice' });
  await assertFails(read('bob', 'alice')); await assertFails(read('alice', 'bob'));
  await assertSucceeds(read('alice', 'alice'));
});

test('friends rules: handles, friend codes, friendships, limits and request receipts have no client access at all', options, async () => {
  await friends.profile(alice); await act(alice, { action: 'profile', handle: 'alice_w' });
  const code = (await friends.profile(alice)).friendCode.code;
  await befriend(bob, alice);
  await act(bob, { action: 'lookup', handle: 'alice_w' }); // spends a limit document
  const sensitive = [`handles/alice_w`, `friendCodes/${sha(code)}`, `friendships/${pairKey('alice', 'bob')}`, `friendLimits/${sha('lookup:bob')}`];
  const receipt = (await admin.collection('users/bob/friendRequests').get()).docs[0];
  assert.ok(receipt, 'the request left a receipt');
  sensitive.push(`users/bob/friendRequests/${receipt.id}`);
  for (const path of sensitive) assert.ok((await admin.doc(path).get()).exists, path);
  for (const client of [user('alice'), user('bob'), user('carol'), anonymous()]) {
    for (const path of sensitive) {
      await assertFails(getDoc(doc(client, path)));
      await assertFails(setDoc(doc(client, path), { uid: 'carol' }));
      await assertFails(deleteDoc(doc(client, path)));
    }
    for (const path of ['handles', 'friendCodes', 'friendships', 'friendLimits', 'users/bob/friendRequests']) await assertFails(getDocs(collection(client, path)));
  }
  // The raw code lives only in the Admin-only document; no query shape reaches it.
  await assertFails(getDocs(query(collectionGroup(user('alice'), 'friendCodes'), where('code', '==', code))));
  await assertFails(getDocs(query(collection(user('alice'), 'friendCodes'), where('uid', '==', 'alice'))));
  await assertFails(getDocs(query(collection(user('alice'), 'friendships'), where('uids', 'array-contains', 'alice'))));
  assert.equal((await admin.doc(`friendCodes/${sha(code)}`).get()).data().code, code);
});

test('friends rules: a pair team follows the ordinary Team rules — both friends read it and speak in its one chat, no one else can, and unfriending closes all of it', options, async () => {
  await befriend(bob, alice);
  await friends.profile(carol);
  const teamId = pairTeamId('alice', 'bob', 1), team = `teams/${teamId}`, chat = `${team}/conversations/${PAIR_CONVERSATION_ID}`;
  // alice / bob are members, carol is not.
  for (const member of ['alice', 'bob']) {
    const client = user(member);
    assert.equal((await assertSucceeds(getDoc(doc(client, team)))).data().kind, 'pair');
    assert.deepEqual((await assertSucceeds(getDocs(collection(client, `${team}/members`)))).docs.map(item => item.id).sort(), ['alice', 'bob']);
    assert.deepEqual((await assertSucceeds(getDoc(doc(client, chat)))).data().memberUids, ['alice', 'bob']);
    assert.equal((await assertSucceeds(getDocs(collection(client, `users/${member}/teams`)))).size, 0, 'a pair team has no entry in the Team list');
  }
  for (const outsider of [user('carol'), anonymous()]) {
    await assertFails(getDoc(doc(outsider, team)));
    await assertFails(getDocs(collection(outsider, `${team}/members`)));
    await assertFails(getDoc(doc(outsider, chat)));
  }
  // The same message mutation the Team rules already allow works in the pair chat, from either friend.
  async function post(client, path, uid, number, text) {
    const conversation = doc(client, path), current = (await getDoc(conversation)).data();
    const seq = current.lastSeq + 1, changeSeq = current.lastChangeSeq + 1, author = { kind: 'user', uid };
    const messageId = `message${String(number).padStart(2, '0')}`, changeId = `change${String(number).padStart(3, '0')}`;
    const batch = writeBatch(client);
    batch.update(conversation, { lastSeq: seq, lastChangeSeq: changeSeq, lastChange: { id: changeId, seq: changeSeq },
      lastMessage: { id: messageId, seq, at: serverTimestamp(), author, preview: messagePreview(text) } });
    batch.set(doc(conversation, 'messages', messageId), { seq, author, kind: 'text', text, createdAt: serverTimestamp(), changeSeq, changeId });
    batch.set(doc(conversation, 'changes', changeId), { seq: changeSeq, messageId, operation: 'create', actorUid: uid, generation: 1,
      at: serverTimestamp(), textDigest: createHash('sha256').update(text).digest('hex') });
    return batch.commit();
  }
  await assertSucceeds(post(user('alice'), chat, 'alice', 1, 'Hello Bob'));
  await assertSucceeds(post(user('bob'), chat, 'bob', 2, 'Hello Alice'));
  assert.equal((await assertSucceeds(getDoc(doc(user('bob'), `${chat}/messages/message01`)))).data().text, 'Hello Bob');
  assert.equal((await assertSucceeds(getDocs(collection(user('alice'), `${chat}/messages`)))).size, 2);
  await assertFails(post(user('carol'), chat, 'carol', 3, 'Let me in'));
  await assertFails(post(user('alice'), chat, 'bob', 3, 'Forged author')); // an author must be the signed-in member
  assert.equal((await admin.doc(chat).get()).data().lastSeq, 2);
  // Structure is the backend's: no extra conversation, member, invite or state change from a member.
  await assertFails(setDoc(doc(user('alice'), `${team}/conversations/extra_chat`), { kind: 'dm', title: 'Extra', participants: ['user:alice', 'user:bob'], memberUids: ['alice', 'bob'],
    createdBy: 'alice', requestDigest: 'x', lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null }));
  await assertFails(updateDoc(doc(user('alice'), team), { state: 'deleted' }));
  await assertFails(setDoc(doc(user('alice'), `${team}/members/carol`), { role: 'member', generation: 1, displayName: 'Carol' }));
  await assertFails(setDoc(doc(user('alice'), `${team}/invites/${sha('x')}`), { role: 'member' }));
  await assertFails(deleteDoc(doc(user('alice'), `${team}/members/bob`)));
  // Unfriending is team deletion: both lose the team, its members, the chat and every message at once.
  await act(alice, { action: 'remove', otherUid: 'bob' });
  for (const former of ['alice', 'bob']) {
    const client = user(former);
    await assertFails(getDoc(doc(client, team)));
    await assertFails(getDocs(collection(client, `${team}/members`)));
    await assertFails(getDoc(doc(client, chat)));
    await assertFails(getDoc(doc(client, `${chat}/messages/message01`)));
    await assertFails(getDocs(collection(client, `${chat}/messages`)));
    await assertFails(getDocs(collection(client, `${chat}/changes`)));
  }
  await assertFails(post(user('bob'), chat, 'bob', 3, 'Anyone there?'));
  await assertFails(post(user('alice'), chat, 'alice', 3, 'Anyone there?'));
  assert.equal((await admin.doc(chat).get()).data().lastSeq, 2, 'the conversation is retained, unreachable');
  // A brand-new friendship is a new team and a fresh chat; the old one stays closed.
  await befriend(alice, bob);
  const fresh = `teams/${pairTeamId('alice', 'bob', 2)}/conversations/${PAIR_CONVERSATION_ID}`;
  assert.equal((await assertSucceeds(getDoc(doc(user('bob'), fresh)))).data().lastSeq, 0);
  await assertFails(getDoc(doc(user('bob'), `${chat}/messages/message01`)));
  await assertSucceeds(post(user('alice'), fresh, 'alice', 1, 'Fresh start'));
  await assertSucceeds(post(user('bob'), fresh, 'bob', 2, 'Welcome back'));
  assert.equal((await admin.doc(chat).get()).data().lastSeq, 2, 'the old chat never moves again');
});
