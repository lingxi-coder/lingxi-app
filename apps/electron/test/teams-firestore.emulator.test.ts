import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { createRequire } from 'node:module';
import { getApps } from 'firebase/app';
import { openTeamsFirestoreSession, type TeamsFirestoreOptions } from '../src/main/teams/firestore.js';

const backend = createRequire(new URL('../../firebase/package.json', import.meta.url));
const { initializeApp, deleteApp } = backend('firebase-admin/app');
const { getAuth } = backend('firebase-admin/auth');
const { getFirestore } = backend('firebase-admin/firestore');
const projectId = 'demo-lingxi-teams-t0';
const enabled = !!process.env.FIRESTORE_EMULATOR_HOST && !!process.env.FIREBASE_AUTH_EMULATOR_HOST;
const testOptions = { skip: !enabled, timeout: 30_000 };
let app: ReturnType<typeof initializeApp>;
let admin: ReturnType<typeof getFirestore>;
before(async () => {
  if (!enabled) return;
  app = initializeApp({ projectId }, 'teams-sdk-spike');
  admin = getFirestore(app);
  await admin.doc('teams/main-spike').set({ name: 'Synthetic main spike', state: 'active' });
  await admin.doc('teams/main-spike/members/alice').set({ role: 'member', generation: 1 });
  await admin.doc('teams/main-spike/conversations/sdk-chat').set({ kind: 'dm', participants: ['user:alice'], memberUids: ['alice'], lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null });
});
after(async () => { await admin?.terminate(); if (app) await deleteApp(app); });
async function options(uid = 'alice'): Promise<TeamsFirestoreOptions> {
  const [host, firestorePort] = process.env.FIRESTORE_EMULATOR_HOST!.split(':');
  const [, authPort] = process.env.FIREBASE_AUTH_EMULATOR_HOST!.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  return { config: { projectId, apiKey: 'emulator-only', appId: 'teams-main-spike' }, uid,
    customToken: await getAuth(app).createCustomToken(uid),
    emulator: { host: host as '127.0.0.1', authPort: Number(authPort), firestorePort: Number(firestorePort) } };
}

test('teams main: custom-token sign-in receives a real Firestore listener and closes cleanly', testOptions, async () => {
  const session = await openTeamsFirestoreSession(await options());
  try {
    const received = new Promise<void>((resolve, reject) => {
      session.watchChanges('main-spike', 'sdk-chat', 1, snapshot => {
        if (snapshot.docs.some(change => change.id === 'change002' && change.data().operation === 'recall')) resolve();
      }, reject);
    });
    await admin.doc('teams/main-spike/conversations/sdk-chat/changes/change002').set({ seq: 2, messageId: 'message01', operation: 'recall', actorUid: 'alice', at: new Date() });
    await received;
  } finally { await session.close(); }
  assert.throws(() => session.watchChanges('main-spike', 'sdk-chat', 0, () => {}, () => {}), /closed/u);
  await session.close();
  assert.equal(getApps().filter(candidate => candidate.name.startsWith('lingxi-teams-main-')).length, 0);
});
test('teams main: exchanged account must match the AccountService UID', testOptions, async () => {
  await assert.rejects(openTeamsFirestoreSession({ ...await options(), uid: 'bob' }), /another account/u);
  assert.equal(getApps().filter(candidate => candidate.name.startsWith('lingxi-teams-main-')).length, 0);
});
test('teams main: valid login without team membership gets listener permission-denied', testOptions, async () => {
  const session = await openTeamsFirestoreSession(await options('outsider'));
  try {
    const failure = await new Promise<Error & { code: string }>(resolve => {
      session.watchChanges('main-spike', 'sdk-chat', 0, () => {}, error => resolve(error));
    });
    assert.equal(failure.code, 'permission-denied');
  } finally { await session.close(); }
});
test('teams main: emulator options reject production project IDs before creating a session', async () => {
  await assert.rejects(openTeamsFirestoreSession({ config: { projectId: 'production-project' }, uid: 'alice', customToken: 'unused', emulator: { host: '127.0.0.1', authPort: 9098, firestorePort: 8085 } }), /demo project/u);
});
test('teams main: a rejected SDK initialization releases its app registration', async () => {
  await assert.rejects(openTeamsFirestoreSession({ config: { projectId: 'demo-invalid-config' }, uid: 'alice', customToken: 'unused' }), /invalid-api-key/u);
  assert.equal(getApps().filter(candidate => candidate.name.startsWith('lingxi-teams-main-')).length, 0);
});
