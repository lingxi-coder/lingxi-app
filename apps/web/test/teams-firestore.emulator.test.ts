import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRequire } from 'node:module';
import { deleteApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, inMemoryPersistence, signInWithCustomToken, signOut } from 'firebase/auth';
import { connectFirestoreEmulator, getFirestore, terminate } from 'firebase/firestore';
import { watchTeamChanges } from '../src/teams/firestore.js';

const enabled = !!process.env.FIRESTORE_EMULATOR_HOST && !!process.env.FIREBASE_AUTH_EMULATOR_HOST;
test('teams web: real SDK authentication and change listener recover an old-message edit', { skip: !enabled, timeout: 30_000 }, async () => {
  const backend = createRequire(new URL('../../firebase/package.json', import.meta.url));
  const adminApp = backend('firebase-admin/app').initializeApp({ projectId: 'demo-lingxi-teams-t0' }, 'teams-web-spike');
  const admin = backend('firebase-admin/firestore').getFirestore(adminApp);
  const app = initializeApp({ projectId: 'demo-lingxi-teams-t0', apiKey: 'emulator-only', appId: 'teams-web-spike' }, 'teams-web-spike');
  const auth = initializeAuth(app, { persistence: inMemoryPersistence });
  const db = getFirestore(app);
  let stop: (() => void) | undefined;
  try {
    const [host, port] = process.env.FIRESTORE_EMULATOR_HOST!.split(':');
    assert.ok(['127.0.0.1', 'localhost'].includes(host));
    connectAuthEmulator(auth, `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`, { disableWarnings: true });
    connectFirestoreEmulator(db, host, Number(port));
    await admin.doc('teams/web-spike').set({ name: 'Synthetic Web spike', state: 'active' });
    await admin.doc('teams/web-spike/members/alice').set({ role: 'member', generation: 1 });
    await admin.doc('teams/web-spike/conversations/sdk-chat').set({ kind: 'dm', participants: ['user:alice'], memberUids: ['alice'], lastSeq: 1, lastChangeSeq: 1 });
    await signInWithCustomToken(auth, await backend('firebase-admin/auth').getAuth(adminApp).createCustomToken('alice'));
    const received = new Promise<void>((resolve, reject) => {
      stop = watchTeamChanges(db, 'web-spike', 'sdk-chat', 1, snapshot => {
        if (snapshot.docs.some(change => change.id === 'change002' && change.data().operation === 'edit')) resolve();
      }, reject);
    });
    await admin.doc('teams/web-spike/conversations/sdk-chat/changes/change002').set({ seq: 2, messageId: 'message01', operation: 'edit', actorUid: 'alice', at: new Date() });
    await received;
    assert.throws(() => watchTeamChanges(db, 'invalid/path', 'sdk-chat', 0, () => {}, () => {}), /Invalid/u);
  } finally {
    stop?.();
    await terminate(db); await signOut(auth); await deleteApp(app);
    await admin.terminate(); await backend('firebase-admin/app').deleteApp(adminApp);
  }
});
