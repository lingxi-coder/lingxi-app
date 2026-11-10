import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { getApps } from 'firebase/app';
import * as sdk from '../src/main/teams/firestore.js';
import { FriendsService } from '../src/main/teams/friends';
// @ts-expect-error The trusted Functions modules are JavaScript, exercised directly here.
import { FriendsStore } from '../../firebase/src/friends.mjs';
// @ts-expect-error The trusted Functions modules are JavaScript, exercised directly here.
import { TeamsStore, createTeamsHandler } from '../../firebase/src/teams.mjs';

const backend = createRequire(new URL('../../firebase/package.json', import.meta.url));
const { initializeApp, deleteApp } = backend('firebase-admin/app');
const { getAuth } = backend('firebase-admin/auth');
const { getFirestore, Timestamp } = backend('firebase-admin/firestore');
const projectId = 'demo-lingxi-teams-t0';
const enabled = !!process.env.FIRESTORE_EMULATOR_HOST && !!process.env.FIREBASE_AUTH_EMULATOR_HOST;

// The main process's view of Friends on the real SDK session, the real HTTP handler and the real rules.
test('friends main: cards and blocks reach the renderer from the real session, actions run through the real backend, and another account ends it all', { skip: !enabled, timeout: 60_000 }, async context => {
  const app = initializeApp({ projectId }, 'friends-main-sdk'), admin = getFirestore(app);
  const friends = new FriendsStore(admin, Date.now, { auth: getAuth(app) });
  // The account's token is its uid: the handler's verifier stands in for Firebase Auth, which the session below goes through for real.
  const handle = createTeamsHandler({ store: new TeamsStore(admin), friends, verifyToken: async (token: string) => ({ uid: token }), createCustomToken: (uid: string) => getAuth(app).createCustomToken(uid) });
  const [host, firestorePort] = process.env.FIRESTORE_EMULATOR_HOST!.split(':');
  const [, authPort] = process.env.FIREBASE_AUTH_EMULATOR_HOST!.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host!));
  const alice = 'main-friends-alice', bob = 'main-friends-bob', stranger = 'main-friends-stranger';
  const session = await sdk.openTeamsFirestoreSession({ config: { projectId, apiKey: 'emulator-only', appId: 'friends-main-sdk' }, uid: alice,
    customToken: await getAuth(app).createCustomToken(alice), emulator: { host: host as '127.0.0.1', authPort: Number(authPort), firestorePort: Number(firestorePort) } });
  let uid = alice, changed: (() => void) | undefined;
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: async () => uid, onChanged(callback: () => void) { changed = callback; return () => { changed = undefined; }; } };
  const original = globalThis.fetch.bind(globalThis);
  context.mock.method(globalThis, 'fetch', async (input: string | URL, init?: RequestInit) =>
    String(input).startsWith('http://fixture.local/') ? handle(new Request(String(input), init)) : original(input, init));
  const service = new FriendsService(account, { firestore: async expected => { if (expected !== uid) throw new Error('INVALID_SESSION'); return { db: session.db, sdk }; } }, 'http://fixture.local');
  const rows: { leaseId: string; rows: any[] }[] = [], failed: { leaseId: string; code: string }[] = [];
  service.onRows(event => rows.push(event)); service.onFailed(event => failed.push(event));
  const latest = (leaseId: string) => rows.filter(event => event.leaseId === leaseId).at(-1)?.rows;
  const until = async (condition: () => boolean, what: string) => {
    const deadline = Date.now() + 15_000;
    while (!condition()) { if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`); await new Promise(resolve => setTimeout(resolve, 20)); }
  };
  try {
    await service.watch('1:cards:', alice, 'friends'); await service.watch('1:blocks:', alice, 'blocks');
    await until(() => latest('1:cards:') !== undefined && latest('1:blocks:') !== undefined, 'the first snapshots');
    assert.deepEqual(latest('1:cards:'), []); assert.deepEqual(latest('1:blocks:'), []);

    // A friend link redeemed through the main process: the card arrives as plain values with the Timestamp in milliseconds.
    assert.equal((await service.profile(alice)).displayName, 'Member');
    const { friendCode } = await friends.profile({ uid: bob, name: 'Bob' });
    const redeemed = await service.execute(alice, { action: 'redeem', code: friendCode.code, requestId: 'main-redeem-01' });
    assert.deepEqual([redeemed.state, redeemed.uid], ['friends', bob]);
    await until(() => latest('1:cards:')?.length === 1, 'the friend\'s card');
    const [card] = latest('1:cards:')!;
    assert.deepEqual([card.uid, card.state, card.displayName, card.teamId], [bob, 'friends', 'Bob', redeemed.teamId]);
    assert.ok(Math.abs(card.since - Date.now()) < 5 * 60_000 && Number.isInteger(card.since)); assert.ok(card.generation >= 1);
    await admin.doc(`users/${alice}/friends/${bob}`).update({ lastSeq: 2, lastReadSeq: 0, lastChangeSeq: 2,
      lastMessage: { id: 'message-2', seq: 2, at: Timestamp.fromDate(new Date('2026-11-03T09:30:00.000Z')), author: { kind: 'user', uid: bob }, preview: 'Hi' } });
    await until(() => latest('1:cards:')?.[0]?.lastSeq === 2, 'the counters');
    assert.deepEqual(latest('1:cards:')![0].lastMessage, { id: 'message-2', seq: 2, at: '2026-11-03T09:30:00.000Z', author: { kind: 'user', uid: bob }, preview: 'Hi' });

    // D10: a friend's profile is readable now, only its name and username leave, and a stranger's is not readable at all.
    await admin.doc(`profiles/${bob}`).update({ displayName: 'Robert', handle: 'rob', email: 'bob@example.com' });
    assert.deepEqual(await service.readProfile(alice, bob), { displayName: 'Robert', handle: 'rob' });
    await friends.profile({ uid: stranger, name: 'Stranger' });
    await assert.rejects(service.readProfile(alice, stranger), /permission/iu);

    // Blocking: the card goes; the block entry keeps the name and username the person has at that moment (D12).
    await service.execute(alice, { action: 'block', otherUid: bob });
    await until(() => latest('1:cards:')?.length === 0 && latest('1:blocks:')?.length === 1, 'the block');
    const [block] = latest('1:blocks:')!;
    assert.deepEqual([block.uid, block.displayName, block.handle], [bob, 'Robert', 'rob']);
    assert.ok(Math.abs(block.at - Date.now()) < 5 * 60_000 && Number.isInteger(block.at));
    await assert.rejects(service.execute(alice, { action: 'unblock', otherUid: alice }), /INVALID_INPUT/u, 'the backend\'s refusal arrives as its code');

    // D19: another account ends every watch and tells each lease; nothing of the first account is delivered afterwards.
    uid = stranger; changed?.();
    assert.deepEqual(failed.map(event => event.code).sort(), ['INVALID_SESSION', 'INVALID_SESSION']);
    const delivered = rows.length;
    await admin.doc(`users/${alice}/friends/after-switch`).set({ state: 'incoming', displayName: 'Late', since: Timestamp.now() });
    await new Promise(resolve => setTimeout(resolve, 300));
    assert.equal(rows.length, delivered);
    await assert.rejects(service.watch('2:cards:', alice, 'friends'), /INVALID_SESSION/u);
  } finally {
    service.close(); await session.close(); await admin.terminate(); await deleteApp(app);
  }
  assert.equal(getApps().filter(candidate => candidate.name.startsWith('lingxi-teams-main-')).length, 0);
});
