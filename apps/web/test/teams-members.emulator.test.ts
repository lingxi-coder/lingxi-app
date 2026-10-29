import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { initializeApp, deleteApp } from 'firebase/app';
import { initializeAuth, inMemoryPersistence, connectAuthEmulator, signInWithCustomToken } from 'firebase/auth';
import { getFirestore, connectFirestoreEmulator, terminate } from 'firebase/firestore';
import { webTeamsApi } from '../src/teams/api';
// @ts-expect-error The trusted Functions module is JavaScript, exercised directly here.
import { TeamsStore, createTeamsHandler } from '../../firebase/src/teams.mjs';

const backendRequire = createRequire(new URL('../../firebase/package.json', import.meta.url));
const { initializeApp: adminApp, deleteApp: deleteAdminApp } = backendRequire('firebase-admin/app');
const { getFirestore: adminFirestore } = backendRequire('firebase-admin/firestore');
const { getAuth: adminAuth } = backendRequire('firebase-admin/auth');

test('Teams Web SDK resumes member updates after removal and a fresh membership generation', {
  skip: !process.env.FIRESTORE_EMULATOR_HOST || !process.env.FIREBASE_AUTH_EMULATOR_HOST, timeout: 30000,
}, async context => {
  const projectId = 'demo-lingxi-teams-t0';
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST!.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host!));
  const admin = adminApp({ projectId }, 'team-members-web-sdk');
  const dbAdmin = adminFirestore(admin), authAdmin = adminAuth(admin), store = new TeamsStore(dbAdmin);
  const app = initializeApp({ projectId, apiKey: 'synthetic-api-key' }, 'team-members-web-sdk');
  const auth = initializeAuth(app, { persistence: inMemoryPersistence }), db = getFirestore(app);
  connectAuthEmulator(auth, `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`, { disableWarnings: true });
  connectFirestoreEmulator(db, host!, Number(port));
  const owner = { uid: 'web-members-owner', name: 'Owner' }, memberUid = 'web-members-person', teamId = 'web-members-live';
  const { user } = await signInWithCustomToken(auth, await authAdmin.createCustomToken(memberUid));
  const handle = createTeamsHandler({ store, verifyToken: (token: string) => authAdmin.verifyIdToken(token, true),
    createCustomToken: (uid: string) => authAdmin.createCustomToken(uid) });
  const original = globalThis.fetch.bind(globalThis);
  context.mock.method(globalThis, 'fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    if (typeof input === 'string' && input.startsWith('/v1/teams')) return handle(new Request(`http://fixture.local${input}`, init));
    return original(input, init);
  });
  const api = webTeamsApi(user, db);
  let changed = 0, invalidated = 0;
  const stop = api.onChanged(event => { changed++; if (event.invalidated) invalidated++; });
  const until = async (condition: () => boolean) => {
    const deadline = Date.now() + 10000;
    while (!condition()) { if (Date.now() > deadline) throw new Error('Member listener did not resume'); await new Promise(resolve => setTimeout(resolve, 20)); }
  };
  try {
    await store.execute(owner, { action: 'create', teamId, name: 'Live member fixture' });
    const { code } = await store.execute(owner, { action: 'invite', teamId, requestId: 'live-invite01', role: 'member', maxUses: 3, ttlSeconds: 3600 });
    await api.execute({ action: 'join', code, requestId: 'live-join01' });
    assert.equal((await api.list()).teams.length, 1);
    assert.equal((await api.detail(teamId)).team.generation, 1);
    await until(() => changed - invalidated >= 2); // Both initial server snapshots.
    const previous = invalidated;
    await store.execute(owner, { action: 'remove', teamId, uid: memberUid, generation: 1 });
    await until(() => invalidated > previous);
    await assert.rejects(api.detail(teamId), { code: 'MEMBERSHIP_REQUIRED' });
    await assert.rejects(api.execute({ action: 'join', code, requestId: 'live-join01' }), { code: 'STALE_MEMBERSHIP' });
    const beforeRejoin = changed - invalidated;
    assert.equal((await api.execute({ action: 'join', code, requestId: 'live-join02' })).generation, 2);
    assert.equal((await api.detail(teamId)).team.generation, 2);
    // Let the new listener deliver its initial server snapshot, then change a
    // different member. The caller's own index is unchanged by that join.
    await until(() => changed - invalidated >= beforeRejoin + 2);
    const beforeNewMember = changed;
    await store.execute({ uid: 'web-members-third', name: 'Third member' }, { action: 'join', code, requestId: 'live-join03' });
    await until(() => changed > beforeNewMember);
    assert.equal((await api.detail(teamId)).members.length, 3);
  } finally {
    stop(); api.close();
    await store.execute(owner, { action: 'delete', teamId }).catch(() => undefined);
    await terminate(db); await deleteApp(app); await dbAdmin.terminate(); await deleteAdminApp(admin);
  }
});
