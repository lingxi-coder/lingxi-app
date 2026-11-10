import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { deleteApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, inMemoryPersistence, signInWithCustomToken } from 'firebase/auth';
import { connectFirestoreEmulator, getFirestore, terminate } from 'firebase/firestore';
import { FRIEND_CHAT } from '../../../packages/teams/friends';
import { FriendsWorkspace } from '../../../packages/teams/friends-workspace';
import { webFriendsApi } from '../src/friends/api';
import { webTeamsApi } from '../src/teams/api';
import { memoryStorage } from './support/fakeTeams';
// @ts-expect-error The trusted Functions modules are JavaScript, exercised directly here.
import { FriendsStore } from '../../firebase/src/friends.mjs';
// @ts-expect-error The trusted Functions modules are JavaScript, exercised directly here.
import { TeamsStore, createTeamsHandler } from '../../firebase/src/teams.mjs';

const backendRequire = createRequire(new URL('../../firebase/package.json', import.meta.url));
const { initializeApp: adminApp, deleteApp: deleteAdminApp } = backendRequire('firebase-admin/app');
const { getFirestore: adminFirestore, Timestamp } = backendRequire('firebase-admin/firestore');
const { getAuth: adminAuth } = backendRequire('firebase-admin/auth');

// The real backend (handler, stores, rules) behind the real Web adapters and workspace: what the Friends views stand on.
test('Friends Web SDK: owner-only listeners, a redeemed friend link and its chat, blocking, all against the real backend and rules', {
  skip: !process.env.FIRESTORE_EMULATOR_HOST || !process.env.FIREBASE_AUTH_EMULATOR_HOST, timeout: 60_000,
}, async context => {
  const projectId = 'demo-lingxi-teams-t0';
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST!.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host!));
  const admin = adminApp({ projectId }, 'friends-web-sdk'), authAdmin = adminAuth(admin), dbAdmin = adminFirestore(admin);
  const friends = new FriendsStore(dbAdmin, Date.now, { auth: authAdmin });
  const handle = createTeamsHandler({ store: new TeamsStore(dbAdmin), friends, verifyToken: (token: string) => authAdmin.verifyIdToken(token, true),
    createCustomToken: (uid: string) => authAdmin.createCustomToken(uid) });
  const app = initializeApp({ projectId, apiKey: 'synthetic-api-key' }, 'friends-web-sdk');
  const auth = initializeAuth(app, { persistence: inMemoryPersistence }), db = getFirestore(app);
  connectAuthEmulator(auth, `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`, { disableWarnings: true });
  connectFirestoreEmulator(db, host!, Number(port));
  const alice = 'web-friends-alice', bob = 'web-friends-bob';
  const { user } = await signInWithCustomToken(auth, await authAdmin.createCustomToken(alice));
  const original = globalThis.fetch.bind(globalThis);
  context.mock.method(globalThis, 'fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    if (typeof input === 'string' && (input.startsWith('/v1/friends') || input.startsWith('/v1/teams'))) return handle(new Request(`http://fixture.local${input}`, init));
    return original(input, init);
  });
  const friendsApi = webFriendsApi(user, db), teamsApi = webTeamsApi(user, db);
  const workspace = new FriendsWorkspace(friendsApi, teamsApi, alice, { chat: { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 20 } });
  const snapshot = () => workspace.getSnapshot();
  const until = async (condition: () => boolean, what: string) => {
    const deadline = Date.now() + 15_000;
    while (!condition()) { if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`); await new Promise(resolve => setTimeout(resolve, 20)); }
  };
  try {
    // The first snapshots arrive empty, and the profile creates itself with its first friend link (D2).
    await until(() => !snapshot().loading && !snapshot().blocksLoading && !!snapshot().profile, 'the first snapshots and the profile');
    assert.deepEqual(snapshot().cards, []); assert.deepEqual(snapshot().blocks, []); assert.equal(snapshot().badge, 0);
    assert.equal(snapshot().profile?.displayName, 'Member', 'the backend\'s fallback name');
    assert.match(snapshot().profile!.friendCode!.link, /^https:\/\/lingxios\.net\/open\/friend\/add\?code=[A-Za-z0-9_-]{32}$/u);
    assert.equal(await workspace.saveProfile({ displayName: 'Alice' }), ''); assert.equal(snapshot().profile?.displayName, 'Alice');

    // Bob's link: look it up (nothing is added), then confirm; the friendship arrives as a card and opens the chat.
    const { friendCode } = await friends.profile({ uid: bob, name: 'Bob' });
    const found = await workspace.lookup({ kind: 'code', code: friendCode.code });
    assert.deepEqual('result' in found ? [found.result.uid, found.result.displayName, found.result.relation] : found, [bob, 'Bob', 'none']);
    assert.deepEqual(snapshot().cards, [], 'a lookup adds nothing');
    const confirmed = await workspace.confirm({ kind: 'code', code: friendCode.code }, 'result' in found ? found.result : { uid: bob, displayName: 'Bob', relation: 'none' }, '');
    assert.deepEqual(confirmed, { done: 'friends', name: 'Bob' });
    await until(() => snapshot().cards.length === 1 && !!snapshot().chat?.workspace, 'the friend\'s card and the chat');
    const card = snapshot().cards[0]!;
    assert.deepEqual([card.uid, card.state, card.displayName], [bob, 'friends', 'Bob']);
    assert.match(card.teamId!, /^f_[0-9a-f]{40}$/u); assert.ok(card.generation! >= 1);
    assert.ok(Math.abs(card.since - Date.now()) < 5 * 60_000, 'the Timestamp arrives as whole epoch milliseconds');

    // The chat is the pair team's one conversation, read and written through the Teams messages API.
    const pair = snapshot().chat!.workspace!;
    await until(() => pair.getSnapshot().conversation?.id === FRIEND_CHAT && !!pair.getSnapshot().messaging, 'the pair team\'s conversation');
    assert.equal(pair.getSnapshot().team?.id, card.teamId); assert.equal(pair.getSnapshot().selectedConversationId, FRIEND_CHAT);
    await pair.getSnapshot().messaging!.queue('create', 'Hello Bob');
    await until(() => pair.getSnapshot().messages.some(message => message.text === 'Hello Bob'), 'the sent message in the thread');
    const stored = await dbAdmin.collection(`teams/${card.teamId}/conversations/${FRIEND_CHAT}/messages`).get();
    assert.deepEqual(stored.docs.map((document: { data(): { text: string } }) => document.data().text), ['Hello Bob']);

    // What the backend keeps on the card as the chat moves (D13) is read from real documents, Timestamps and all.
    workspace.show('home');
    assert.equal(snapshot().badge, 0);
    const at = new Date('2026-11-03T09:30:00.000Z');
    await dbAdmin.doc(`users/${alice}/friends/${bob}`).update({ lastSeq: 3, lastReadSeq: 1, lastChangeSeq: 3,
      lastMessage: { id: 'message-3', seq: 3, at: Timestamp.fromDate(at), author: { kind: 'user', uid: bob }, preview: 'Are you there?' } });
    await until(() => snapshot().cards[0]?.lastSeq === 3, 'the counters');
    assert.deepEqual([snapshot().cards[0]!.lastReadSeq, snapshot().cards[0]!.lastChangeSeq], [1, 3]);
    assert.deepEqual(snapshot().cards[0]!.lastMessage, { id: 'message-3', seq: 3, at: at.toISOString(), author: { kind: 'user', uid: bob }, preview: 'Are you there?' });
    assert.equal(snapshot().badge, 2, 'two messages are unread');
    workspace.openChat(bob);
    assert.equal(snapshot().badge, 0, 'the chat on screen is read');
    workspace.show('home');

    // Blocking removes the friend, the block entry keeps the name, and unblocking restores nothing (D12).
    assert.equal(await workspace.leave(bob, 'block'), '');
    await until(() => snapshot().cards.length === 0 && snapshot().blocks.length === 1, 'the block');
    assert.deepEqual([snapshot().blocks[0]!.uid, snapshot().blocks[0]!.displayName], [bob, 'Bob']);
    assert.ok(Math.abs(snapshot().blocks[0]!.at - Date.now()) < 5 * 60_000);
    assert.equal(snapshot().page, 'home');
    assert.equal(await workspace.unblock(bob), '');
    await until(() => snapshot().blocks.length === 0, 'the unblock');
    assert.deepEqual(snapshot().cards, []);

    // Closing the workspace ends both listeners.
    workspace.close();
    await dbAdmin.doc(`users/${alice}/friends/after-close`).set({ state: 'incoming', displayName: 'Late', since: Timestamp.now() });
    await new Promise(resolve => setTimeout(resolve, 200));
    assert.deepEqual(snapshot().cards, [], 'nothing is delivered after close');
  } finally {
    workspace.close(); friendsApi.close(); teamsApi.close();
    await terminate(db); await deleteApp(app); await dbAdmin.terminate(); await deleteAdminApp(admin);
  }
});
