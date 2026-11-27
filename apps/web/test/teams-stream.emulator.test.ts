import * as firestore from 'firebase/firestore';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { initializeApp, deleteApp } from 'firebase/app';
import { initializeAuth, inMemoryPersistence, connectAuthEmulator, signInWithCustomToken } from 'firebase/auth';
import { getFirestore, connectFirestoreEmulator, disableNetwork, enableNetwork, terminate } from 'firebase/firestore';
import { prefetchFirestoreMessages, readFirestoreMessagePage, watchFirestoreMessages } from '../../../packages/teams/firestore.js';
import { emptyProjection, applyMessageSnapshot, TeamMessaging, type MessageApi, type MessageSnapshot } from '../../../packages/teams/messages.js';
import { memoryStorage } from './support/fakeTeams.js';

const enabled = !!process.env.FIRESTORE_EMULATOR_HOST && !!process.env.FIREBASE_AUTH_EMULATOR_HOST;
test('SDK stream: a pre-create directory delivered after the create ACK keeps the new channel selected', { skip: !enabled, timeout: 45000 }, async context => {
  // The renderer keeps its verified membership lease across Firestore subscriptions.
  const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const leases = new Map<string, string>();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => leases.get(key) ?? null,
    setItem: (key: string, value: string) => leases.set(key, value),
    removeItem: (key: string) => leases.delete(key),
  } });
  context.after(() => {
    if (storageDescriptor) Object.defineProperty(globalThis, 'localStorage', storageDescriptor);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  });
  const backend = createRequire(new URL('../../firebase/package.json', import.meta.url));
  // @ts-expect-error The trusted Functions module is JavaScript, exercised directly here.
  const { createTeamConversation } = await import('../../firebase/src/team-conversations.mjs');
  const projectId = 'demo-lingxi-teams-t0', uid = 'channel-creator', otherUid = 'channel-member';
  const teamId = 'channel-create-team', generation = 1;
  const adminApp = backend('firebase-admin/app').initializeApp({ projectId }, 'channel-creation');
  const admin = backend('firebase-admin/firestore').getFirestore(adminApp);
  const app = initializeApp({ projectId, apiKey: 'emulator-only', appId: 'channel-creation' }, 'channel-creation');
  const auth = initializeAuth(app, { persistence: inMemoryPersistence }), db = getFirestore(app);
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST!.split(':'); assert.equal(host, '127.0.0.1');
  connectAuthEmulator(auth, `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`, { disableWarnings: true });
  connectFirestoreEmulator(db, host, Number(port));
  let model: TeamMessaging | undefined;
  let deliverPreCreateDirectory: (() => void) | undefined;
  const snapshots: MessageSnapshot[] = [];
  async function until(predicate: (snapshot: MessageSnapshot) => boolean) {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      const failure = snapshots.find(snapshot => snapshot.kind === 'error');
      assert.equal(failure, undefined, 'Selecting a new channel must keep the team connected');
      const result = snapshots.find(predicate);
      if (result) return result;
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.fail('Timed out waiting for the channel listener');
  }
  try {
    const team = admin.doc(`teams/${teamId}`), conversations = team.collection('conversations');
    const participants = [`user:${uid}`, `user:${otherUid}`];
    const batch = admin.batch();
    batch.set(team, { state: 'active' });
    for (const member of [uid, otherUid]) batch.set(team.collection('members').doc(member), { generation, role: 'member' });
    // Keep the new channel outside the first page so its own listener has to discover it.
    for (let i = 0; i < 50; i++) batch.set(conversations.doc(`000-existing-channel-${i}`), {
      kind: 'channel', title: 'Existing channel', participants, memberUids: [uid, otherUid], lastSeq: 0, lastChangeSeq: 0, lastMessage: null,
    });
    await batch.commit();
    await signInWithCustomToken(auth, await backend('firebase-admin/auth').getAuth(adminApp).createCustomToken(uid));

    const api: MessageApi = {
      async messages(command) {
        assert.equal(command.action, 'create');
        await until(snapshot => !!deliverPreCreateDirectory && snapshot.kind === 'directory'
          && !snapshot.fromCache && snapshot.conversations.length === 50);
        return await createTeamConversation(admin, { teamId, conversationId: command.conversationId,
          callerUid: uid, callerGeneration: generation, kind: command.kind, title: command.title, participants });
      },
      onMessagesChanged: () => () => {},
      watchMessages(scope, id, receive) {
        let paused = false;
        return watchFirestoreMessages(firestore, db, uid, scope, id, snapshot => {
          if (paused) return;
          snapshots.push(snapshot);
          if (id && !deliverPreCreateDirectory && snapshot.kind === 'directory' && !snapshot.fromCache
            && !snapshot.conversations.some(row => row.id === id)) {
            // Keep the first result queued at the transport boundary while the independent create call finishes.
            paused = true;
            deliverPreCreateDirectory = () => receive(snapshot);
            return;
          }
          receive(snapshot);
        }, () => auth.currentUser?.uid === uid);
      },
    };
    model = new TeamMessaging(api, memoryStorage(), uid, { teamId, generation });
    await model.start();
    const conversationId = await model.create('channel', 'Next channel', [uid, otherUid]);
    assert.equal(model.state.creation, undefined, 'HTTP create has succeeded and the pending command is cleared');
    assert.ok(deliverPreCreateDirectory);
    deliverPreCreateDirectory();
    assert.equal(model.selected, conversationId, 'a delayed empty lookup must preserve the selected channel');
    // The directory and message listeners can confirm server state in either order.
    await until(snapshot => snapshot.kind === 'messages' && !snapshot.fromCache
      && snapshot.conversationId === conversationId && snapshot.replace === true && model?.offline === false);
    assert.equal(model.selected, conversationId);
    assert.equal(model.state.conversations.find(row => row.id === conversationId)?.title, 'Next channel');
  } finally { model?.close(); await terminate(db); await deleteApp(app); await admin.terminate(); await backend('firebase-admin/app').deleteApp(adminApp); }
});

test('SDK stream: cached metadata, reconnect, old edits/recalls, read marks, generation and teardown', { skip: !enabled, timeout: 45000 }, async () => {
  const backend = createRequire(new URL('../../firebase/package.json', import.meta.url));
  const projectId = 'demo-lingxi-teams-t0', uid = 'stream-reader', teamId = 'stream-team', conversationId = 'stream-chat';
  const adminApp = backend('firebase-admin/app').initializeApp({ projectId }, 'teams-stream-test');
  const admin = backend('firebase-admin/firestore').getFirestore(adminApp);
  const app = initializeApp({ projectId, apiKey: 'emulator-only', appId: 'stream-test' }, 'stream-test');
  const auth = initializeAuth(app, { persistence: inMemoryPersistence }), db = getFirestore(app);
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST!.split(':'); assert.equal(host, '127.0.0.1');
  connectAuthEmulator(auth, `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`, { disableWarnings: true }); connectFirestoreEmulator(db, host, Number(port));
  const root = admin.doc(`teams/${teamId}`), conversation = root.collection('conversations').doc(conversationId);
  let stop: (() => void) | undefined;
  const snapshots: MessageSnapshot[] = [];
  let view = emptyProjection(uid, { teamId, generation: 1 });
  const until = async (predicate: () => boolean | Promise<boolean>) => { const end = Date.now() + 15000; while (!(await predicate())) { if (Date.now() >= end) throw new Error('SDK stream condition timed out'); await new Promise(resolve => setTimeout(resolve, 10)); } };
  const pointer = () => snapshots.filter((value): value is Extract<MessageSnapshot, { kind: 'directory' }> => value.kind === 'directory').at(-1)?.conversations[0]?.lastMessage;
  function latestMessages(): Extract<MessageSnapshot, { kind: 'messages' }> | undefined { const last = snapshots.filter((value): value is Extract<MessageSnapshot, { kind: 'messages' }> => value.kind === 'messages').at(-1); return last ? { ...last, messages: view.threads[conversationId]?.messages ?? [] } : undefined; }
  try {
    await root.set({ state: 'active' }); await root.collection('members').doc(uid).set({ generation: 1, role: 'member' });
    await conversation.set({ kind: 'dm', title: 'Stream fixture', participants: [`user:${uid}`], memberUids: [uid], lastSeq: 125, lastChangeSeq: 125,
      lastMessage: { id: 'message-125', seq: 125, at: new Date('2026-11-02T10:00:00Z'), author: { kind: 'user', uid }, preview: 'Body 125' } });
    const batch = admin.batch();
    for (let i = 1; i <= 125; i++) batch.set(conversation.collection('messages').doc(`message-${i}`), { seq: i, author: { uid, kind: 'user' }, kind: 'text', text: `Body ${i}`, createdAt: new Date(), changeSeq: i, changeId: `change-${i}` });
    await batch.commit();
    await signInWithCustomToken(auth, await backend('firebase-admin/auth').getAuth(adminApp).createCustomToken(uid));
    const head = firestore.query(firestore.collection(db, 'teams', teamId, 'conversations', conversationId, 'messages'),
      firestore.orderBy('seq', 'desc'), firestore.limit(50));
    assert.equal((await firestore.getDocsFromCache(head)).size, 0);
    const cancelPrefetch = prefetchFirestoreMessages(firestore, db, { teamId, generation: 1 }, conversationId);
    try { await until(async () => (await firestore.getDocsFromCache(head)).size === 50); }
    finally { cancelPrefetch(); }
    stop = watchFirestoreMessages(firestore, db, uid, { teamId, generation: 1 }, conversationId, value => { if (value.kind === 'messages') view = applyMessageSnapshot(view, value); snapshots.push(value); }, () => auth.currentUser?.uid === uid);
    await until(() => latestMessages()?.messages.length === 50 && latestMessages()?.fromCache === false);
    for(const before of [76,26]) {
      const messages=await readFirestoreMessagePage(firestore,db,{teamId,generation:1},conversationId,before);
      assert.ok(messages.length<=50);
      view=applyMessageSnapshot(view,{kind:'messages',uid,teamId,generation:1,conversationId,fromCache:false,replace:false,older:true,messages});
    }
    assert.equal(view.threads[conversationId]?.messages.length,125);
    // The directory row carries the conversation's newest message: real Timestamps arrive as ISO strings.
    await until(() => pointer()?.preview === 'Body 125');
    assert.deepEqual(pointer(), { id: 'message-125', seq: 125, at: '2026-11-02T10:00:00.000Z', preview: 'Body 125', author: { kind: 'user', uid } });
    await disableNetwork(db); await until(() => latestMessages()?.fromCache === true);
    await conversation.collection('messages').doc('message-1').update({ text: 'Edited oldest', changeSeq: 126 });
    await conversation.collection('messages').doc('message-2').update({ text: '', recalled: true, changeSeq: 127 });
    await conversation.collection('changes').doc('change-126').set({seq:126,messageId:'message-1',operation:'edit',at:new Date()});
    await conversation.collection('changes').doc('change-127').set({seq:127,messageId:'message-2',operation:'recall',at:new Date()});
    await conversation.update({lastChangeSeq:127});
    assert.equal(latestMessages()!.messages[0]!.text, 'Body 1');
    await enableNetwork(db); await until(() => latestMessages()?.fromCache === false && latestMessages()?.messages[0]?.text === 'Edited oldest');
    assert.equal(latestMessages()!.messages[1]!.text, ''); assert.equal(latestMessages()!.messages[1]!.recalled, true);
    await conversation.update({ 'lastMessage.preview': '', 'lastMessage.recalled': true }); // a recall of the newest message reaches the list
    await until(() => pointer()?.recalled === true);
    assert.deepEqual(pointer(), { id: 'message-125', seq: 125, at: '2026-11-02T10:00:00.000Z', preview: '', recalled: true, author: { kind: 'user', uid } });
    await conversation.collection('reads').doc(uid).set({ generation: 1, lastReadSeq: 120 });
    await until(() => snapshots.some(value => value.kind === 'read' && value.lastReadSeq === 120));
    await root.collection('members').doc(uid).update({ generation: 2 });
    await until(() => snapshots.some(value => value.kind === 'error' && value.error === 'STALE_MEMBERSHIP'));
    const length = snapshots.length; stop(); await conversation.collection('messages').doc('message-1').update({ text: 'After teardown' });
    await new Promise(resolve => setTimeout(resolve, 100)); assert.equal(snapshots.length, length);
  } finally { stop?.(); await terminate(db); await deleteApp(app); await admin.terminate(); await backend('firebase-admin/app').deleteApp(adminApp); }
});
