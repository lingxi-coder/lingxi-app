import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore, Timestamp } from 'firebase-admin/firestore';
import { ApiError } from '../src/catalog.mjs';
import { buildPushMessage, createPushHandler, PushStore, SELF_PUSH_LIMIT, SELF_PUSH_WINDOW_MS } from '../src/push.mjs';

const route = '/v1/push/self/messages';
const hostId = 'AbCdEfGhIjKlMnOpQrSt_-', sessionId = '0f8fad5b-d9cb-469f-a165-70867728950e';
const body = (extra = {}) => ({ kind: 'permission', host_id: hostId, session_id: sessionId, ...extra });
const [a, b, c, x] = [randomUUID(), randomUUID(), randomUUID(), randomUUID()];
const phone = (id, extra = {}) => ({ id, uid: 'user-a', revision: 1, registrationHash: id, fcmInstallationId: `fid-${id}`,
  platform: 'android', appId: 'com.lingxi.code', locale: 'en-US', expiresAt: Timestamp.fromMillis(Date.now() + 86_400_000), ...extra });

// Fakes in the style of push.test.mjs. `devices` is what active() returns, `current` what each device
// document holds when it is re-read before sending. Pass a real PushStore as `store` to use its quota.
function backend(devices, { current = devices, quota = async () => {}, send = async () => {}, store: real } = {}) {
  const messages = [], quotas = [], invalidated = [];
  const store = real ?? { takeSelfQuota: async (uid) => { quotas.push(uid); await quota(uid); } };
  store.active = async (uid) => devices.filter((d) => d.uid === uid);
  store.device = (id) => ({ get: async () => ({ data: () => current.find((d) => d.id === id) }) });
  store.invalidate = async (d) => { invalidated.push(d.id); };
  const messaging = { send: async (message) => { await send(message); messages.push(message); } };
  const handler = createPushHandler({ store, messaging, verifyToken: async (token) => {
    if (token === 'bad') throw Error('invalid');
    return { uid: token === 'other' ? 'user-b' : 'user-a' };
  } });
  const post = (payload, token = 'user', path = route, method = 'POST') => handler(new Request(`https://test${path}`, { method,
    headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: typeof payload === 'string' || payload === undefined ? payload : JSON.stringify(payload) }));
  return { messages, quotas, invalidated, post };
}
// A Firestore stand-in with just what the quota needs. Transactions are serialized, as Firestore's are.
function memoryDb() {
  const docs = new Map();
  let tail = Promise.resolve();
  return { docs, doc: (path) => ({ path }), runTransaction: (fn) => {
    const run = tail.then(() => fn({ get: async (ref) => ({ data: () => docs.get(ref.path) }), set: (ref, value) => { docs.set(ref.path, value); } }));
    tail = run.catch(() => {});
    return run;
  } };
}

test('self push HTTP: authentication, routing and strict validation that never spends quota', async () => {
  const { post, messages, quotas } = backend([phone(a)]);
  assert.equal((await post(body(), null)).status, 401);
  assert.equal((await post(body(), 'bad')).status, 401);
  assert.equal((await post(undefined, 'user', route, 'GET')).status, 404);
  assert.equal((await post(body(), 'user', route, 'PUT')).status, 404);
  assert.equal((await post(body(), 'user', '/v1/push/self/other')).status, 404);
  const invalid = [{}, body({ kind: 'done' }), body({ kind: undefined }), body({ host_id: undefined }), body({ session_id: undefined }),
    body({ host_id: hostId.slice(1) }), body({ host_id: `${hostId.slice(1)}+` }), body({ host_id: `${hostId}A` }),
    body({ session_id: 'pending-new:1' }), body({ session_id: '00000000-0000-0000-0000-000000000000' }), body({ session_id: `${sessionId}0` }),
    body({ session_id: '0f8fad5b-d9cb-969f-a165-70867728950e' }), body({ session_id: '0f8fad5b-d9cb-069f-a165-70867728950e' }),
    body({ session_id: '0f8fad5b-d9cb-469f-c165-70867728950e' }), body({ session_id: '0f8fad5b-d9cb-469f-0165-70867728950e' }),
    body({ exclude_installation_ids: Array.from({ length: 17 }, () => randomUUID()) }), body({ exclude_installation_ids: ['not-a-uuid'] }),
    body({ exclude_installation_ids: a }), body({ title: 'text from the caller' }), body({ prompt: 'rm -rf /' }), body({ uid: 'user-b' }), null, []];
  for (const input of [...invalid, '"x"']) {
    const response = await post(input);
    assert.equal(response.status, 400, JSON.stringify(input));
    assert.deepEqual(await response.json(), { error: { code: 'INVALID_INPUT' } });
  }
  assert.equal((await post('{')).status, 400);
  assert.equal((await post('x'.repeat(16385))).status, 413);
  assert.deepEqual([messages.length, quotas.length], [0, 0], 'rejected requests send nothing and cost no quota');
  assert.equal((await post(body({ exclude_installation_ids: Array.from({ length: 16 }, () => randomUUID()) }))).status, 200, '16 exclusions are allowed');
  // The desktop's session ids: any RFC 9562 version 1-8, either case.
  for (const session_id of [sessionId, sessionId.toUpperCase(), '01939c4e-7b1a-7c2d-8e3f-4a5b6c7d8e9f', '6ba7b810-9dad-11d1-80b4-00c04fd430c8']) {
    assert.equal((await post(body({ session_id }))).status, 200, session_id);
  }
  const ok = await post(body());
  assert.equal(ok.status, 200);
  assert.equal(ok.headers.get('Cache-Control'), 'no-store');
});

test('self push goes to the caller\'s own installations minus the excluded ones', async () => {
  const { post, messages, quotas } = backend([phone(a), phone(b), phone(c), phone(x, { uid: 'user-b' })]);
  const run = async (exclude, token = 'user') => {
    messages.length = 0;
    const response = await post(body({ exclude_installation_ids: exclude }), token);
    assert.equal(response.status, 200);
    return response.json();
  };
  assert.deepEqual(await run([b]), { sent: 2, skipped: 1, failed: 0 });
  assert.deepEqual(messages.map((m) => m.fid), [`fid-${a}`, `fid-${c}`]);
  assert.deepEqual(await run([b.toUpperCase()]), { sent: 2, skipped: 1, failed: 0 }, 'ids are matched case-insensitively');
  assert.deepEqual(await run([x, randomUUID()]), { sent: 3, skipped: 0, failed: 0 }, 'other users\' and unknown ids exclude nothing');
  assert.deepEqual(await run([]), { sent: 3, skipped: 0, failed: 0 });
  assert.deepEqual(await run([], 'other'), { sent: 1, skipped: 0, failed: 0 });
  assert.deepEqual(messages.map((m) => m.fid), [`fid-${x}`], 'the uid comes from the verified token');
  quotas.length = 0;
  assert.deepEqual(await run([a, b, c]), { sent: 0, skipped: 3, failed: 0 });
  assert.deepEqual(quotas, [], 'nothing to send, so no quota is spent');
  const empty = backend([]);
  assert.deepEqual(await (await empty.post(body())).json(), { sent: 0, skipped: 0, failed: 0 });
  assert.deepEqual(empty.quotas, []);
});

test('self push content is generic, localized per installation and carries no request text', async () => {
  const copy = { permission: ['灵犀需要你的许可', 'Lingxi needs your permission'], question: ['灵犀正在等待你', 'Lingxi is waiting for you'],
    idle: ['灵犀正在等待你', 'Lingxi is waiting for you'], task: ['灵犀已完成', 'Lingxi finished'] };
  // [installation locale, index into the [zh, en] pairs above]
  const locales = [['zh-CN', 0], ['zh_TW', 0], ['zh-Hans-CN', 0], ['zh', 0], ['ZH-cn', 0], ['en-US', 1], ['en_GB', 1], ['ja-JP', 1], ['zhx', 1], ['', 1], [undefined, 1]];
  for (const kind of Object.keys(copy)) {
    const { post, messages } = backend(locales.map(([locale], i) => phone(randomUUID(), { locale, fcmInstallationId: `fid-${i}` })));
    assert.equal((await post(body({ kind }))).status, 200);
    assert.equal(messages.length, locales.length);
    messages.forEach((message, i) => {
      const language = locales[i][1], { title, body: text } = message.data;
      assert.equal(title, copy[kind][language], `${kind} ${locales[i][0]}`);
      assert.equal(/[一-鿿]/.test(text), language === 0, `${kind} body language for ${locales[i][0]}`);
      assert.deepEqual(message.apns.payload.aps.alert, { title, body: text }, 'the same generic text on APNs');
      assert.ok(!`${title}${text}`.includes(hostId) && !`${title}${text}`.includes(sessionId), 'only fixed copy, no ids and no request text');
      assert.deepEqual(Object.keys(message.data).sort(), ['body', 'expiresAt', 'host_id', 'kind', 'recipientUid', 'route', 'session_id', 'title']);
      assert.equal(message.notification, undefined, 'data-only on Android, as for every push');
    });
  }
  const mixed = backend([phone(a, { locale: 'zh-CN' }), phone(b, { locale: 'en-US' })]);
  await mixed.post(body({ kind: 'task' }));
  assert.deepEqual(mixed.messages.map((m) => m.data.title), ['灵犀已完成', 'Lingxi finished'], 'one request, one language per device');
});

test('self push route and collapse key reach FCM data, Android collapseKey and APNs apns-collapse-id', async () => {
  const { post, messages } = backend([phone(a, { platform: 'android' }), phone(b, { platform: 'ios', appId: 'com.lingxi.code.full' })]);
  for (const kind of ['permission', 'question', 'idle', 'task']) {
    messages.length = 0;
    await post(body({ kind }));
    const key = `rc:${sessionId}:${kind}`;
    assert.ok(Buffer.byteLength(key) <= 64, 'APNs rejects collapse ids over 64 bytes');
    assert.equal(messages.length, 2);
    for (const message of messages) {
      assert.deepEqual({ route: message.data.route, kind: message.data.kind, host_id: message.data.host_id, session_id: message.data.session_id },
        { route: `remote/${hostId}/${sessionId}`, kind, host_id: hostId, session_id: sessionId });
      assert.equal(message.android.collapseKey, key);
      assert.equal(message.apns.headers['apns-collapse-id'], key);
      assert.equal(message.android.ttl, 3_600_000, 'default one hour lifetime');
      assert.equal(message.apns.headers['apns-topic'], message.android.restrictedPackageName);
    }
  }
  const [android, ios] = messages;
  assert.equal(android.android.restrictedPackageName, 'com.lingxi.code');
  assert.equal(ios.android.restrictedPackageName, 'com.lingxi.code.full');
});

test('buildPushMessage output is unchanged without the new options, and extras cannot override the base keys', () => {
  const device = { uid: 'user-a', fcmInstallationId: 'token', appId: 'com.lingxi.code', expiresAt: Timestamp.fromMillis(50_000) };
  const legacy = { fid: 'token', data: { recipientUid: 'user-a', expiresAt: '50000', title: 'Title', body: 'Body' },
    android: { priority: 'high', ttl: 40_000, restrictedPackageName: 'com.lingxi.code' },
    apns: { headers: { 'apns-expiration': '50', 'apns-push-type': 'alert', 'apns-priority': '10', 'apns-topic': 'com.lingxi.code' },
      payload: { aps: { alert: { title: 'Title', body: 'Body' }, sound: 'default' } } } };
  assert.deepEqual(buildPushMessage(device, { title: 'Title', body: 'Body', ttlSeconds: 3600 }, 10_000), legacy);
  assert.deepEqual(buildPushMessage(device, { title: 'Title', body: 'Body', ttlSeconds: 3600, route: '', collapseKey: '' }, 10_000), legacy);
  const extended = buildPushMessage(device, { title: 'Title', body: 'Body', ttlSeconds: 3600, route: 'remote/h/s', collapseKey: 'rc:s:idle',
    data: { kind: 'idle', title: 'spoofed', recipientUid: 'user-b', expiresAt: '1' } }, 10_000);
  assert.deepEqual(extended.data, { kind: 'idle', recipientUid: 'user-a', expiresAt: '50000', title: 'Title', body: 'Body', route: 'remote/h/s' });
  assert.equal(extended.android.collapseKey, 'rc:s:idle');
  assert.equal(extended.apns.headers['apns-collapse-id'], 'rc:s:idle');
  const stripped = structuredClone(extended);
  delete stripped.data.route; delete stripped.data.kind; delete stripped.android.collapseKey; delete stripped.apns.headers['apns-collapse-id'];
  assert.deepEqual(stripped, legacy, 'nothing else changes');
});

test('self push counts send failures and skipped devices; only definite registration failures invalidate', async () => {
  const devices = ['gone', 'flaky', 'good', 'rebound'].map((id) => phone(id));
  const { post, invalidated, messages } = backend(devices, {
    current: devices.map((d) => (d.id === 'rebound' ? { ...d, uid: 'user-b' } : d)),
    send: async (message) => {
      const code = { 'fid-gone': 'messaging/registration-token-not-registered', 'fid-flaky': 'messaging/server-unavailable' }[message.fid];
      if (code) throw Object.assign(Error(), { code });
    } });
  assert.deepEqual(await (await post(body())).json(), { sent: 1, skipped: 1, failed: 2 });
  assert.deepEqual(invalidated, ['gone']);
  assert.deepEqual(messages.map((m) => m.fid), ['fid-good']);
});

test('self push rate limit: 30 per user per 10 minutes, sliding, spent only by requests that send', async () => {
  let now = 1_800_000_000_000;
  const db = memoryDb(), store = new PushStore(db, () => now);
  const { post, messages } = backend([phone(a), phone(x, { uid: 'user-b' })], { store });
  const status = async (token = 'user') => (await post(body(), token)).status;
  for (let i = 0; i < SELF_PUSH_LIMIT; i++) assert.equal(await status(), 200, `request ${i + 1}`);
  const refused = await post(body());
  assert.equal(refused.status, 429);
  assert.deepEqual(await refused.json(), { error: { code: 'RATE_LIMITED' } });
  assert.equal(messages.length, SELF_PUSH_LIMIT, 'a refused request sends nothing');
  assert.equal([...db.docs.values()][0].sentAt.length, SELF_PUSH_LIMIT, 'a refused request is not recorded');
  assert.ok(![...db.docs.keys()].some((path) => path.includes('user-a')), 'the document id does not contain the uid');
  assert.equal(await status('other'), 200, 'the limit is per user');
  now += SELF_PUSH_WINDOW_MS - 1;
  assert.equal(await status(), 429, 'still inside the window');
  now += 1;
  assert.equal(await status(), 200, 'the whole window has passed');
  // Sliding, not fixed: 15 now and 15 five minutes later free exactly 15 ten minutes after the first batch.
  const fresh = backend([phone(a)], { store: new PushStore(memoryDb(), () => now) });
  const burst = async (n) => { for (let i = 0; i < n; i++) assert.equal((await fresh.post(body())).status, 200); };
  await burst(15); now += SELF_PUSH_WINDOW_MS / 2; await burst(15);
  assert.equal((await fresh.post(body())).status, 429);
  now += SELF_PUSH_WINDOW_MS / 2; await burst(15);
  assert.equal((await fresh.post(body())).status, 429);
  // Requests with nothing to send never reach the limiter.
  const nobody = backend([], { store: new PushStore(memoryDb(), () => now) });
  for (let i = 0; i < SELF_PUSH_LIMIT + 5; i++) assert.equal((await nobody.post(body())).status, 200);
  // The quota is taken before any FCM call, so provider failures still count against it.
  const failing = backend([phone(a)], { store: new PushStore(memoryDb(), () => now),
    send: async () => { throw Object.assign(Error(), { code: 'messaging/server-unavailable' }); } });
  for (let i = 0; i < SELF_PUSH_LIMIT; i++) assert.deepEqual(await (await failing.post(body())).json(), { sent: 0, skipped: 0, failed: 1 });
  assert.equal((await failing.post(body())).status, 429);
});

test('self push reports a refusal from the quota as 429 and sends nothing', async () => {
  const { post, messages } = backend([phone(a)], { quota: async () => { throw new ApiError(429, 'RATE_LIMITED'); } });
  const response = await post(body());
  assert.equal(response.status, 429);
  assert.deepEqual(await response.json(), { error: { code: 'RATE_LIMITED' } });
  assert.deepEqual(messages, []);
});

test('Firestore: the self push quota is atomic under concurrency and its documents are pruned', { skip: !process.env.FIRESTORE_EMULATOR_HOST }, async () => {
  const app = initializeApp({ projectId: 'demo-lingxi-backend' }, randomUUID());
  const db = getFirestore(app, `push-${randomUUID()}`);
  let now = 1_800_000_000_000;
  const store = new PushStore(db, () => now), uid = `user-${randomUUID()}`, other = `user-${randomUUID()}`;
  try {
    // Race for the last 5 slots: a non-transactional read-then-write would let all 10 through.
    for (let i = 0; i < SELF_PUSH_LIMIT - 5; i++) await store.takeSelfQuota(uid);
    const attempts = await Promise.allSettled(Array.from({ length: 10 }, () => store.takeSelfQuota(uid)));
    assert.equal(attempts.filter((r) => r.status === 'fulfilled').length, 5);
    assert.ok(attempts.filter((r) => r.status === 'rejected').every((r) => r.reason.status === 429 && r.reason.code === 'RATE_LIMITED'));
    assert.equal((await store.limit(uid).get()).data().sentAt.length, SELF_PUSH_LIMIT);
    await store.takeSelfQuota(other);
    now += SELF_PUSH_WINDOW_MS - 1;
    await assert.rejects(store.takeSelfQuota(uid), { status: 429 });
    now += 1;
    await store.takeSelfQuota(uid);
    assert.equal((await store.limit(uid).get()).data().sentAt.length, 1, 'only the current window is kept');
    await store.prune();
    assert.equal((await store.limit(uid).get()).exists, true, 'a live window survives pruning');
    assert.equal((await store.limit(other).get()).exists, false, 'a closed window is pruned');
    now += SELF_PUSH_WINDOW_MS;
    await store.prune();
    assert.equal((await store.limit(uid).get()).exists, false);
  } finally { await db.terminate(); await deleteApp(app); }
});
