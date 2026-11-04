import * as firestore from 'firebase/firestore';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { initializeApp, deleteApp } from 'firebase/app';
import { initializeAuth, inMemoryPersistence, connectAuthEmulator, signInWithCustomToken } from 'firebase/auth';
import { getFirestore, connectFirestoreEmulator, disableNetwork, enableNetwork, terminate } from 'firebase/firestore';
import { watchFirestoreMessages } from '../../../packages/teams/firestore.js';
import { emptyProjection, applyMessageSnapshot, type MessageSnapshot } from '../../../packages/teams/messages.js';

const enabled = !!process.env.FIRESTORE_EMULATOR_HOST && !!process.env.FIREBASE_AUTH_EMULATOR_HOST;
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
  const until = async (predicate: () => boolean) => { const end = Date.now() + 15000; while (!predicate()) { if (Date.now() >= end) throw new Error('SDK stream condition timed out'); await new Promise(resolve => setTimeout(resolve, 10)); } };
  function latestMessages(): Extract<MessageSnapshot, { kind: 'messages' }> | undefined { const last = snapshots.filter((value): value is Extract<MessageSnapshot, { kind: 'messages' }> => value.kind === 'messages').at(-1); return last ? { ...last, messages: view.threads[conversationId]?.messages ?? [] } : undefined; }
  try {
    await root.set({ state: 'active' }); await root.collection('members').doc(uid).set({ generation: 1, role: 'member' });
    await conversation.set({ kind: 'dm', title: 'Stream fixture', participants: [`user:${uid}`], memberUids: [uid], lastSeq: 125, lastChangeSeq: 125 });
    const batch = admin.batch();
    for (let i = 1; i <= 125; i++) batch.set(conversation.collection('messages').doc(`message-${i}`), { seq: i, author: { uid, kind: 'user' }, kind: 'text', text: `Body ${i}`, createdAt: new Date(), changeSeq: i, changeId: `change-${i}` });
    await batch.commit();
    await signInWithCustomToken(auth, await backend('firebase-admin/auth').getAuth(adminApp).createCustomToken(uid));
    stop = watchFirestoreMessages(firestore, db, uid, { teamId, generation: 1 }, conversationId, value => { if (value.kind === 'messages') view = applyMessageSnapshot(view, value); snapshots.push(value); }, () => auth.currentUser?.uid === uid);
    await until(() => latestMessages()?.messages.length === 125 && latestMessages()?.fromCache === false);
    await disableNetwork(db); await until(() => latestMessages()?.fromCache === true);
    await conversation.collection('messages').doc('message-1').update({ text: 'Edited oldest', changeSeq: 126 });
    await conversation.collection('messages').doc('message-2').update({ text: '', recalled: true, changeSeq: 127 });
    assert.equal(latestMessages()!.messages[0]!.text, 'Body 1');
    await enableNetwork(db); await until(() => latestMessages()?.fromCache === false && latestMessages()?.messages[0]?.text === 'Edited oldest');
    assert.equal(latestMessages()!.messages[1]!.text, ''); assert.equal(latestMessages()!.messages[1]!.recalled, true);
    await conversation.collection('reads').doc(uid).set({ generation: 1, lastReadSeq: 120 });
    await until(() => snapshots.some(value => value.kind === 'read' && value.lastReadSeq === 120));
    await root.collection('members').doc(uid).update({ generation: 2 });
    await until(() => snapshots.some(value => value.kind === 'error' && value.error === 'STALE_MEMBERSHIP'));
    const length = snapshots.length; stop(); await conversation.collection('messages').doc('message-1').update({ text: 'After teardown' });
    await new Promise(resolve => setTimeout(resolve, 100)); assert.equal(snapshots.length, length);
  } finally { stop?.(); await terminate(db); await deleteApp(app); await admin.terminate(); await backend('firebase-admin/app').deleteApp(adminApp); }
});
