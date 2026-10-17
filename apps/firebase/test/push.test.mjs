import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore, Timestamp } from 'firebase-admin/firestore';
import { buildPushMessage, createPushHandler, PushStore, REGISTRATION_TTL_MS, sendUserPush } from '../src/push.mjs';

const input = (revision = 1, extra = {}) => ({ installationSecret: 'a'.repeat(64), revision,
  fcmInstallationId: 'caaaaaaaaaaaaaaaaaaaaa', platform: 'android', appId: 'com.lingxi.code.debug',
  appVersion: '0.1.0', osVersion: '16', deviceModel: 'Test Phone', locale: 'zh-CN', notificationsEnabled: true, ...extra });
const revoke = (revision) => ({ installationSecret: 'a'.repeat(64), revision });

test('HTTP: authentication, revocation capability, admin claim and strict identity validation', async () => {
  const mutations = [], initializations = [];
  const store = { mutate: async (...args) => { mutations.push(args); return { revision: args[1].revision }; },
    initialize: async (...args) => { initializations.push(args); return { initialized: true }; } };
  const handler = createPushHandler({ store, messaging: {}, verifyToken: async (token) => {
    if (token === 'bad') throw Error('invalid');
    return { uid: 'authenticated-user', email_verified: true, push_admin: token === 'admin' };
  } });
  const id = randomUUID(), path = `/v1/push/installations/${id}`;
  const request = (method, body, token, route = path) => handler(new Request(`https://test${route}`, {
    method, headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: typeof body === 'string' ? body : JSON.stringify(body),
  }));
  assert.equal((await request('PUT', input())).status, 401);
  assert.equal((await request('PUT', input(), 'bad')).status, 401);
  assert.equal((await request('POST', { installationSecret: input().installationSecret }, undefined, `${path}/initialize`)).status, 401);
  assert.equal((await request('POST', { installationSecret: input().installationSecret }, 'user', `${path}/initialize`)).status, 200);
  assert.equal(initializations.length, 1);
  assert.equal((await request('PUT', input(), 'user')).status, 200);
  assert.equal(mutations[0][2], 'authenticated-user', 'UID must come from verified authentication');
  assert.equal((await request('DELETE', revoke(2))).status, 200, 'revoke can finish after local Firebase sign-out');
  assert.equal(mutations[1][2], undefined);
  assert.equal((await request('POST', {}, 'user', '/v1/admin/push/messages')).status, 403);
  assert.equal((await request('PUT', '{', 'user')).status, 400);
  assert.equal((await request('PUT', 'x'.repeat(16385), 'user')).status, 413);
  assert.equal((await request('GET', undefined, 'user', '/v1/push/installations')).status, 404);
  const cors = await handler(new Request('https://test/health', { headers: { Origin: 'https://evil.example' } }));
  assert.equal(cors.status, 403);
});

test('message expiry is explicit on Android and APNs and bounded by the registration lease', () => {
  const device = { uid: 'user-a', fcmInstallationId: 'token', appId: 'com.lingxi.code', expiresAt: Timestamp.fromMillis(50_000) };
  const message = buildPushMessage(device, { title: 'Title', body: 'Body', ttlSeconds: 3600 }, 10_000);
  assert.equal(message.android.ttl, 40_000);
  assert.equal(message.apns.headers['apns-expiration'], '50');
  assert.equal(message.data.expiresAt, '50000');
  assert.equal(message.data.recipientUid, 'user-a');
  assert.equal(message.fid, 'token');
  assert.equal(message.token, undefined, 'send through the current FID API');
  assert.equal(message.android.restrictedPackageName, 'com.lingxi.code');
  assert.equal(message.notification, undefined, 'Android must not display messages before account validation');
});

test('sending skips expired/rebound devices and only invalidates definite registration failures', async () => {
  const devices = ['invalid', 'payload-error', 'transient', 'good', 'rebound', 'expired'].map((id) => ({
    id, uid: 'user-a', revision: 1, registrationHash: id, fcmInstallationId: id, appId: 'com.lingxi.code', expiresAt: Timestamp.fromMillis(id === 'expired' ? 500 : 100_000),
  }));
  const invalidated = [], sent = [];
  const store = { active: async () => devices,
    device: (id) => ({ get: async () => ({ data: () => ({ ...devices.find((d) => d.id === id), ...(id === 'rebound' ? { uid: 'user-b' } : {}) }) }) }),
    invalidate: async (d) => invalidated.push(d.id) };
  const messaging = { send: async (message) => {
    const code = { invalid: 'messaging/registration-token-not-registered', 'payload-error': 'messaging/invalid-argument', transient: 'messaging/server-unavailable' }[message.fid];
    if (code) throw Object.assign(Error(), { code });
    sent.push(message.fid);
  } };
  assert.deepEqual(await sendUserPush(store, messaging, { uid: 'user-a', title: 'Title', body: 'Body' }, () => 1000), { sent: 1, failed: 3, invalidated: 1 });
  assert.deepEqual(sent, ['good']); assert.deepEqual(invalidated, ['invalid']);
  await assert.rejects(sendUserPush(store, messaging, { uid: 'user-a', title: 'Title', body: 'Body', ttlSeconds: 86401 }));
});

test('Firestore: binding ownership, FID rotation, reordered logout, retries, expiry and cleanup', { skip: !process.env.FIRESTORE_EMULATOR_HOST }, async () => {
  const app = initializeApp({ projectId: 'demo-lingxi-backend' }, randomUUID());
  const db = getFirestore(app, `push-${randomUUID()}`);
  let now = 1_800_000_000_000;
  const store = new PushStore(db, () => now), id = randomUUID();
  try {
    const unknownId = randomUUID();
    await store.mutate(unknownId, revoke(1));
    assert.equal((await store.device(unknownId).get()).exists, false, 'anonymous revocation cannot create documents');
    await assert.rejects(store.mutate(unknownId, input(), 'user-a'), { code: 'INSTALLATION_NOT_INITIALIZED' });
    await store.initialize(id, { installationSecret: input().installationSecret });
    await assert.rejects(store.mutate(id, input(1, { uid: 'spoofed' }), 'user-a'));
    await assert.rejects(store.mutate(id, input(1, { appId: 'other.application' }), 'user-a'));
    const first = await store.mutate(id, input(), 'user-a');
    assert.equal(Date.parse(first.expiresAt), now + REGISTRATION_TTL_MS);
    assert.deepEqual(await store.mutate(id, input(), 'user-a'), first, 'lost-response retry must be idempotent');
    await assert.rejects(store.mutate(id, input(), 'user-b'), { code: 'STALE_REGISTRATION' });
    await assert.rejects(store.mutate(id, input(2, { installationSecret: 'b'.repeat(64) }), 'user-a'), { status: 403 });
    const otherId = randomUUID();
    await store.initialize(otherId, { installationSecret: input().installationSecret });
    await assert.rejects(store.mutate(otherId, input(), 'user-b'), { code: 'REGISTRATION_ALREADY_BOUND' });
    const old = (await store.active('user-a'))[0];
    await store.mutate(id, input(2, { fcmInstallationId: 'dbbbbbbbbbbbbbbbbbbbbb' }), 'user-a');
    assert.equal((await store.claim(old.registrationHash).get()).exists, false);
    await store.invalidate(old);
    assert.equal((await store.active('user-a')).length, 1, 'a failed old send must preserve the refreshed FID');
    await store.mutate(id, input(3, { fcmInstallationId: 'dbbbbbbbbbbbbbbbbbbbbb' }), 'user-b');
    assert.equal((await store.active('user-a')).length, 0);
    assert.equal((await store.active('user-b')).length, 1);
    await assert.rejects(store.mutate(id, revoke(2)), { code: 'STALE_REGISTRATION' });
    await store.mutate(id, revoke(4));
    assert.equal((await store.active('user-b')).length, 0);
    assert.deepEqual(await store.mutate(id, revoke(4)), { revision: 4, expiresAt: new Date(now + REGISTRATION_TTL_MS).toISOString() });
    await assert.rejects(store.mutate(id, input(3), 'user-a'), { code: 'STALE_REGISTRATION' });
    await store.mutate(id, input(5), 'user-a');
    const active = (await store.active('user-a'))[0];
    now += REGISTRATION_TTL_MS;
    assert.equal((await store.active('user-a')).length, 0, 'expired devices must be excluded even before physical TTL deletion');
    await store.prune();
    assert.equal((await store.device(id).get()).exists, false);
    assert.equal((await store.claim(active.registrationHash).get()).exists, false);
    // Independent requests can arrive in either order; the higher revision wins.
    const concurrentId = randomUUID();
    await store.initialize(concurrentId, { installationSecret: input().installationSecret });
    await Promise.allSettled([store.mutate(concurrentId, input(6), 'user-a'), store.mutate(concurrentId, revoke(7))]);
    assert.equal((await store.device(concurrentId).get()).data().uid, null);
    assert.equal((await store.active('user-a')).length, 0);
    // Renewal extends the lease using server time, not any client timestamp.
    const freshId = randomUUID();
    await store.initialize(freshId, { installationSecret: input().installationSecret });
    await store.mutate(freshId, input(1), 'user-a');
    now += REGISTRATION_TTL_MS - 1;
    await store.mutate(freshId, input(2), 'user-a');
    now += 1;
    await store.prune();
    assert.equal((await store.active('user-a')).length, 1);
    // The old expired installation must not remove a registration claim which a new
    // installation has legitimately acquired after the old lease expired.
    now += REGISTRATION_TTL_MS;
    const takeoverId = randomUUID();
    await store.initialize(takeoverId, { installationSecret: input().installationSecret });
    await store.mutate(takeoverId, input(), 'user-b');
    const takeover = (await store.active('user-b'))[0];
    await store.prune();
    assert.equal((await store.claim(takeover.registrationHash).get()).data().installationId, takeoverId);
    await store.invalidate(takeover);
    assert.equal((await store.active('user-b')).length, 0);
    assert.equal((await store.claim(takeover.registrationHash).get()).exists, false);
  } finally { await db.terminate(); await deleteApp(app); }
});
