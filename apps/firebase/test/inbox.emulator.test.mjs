import assert from 'node:assert/strict';
import { test, before, after, beforeEach } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails, assertSucceeds } from '@firebase/rules-unit-testing';
import { doc, getDoc, getDocs, collection, setDoc } from 'firebase/firestore';
import { InboxStore } from '../src/inbox-store.mjs';
import { InboxDelivery } from '../src/inbox-delivery.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { InboxJobs } from '../src/inbox-jobs.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 90000 };
const projectId = 'demo-lingxi-teams-t0',
  tid = 'team_0001',
  cid = 'chat_0001',
  key = 'team:' + tid + ':' + cid;
const ref = { kind: 'team', sourceId: tid, conversationId: cid },
  team = 'teams/' + tid,
  thread = team + '/conversations/' + cid;
const read = (through, manualUnreadRevision = 0) => ({
  action: 'markRead',
  key,
  generation: 1,
  through,
  manualUnreadRevision
});
let db, app, env, store, now;
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  env = await initializeTestEnvironment({
    projectId,
    firestore: {
      host,
      port: Number(port),
      rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8')
    }
  });
  app = initializeApp({ projectId }, 'inbox-test');
  db = getFirestore(app);
});
beforeEach(async () => {
  if (!env) return;
  await env.clearFirestore();
  now = Date.now();
  store = new InboxStore(db, () => now);
  const batch = db.batch();
  batch.set(db.doc(team), { state: 'active', name: 'Team' });
  for (const uid of ['me', 'peer'])
    batch.set(db.doc(team + '/members/' + uid), { generation: 1, joinedAt: new Date(0) });
  batch.set(db.doc(thread), {
    kind: 'dm',
    title: 'Chat',
    memberUids: ['me', 'peer'],
    participants: ['user:me', 'user:peer'],
    lastSeq: 0,
    lastChangeSeq: 0
  });
  await batch.commit();
});
after(async () => {
  await env?.cleanup();
  await db?.terminate();
  if (app) await deleteApp(app);
});
async function seed(n, foldInitial = true, fields = () => ({}), target = ref) {
  const thread = team + '/conversations/' + target.conversationId;
  const batch = db.batch();
  for (let i = 1; i <= n; i++) {
    const id = 'message_' + String(i).padStart(4, '0'),
      change = 'change_' + String(i).padStart(4, '0');
    batch.set(db.doc(thread + '/changes/' + change), {
      operation: 'create',
      seq: i,
      messageId: id,
      actorUid: 'peer',
      generation: 1
    });
    batch.set(db.doc(thread + '/messages/' + id), {
      seq: i,
      changeId: change,
      kind: 'text',
      author: { uid: 'peer', kind: 'user' },
      createdAt: new Date(now),
      text: 'synthetic',
      ...fields(i)
    });
  }
  batch.update(db.doc(thread), {
    lastSeq: n,
    lastChangeSeq: n,
    lastMessage: { at: new Date(now), preview: 'synthetic' }
  });
  await batch.commit();
  if (foldInitial) while (await store.fold('me', target)) {}
}
const get = async () => (await store.execute('me', { action: 'get', key })).item;
test('ordered fold, duplicates, 99+ partial reads, frozen manual revision, and legacy healing', options, async () => {
  await seed(130);
  assert.equal((await get()).unreadCount, 100);
  await store.fold('me', ref);
  assert.equal((await get()).sourceRevision, 130);
  await store.execute('me', read(20));
  assert.equal((await get()).unreadCount, 100);
  const op = { action: 'markUnread', expectedManualUnreadRevision: 0, key, generation: 1, operationId: 'read_later' };
  await store.execute('me', op);
  await store.execute('me', {
    action: 'markAllRead',
    entries: [{ key, generation: 1, through: 45, manualUnreadRevision: 0 }]
  });
  assert.equal((await get()).manualUnread, true);
  assert.equal((await get()).unreadCount, 85);
  await store.execute('me', read(50, 1));
  await store.execute('me', op);
  assert.equal((await get()).manualUnread, false);
  assert.equal((await db.doc(thread + '/reads/me').get()).data().lastReadSeq, 50);
  await db.doc(thread + '/reads/me').set({ generation: 1, lastReadSeq: 125 });
  await store.fold('me', ref);
  assert.equal((await get()).unreadCount, 5);
  await Promise.all([store.execute('me', read(126, 1)), store.execute('me', read(130, 1))]);
  assert.equal((await get()).readThrough, 130);
  assert.equal((await get()).unreadCount, 0);
  const badges = await db.collection('users/me/inboxBadges').get();
  assert.equal(
    badges.docs.reduce((s, d) => s + d.data().count, 0),
    0
  );
});
test('own rules, revoked membership, DM double opt-in and generation reset', options, async () => {
  await seed(1);
  await db.doc(thread + '/reads/peer').set({ lastReadSeq: 1, generation: 1 });
  const receipt = () => store.execute('me', { action: 'peerReceipt', key, generation: 1 });
  assert.deepEqual(await receipt(), { readThrough: null });
  await store.execute('me', { action: 'accountPreferences', dmReceipts: true });
  assert.deepEqual(await receipt(), { readThrough: null });
  await store.execute('peer', { action: 'accountPreferences', dmReceipts: true });
  assert.deepEqual(await receipt(), { readThrough: 1 });
  const me = env.authenticatedContext('me').firestore(),
    other = env.authenticatedContext('peer').firestore();
  await assertSucceeds(getDoc(doc(me, 'users/me/inbox/' + key)));
  await assertFails(getDoc(doc(other, 'users/me/inbox/' + key)));
  await assertFails(getDocs(collection(me, 'users/me/inbox')));
  await assertFails(setDoc(doc(me, 'users/me/inbox/' + key), {}));
  await assertFails(getDoc(doc(me, thread + '/reads/peer')));
  await db.doc(team + '/members/me').delete();
  await assertFails(getDoc(doc(me, 'users/me/inbox/' + key)));
  assert.deepEqual((await store.execute('me', { action: 'list' })).items, []);
  await store.fold('me', ref);
  assert.equal((await db.doc('users/me/inbox/' + key).get()).exists, false);
  await db.doc(team + '/members/me').set({ generation: 2, joinedAt: new Date(now + 10) });
  await store.fold('me', ref);
  await assert.rejects(store.execute('me', read(1)), { code: 'STALE_MEMBERSHIP' });
  assert.equal((await get()).unreadCount, 0);
});
test('Assistant opt-in, baseline, stale publishing, source secrecy and highwater persistence', options, async () => {
  const sourceId = 'host',
    sourceSecret = 's'.repeat(32),
    conversationId = 'session';
  const bind = { action: 'assistantSource', sourceId, sourceSecret, enabled: true };
  const publish = {
    action: 'assistantPublish',
    sourceId,
    sourceSecret,
    conversationId,
    generation: 1,
    baseline: 9,
    sourceCursor: 10,
    sourceRevision: 1,
    lastActivityAt: now,
    state: 'completed',
    pendingActionCount: 0,
    previewKind: 'taskResult'
  };
  await assert.rejects(store.execute('me', publish), { code: 'SOURCE_ACCESS_REQUIRED' });
  await store.execute('me', bind);
  await assert.rejects(store.execute('peer', publish), { code: 'SOURCE_ACCESS_REQUIRED' });
  await store.execute('me', publish);
  const k = 'assistant:host:session';
  await store.execute('me', {
    action: 'preferences',
    expectedPreferencesRevision: 0,
    key: k,
    generation: 1,
    patch: { pinOrder: 1 }
  });
  await store.execute('me', { ...read(10), key: k });
  await store.execute('me', {
    ...publish,
    sourceRevision: 2,
    sourceCursor: 14,
    state: 'waiting_for_approval',
    pendingActionCount: 1
  });
  await store.execute('me', publish);
  const row = (await store.execute('me', { action: 'get', key: k })).item;
  assert.equal(row.unreadCount, 4);
  assert.equal(row.pendingActionCount, 1);
  assert.equal(row.pinOrder, 1);
  await assert.rejects(store.execute('me', { ...bind, sourceSecret: 'x'.repeat(32) }), {
    code: 'SOURCE_ACCESS_REQUIRED'
  });
  await store.execute('me', { ...bind, enabled: false });
  assert.equal((await store.execute('me', { action: 'list' })).items.length, 0);
  const reenabled = await store.execute('me', bind);
  assert.equal(reenabled.generation, 3);
  await assert.rejects(store.execute('me', publish), { code: 'STALE_MEMBERSHIP' });
});
test(
  'stable tied-activity pages, filtered archive access, keyed notice delivery and 15-second fallback',
  options,
  async () => {
    await store.execute('me', { action: 'presence', deviceId: 'first', key, generation: 1, foreground: true });
    await seed(1);
    const uid = 'me',
      sent = [],
      installs = new Map(
        ['first', 'second'].map(id => [
          id,
          {
            id,
            uid,
            revision: 1,
            notificationsEnabled: true,
            fcmInstallationId: id,
            expiresAt: { toMillis: () => now + 60000 },
            appId: 'test.app'
          }
        ])
      );
    const scheduled = [],
      enqueue = async (payload, id, delay) => scheduled.push({ payload, id, delay });
    const pushStore = {
      active: async () => [...installs.values()],
      device: id => ({ get: async () => ({ data: () => installs.get(id) }) })
    };
    const delivery = new InboxDelivery({
      db,
      pushStore,
      messaging: { send: async m => sent.push(m) },
      enqueue,
      clock: () => now
    });
    const notice = (await db.collection('users/me/inboxNotices').get()).docs[0].id;
    await store.execute(uid, { action: 'presence', deviceId: 'first', key, generation: 1, foreground: true });
    await delivery.onNotice(uid, notice);
    const poll = async deviceId => (await store.execute(uid, { action: 'notices', deviceId })).items[0];
    assert.equal((await poll('first')).deliverAfter, now);
    assert.equal((await poll('second')).deliverAfter, now + 15000);
    const early = scheduled.find(job => job.payload.deviceId === 'second').payload;
    await delivery.deliver(early);
    assert.equal(sent.length, 0);

    assert.equal(scheduled.find(job => job.payload.deviceId === 'second').delay, 15);
    await delivery.deliver(scheduled.find(job => job.payload.deviceId === 'first').payload);
    assert.equal(sent.length, 0);
    await store.execute(uid, {
      action: 'markUnread',
      expectedManualUnreadRevision: 0,
      key,
      generation: 1,
      operationId: 'later'
    });
    const queued = scheduled.find(job => job.payload.deviceId === 'second').payload;
    now += 15000;
    await delivery.deliver(queued);
    await delivery.deliver(queued);
    assert.equal(sent.length, 1);
    assert.equal(sent[0].data.route, 'inbox/' + encodeURIComponent(key));
    assert.equal(sent[0].data.body, 'Conversation update');
    assert.equal(sent[0].data.recipientUid, uid);
    const first = scheduled[0].payload;
    await store.execute(uid, { action: 'presence', deviceId: 'first', key: null, foreground: false });
    await store.execute(uid, read(1, 1));
    await delivery.deliver(first);
    assert.equal(sent.length, 1);
    assert.equal((await store.execute(uid, { action: 'notices' })).items[0].deliverable, false);
    const bind = { action: 'assistantSource', sourceId: 'host', sourceSecret: 'a'.repeat(40), enabled: true };
    await store.execute(uid, bind);
    const shared = { sourceId: bind.sourceId, sourceSecret: bind.sourceSecret, generation: 1 };
    const publish = {
      action: 'assistantPublish',
      ...shared,
      baseline: 1,
      sourceCursor: 1,
      sourceRevision: 1,
      lastActivityAt: now,
      state: 'idle',
      pendingActionCount: 0,
      previewKind: null
    };
    for (let i = 0; i < 52; i++) await store.execute(uid, { ...publish, conversationId: 'c' + i });
    const page = await store.execute(uid, { action: 'list', category: 'assistant' });
    assert.equal(page.items.length, 50);
    const tail = await store.execute(uid, { action: 'list', category: 'assistant', after: page.next });
    assert.equal(tail.items.length, 2);
    assert.equal(new Set([...page.items, ...tail.items].map(r => r.key)).size, 52);
    const row = page.items[0];
    await store.execute(uid, {
      action: 'preferences',
      expectedPreferencesRevision: 0,
      key: row.key,
      generation: 1,
      patch: { archived: true }
    });
    assert.deepEqual(
      (await store.execute(uid, { action: 'list', archived: true })).items.map(v => v.key),
      [row.key]
    );
    const remove = {
      action: 'assistantRemove',
      ...shared,
      conversationId: row.ref.conversationId,
      sourceRevision: 1
    };
    await store.execute(uid, remove);
    await store.execute(uid, remove);
    await assert.rejects(store.execute(uid, { ...publish, conversationId: row.ref.conversationId }), {
      code: 'SOURCE_REMOVED'
    });
  }
);
test(
  'existing history and HTTP read share authoritatively confirmed cursor without clearing manual unread',
  options,
  async () => {
    await seed(55);
    const messages = new TeamMessagesStore(db, () => now),
      scope = { teamId: tid, conversationId: cid, generation: 1 };
    const page = await messages.execute({ uid: 'me' }, { ...scope, action: 'history' });
    assert.equal(page.messages.length, 50);
    assert.equal(page.messages[0].seq, 6);
    assert.equal(page.next, 6);
    const prev = await messages.execute({ uid: 'me' }, { ...scope, action: 'history', before: page.next });
    assert.deepEqual(
      prev.messages.map(m => m.seq),
      [1, 2, 3, 4, 5]
    );
    await store.execute('me', {
      action: 'markUnread',
      expectedManualUnreadRevision: 0,
      key,
      generation: 1,
      operationId: 'later'
    });
    await messages.execute({ uid: 'me' }, { ...scope, action: 'read', lastReadSeq: 50 });
    const row = await get();
    assert.equal(row.manualUnread, true);
    assert.equal(row.readThrough, 50);
    assert.equal(row.unreadCount, 5);
  }
);
test(
  'authoritative task waiting/approval outranks success, survives reading, and expires after its deadline',
  options,
  async () => {
    await seed(2);
    const botId = 'shared_bot1',
      taskId = 'pending_01',
      waitId = 'wait_0001',
      maintainer = { uid: 'peer', generation: 1 };
    await db.doc(thread).update({ participants: ['user:me', 'user:peer', 'shared_bot:' + botId] });
    await db.doc(team + '/sharedBots/' + botId).set({
      identity: {
        schemaVersion: 1,
        sharedBotId: botId,
        teamId: tid,
        maintainer,
        revision: 1,
        state: 'published',
        usePolicy: { mode: 'all' },
        activeDeploymentId: 'deploy_01'
      },
      grants: []
    });
    const task = {
      requester: { uid: 'me', generation: 1 },
      maintainer,
      sharedBotId: botId,
      taskId,
      conversationId: cid,
      messageSeq: 1,
      messageId: 'message_0001',
      changeId: 'change_0001',
      botRevision: 1,
      state: 'waiting',
      currentWaitId: waitId,
      currentWaitKind: 'tool_approval',
      deadline: now + 30000,
      createdAt: now
    };
    await db.doc(team + '/tasks/' + taskId).set(task);
    await db
      .doc(team + '/waits/' + waitId)
      .set({ state: 'pending', kind: 'tool_approval', messageId: 'message_0002', createdAt: now });
    const jobs = [],
      runner = new InboxJobs(store, null, async (payload, id, delay) => jobs.push({ payload, id, delay }));
    const job = { type: 'task', uid: 'me', ref: { ...ref, generation: 1 }, taskId };
    await runner.run(job);
    await runner.run(job);
    assert.ok(jobs.some(j => j.delay === 30));
    assert.equal((await get()).pendingActionCount, 1);
    assert.equal((await get()).state, 'waiting_for_approval');
    assert.ok(
      (await store.execute('me', { action: 'notices' })).items.some(
        n => n.kind === 'approval' && n.deliverable && n.deliverAfter === now + 6000
      )
    );
    await store.execute('me', { ...read(2), observedPendingActionRevision: (await get()).pendingActionRevision });
    assert.equal((await get()).pendingActionCount, 1);
    assert.equal(
      (await store.execute('me', { action: 'notices' })).items.find(n => n.kind === 'approval').deliverable,
      false
    );
    now += 30001;
    await runner.run(job);
    assert.equal((await get()).pendingActionCount, 0);
  }
);
test(
  'bounded 200-member local fanout updates rows and badge shards with no self or duplicate increment',
  options,
  async t => {
    const users = Array.from({ length: 200 }, (_, i) =>
      i === 0 ? 'peer' : 'recipient_' + String(i).padStart(3, '0')
    );
    const b = db.batch();
    for (const uid of users) b.set(db.doc(team + '/members/' + uid), { generation: 1, joinedAt: new Date(0) });
    b.update(db.doc(thread), { kind: 'group', memberUids: users, participants: users.map(uid => 'user:' + uid) });
    await b.commit();
    await seed(1, false);
    let cost;
    store.observe = data => {
      cost = data;
    };
    const completed = [],
      started = performance.now(),
      fold = store.fold.bind(store);
    store.fold = async (...args) => {
      const more = await fold(...args);
      completed.push(performance.now() - started);
      return more;
    };
    assert.equal(await store.fanout(ref), false);
    assert.equal(cost.normalizedEventWrites, 1);
    assert.equal(cost.recipientCount, 200);
    assert.equal(cost.rowWrites, 200);
    assert.equal(cost.signalWrites, 200);
    assert.equal(cost.badgeWrites, 199);
    assert.equal(cost.recountQueries, 0);
    assert.ok(cost.logicalCanonicalReads >= 800);
    t.diagnostic(JSON.stringify(cost));
    const sorted = completed.sort((a, b) => a - b);
    t.diagnostic(
      'Cold local fold after source commit, 200 recipients, excluding queue delay: p50=' +
        Math.round(sorted[99]) +
        'ms p95=' +
        Math.round(sorted[189]) +
        'ms max=' +
        Math.round(sorted[199]) +
        'ms'
    );
    assert.equal(completed.length, 200);
    const rows = await db.getAll(...users.map(u => db.doc('users/' + u + '/inbox/' + key)));
    assert.equal(rows[0].data().unreadCount, 0);
    assert.ok(rows.slice(1).every(d => d.data().unreadCount === 1));
    const badgeDocs = await db.collectionGroup('inboxBadges').get();
    assert.equal(
      badgeDocs.docs.reduce((sum, d) => sum + d.data().count, 0),
      199
    );
    await fold(users[1], ref);
    assert.equal((await rows[1].ref.get()).data().unreadCount, 1);
  }
);
test(
  'existing inactive conversations are bootstrapped once from per-user membership and changes',
  options,
  async () => {
    await seed(1);
    const jobs = new InboxJobs(store, null, async () => {});
    await jobs.bootstrap('peer');
    await jobs.bootstrap('peer');
    await jobs.run({ type: 'bootstrap', uid: 'peer' });
    await jobs.run({ type: 'bootstrap', uid: 'peer' });
    const me = await store.execute('peer', { action: 'list' });
    assert.equal(me.items[0].sourceCursor, 1);
    assert.equal(me.items[0].unreadCount, 0);
    assert.equal((await db.doc('users/peer/inboxJobs/bootstrap').get()).data().state, 'done');
  }
);
test('friend card uses incoming projection count and pair key with consent-gated receipts', options, async () => {
  await db.doc(team).update({ kind: 'pair' });
  await db.doc('users/me/friends/peer').set({ teamId: tid, state: 'friends', displayName: 'Friend', generation: 1 });
  await db.doc('users/peer/friends/me').set({ teamId: tid, state: 'friends', displayName: 'Me', generation: 1 });
  const human = { ...ref, conversationId: 'friend_chat' };
  const humanKey = 'team:' + tid + ':' + human.conversationId;
  const humanThread = team + '/conversations/' + human.conversationId;
  await db.doc(humanThread).set((await db.doc(thread).get()).data());
  await db.doc(thread).update({ kind: 'bot_dm' });
  await seed(2, true, undefined, human);
  const row = (await store.execute('me', { action: 'get', key: humanKey })).item;
  assert.equal(row.peerUid, 'peer');
  assert.equal(row.title, 'Friend');
  assert.equal(row.category, 'friend');
  assert.equal(row.key, humanKey);
  assert.equal(row.conversationKind, 'dm', 'rows say what kind of conversation they are');
  await seed(3);
  assert.equal((await get()).unreadCount, 3, 'the Bot DM has its own Inbox count');
  assert.equal((await db.doc('users/me/friends/peer').get()).data().unreadCount, 2);
  await store.execute('me', read(3));
  assert.equal((await db.doc('users/me/friends/peer').get()).data().unreadCount, 2);
  await store.fold('peer', human);
  assert.equal((await db.doc('users/peer/friends/me').get()).data().unreadCount, 0);
  assert.equal((await db.doc(humanThread + '/reads/peer').get()).exists, false);
  await store.execute('me', { ...read(1), key: humanKey });
  assert.equal((await db.doc('users/me/friends/peer').get()).data().unreadCount, 1);
});
test(
  'a delayed replay of a prior true read cannot cancel a newer same-cursor Assistant approval',
  options,
  async () => {
    const sourceId = 'writer',
      sourceSecret = 'p'.repeat(40),
      conversationId = 'session',
      k = 'assistant:writer:session';
    await store.execute('me', { action: 'assistantSource', sourceId, sourceSecret, enabled: true });
    const p = {
      action: 'assistantPublish',
      sourceId,
      sourceSecret,
      conversationId,
      generation: 1,
      baseline: 1,
      sourceCursor: 1,
      sourceRevision: 1,
      lastActivityAt: now,
      state: 'idle',
      pendingActionCount: 0,
      previewKind: null
    };
    await store.execute('me', p);
    const first = { ...read(1), key: k, operationId: 'actual-first' };
    await store.execute('me', first);
    await store.execute('me', { ...p, sourceRevision: 2, state: 'waiting_for_approval', pendingActionCount: 1 });
    await store.execute('me', first);
    const notices = async () => (await store.execute('me', { action: 'notices' })).items;
    assert.equal((await notices())[0].deliverable, true);
    await store.execute('me', { ...first, operationId: 'actual-next', observedPendingActionRevision: 1 });
    assert.equal((await notices())[0].deliverable, false);
  }
);
test('service inbox checks live service audience and entitlement generation even before purge', options, async () => {
  const sid = 's_service1',
    kind = 'service',
    k = 'service:' + sid + ':' + cid;
  await db
    .doc('services/' + sid)
    .set({ kind: 'service', state: 'active', name: 'Test service', audience: { mode: 'users', uids: ['me'] } });
  await db.doc('services/' + sid + '/members/me').set({ role: 'member', generation: 1 });
  await db
    .doc('services/' + sid + '/conversations/' + cid)
    .set({ memberUids: ['me'], lastSeq: 0, lastChangeSeq: 0, title: 'Service session' });
  await store.fold('me', { kind, sourceId: sid, conversationId: cid });
  assert.equal((await store.execute('me', { action: 'get', key: k })).item.category, 'service');
  await db.doc('services/' + sid).update({ audience: { mode: 'users', uids: [] } });
  assert.equal((await store.execute('me', { action: 'list', category: 'service' })).items.length, 0);
  await assert.rejects(store.execute('me', { action: 'get', key: k }), { code: 'SOURCE_ACCESS_REQUIRED' });
  await store.reconcileOwner('me', 'service', sid);
  assert.equal((await db.doc('users/me/inbox/' + k).get()).exists, false);
});
test(
  'pending alerts use action time on first turn and causal action version on old cursors or reopen',
  options,
  async () => {
    const sourceId = 'source',
      sourceSecret = 'h'.repeat(40),
      k = 'assistant:source:first';
    await store.execute('me', {
      action: 'assistantSource',
      sourceId,
      sourceSecret,
      enabled: true,
      consentRevision: 1
    });
    const start = {
      action: 'assistantPublish',
      sourceId,
      sourceSecret,
      conversationId: 'first',
      generation: 1,
      baseline: 0,
      sourceCursor: 0,
      sourceRevision: 1,
      lastActivityAt: 0,
      state: 'waiting_for_input',
      pendingActionCount: 1,
      previewKind: null,
      pendingActionAt: now,
      pendingActionRevision: 1
    };
    await store.execute('me', start);
    const history = async () => (await store.execute('me', { action: 'notices' })).items;
    const first = (await history())[0];
    assert.ok(first.deliverable);
    assert.equal(first.deliverAfter, now + 6000);
    assert.equal((await store.execute('me', { action: 'get', key: k })).item.lastActivityAt, 0);
    now += 100;
    await store.execute('me', {
      ...start,
      sourceRevision: 2,
      state: 'idle',
      pendingActionCount: 0,
      pendingActionAt: now,
      pendingActionRevision: 2,
      sourceCursor: 10
    });
    await store.execute('me', {
      ...read(10),
      key: k,
      operationId: 'frozen-before-open',
      observedPendingActionRevision: 2
    });
    now += 100;
    await store.execute('me', {
      ...start,
      sourceRevision: 3,
      sourceCursor: 10,
      pendingActionAt: now,
      pendingActionRevision: 3
    });
    const active = (await history()).find(n => n.kind === 'input' && n.id !== first.id);
    assert.ok(active.deliverable);
    assert.equal((await history()).find(n => n.id === first.id).deliverable, false);
    await store.execute('me', { ...read(1), key: k, operationId: 'tablet-late', observedPendingActionRevision: 3 });
    await store.execute('me', {
      ...read(10),
      key: k,
      operationId: 'before-open-same-cursor',
      observedPendingActionRevision: 2
    });
    await store.execute('me', { ...read(10), key: k, operationId: 'version-unknown' });
    assert.equal((await history()).find(n => n.id === active.id).deliverable, true);
    await store.execute('me', {
      ...read(10),
      key: k,
      operationId: 'observed-new-input',
      observedPendingActionRevision: 3
    });
    assert.equal((await history()).find(n => n.id === active.id).deliverable, false);
    assert.equal((await store.execute('me', { action: 'get', key: k })).item.pendingActionCount, 1);
  }
);
test(
  'indexed prefix search finds old titles beyond the initial page; versioned disable survives reordered requests',
  options,
  async () => {
    const src = { action: 'assistantSource', sourceId: 'reader', sourceSecret: 'h'.repeat(40) };
    const disabled = await store.execute('me', { ...src, enabled: false, consentRevision: 2 });
    assert.equal(disabled.enabled, false);
    assert.deepEqual(await store.execute('me', { ...src, enabled: true, consentRevision: 1 }), disabled);
    await assert.rejects(store.execute('me', { ...src, enabled: true }), { code: 'CONSENT_REVISION_REQUIRED' });
    await assert.rejects(store.execute('me', { ...src, enabled: true, consentRevision: 2 }), {
      code: 'CONSENT_REVISION_CONFLICT'
    });
    const { generation } = await store.execute('me', { ...src, enabled: true, consentRevision: 3 });
    const p = {
      action: 'assistantPublish',
      sourceId: src.sourceId,
      sourceSecret: src.sourceSecret,
      generation,
      baseline: 1,
      sourceRevision: 1,
      sourceCursor: 1,
      pendingActionCount: 0,
      state: 'idle',
      previewKind: null,
      lastActivityAt: now
    };
    await store.execute('me', { ...p, title: 'Ａlpha🙂', conversationId: 'old', lastActivityAt: 1 });
    for (let i = 0; i < 51; i++) await store.execute('me', { ...p, title: 'Recent ' + i, conversationId: 'new' + i });
    assert.ok(!(await store.execute('me', { action: 'list' })).items.some(i => i.ref.conversationId === 'old'));
    const page = await store.execute('me', { action: 'list', searchPrefix: ' ALPhA ' });
    assert.deepEqual(
      page.items.map(i => i.ref.conversationId),
      ['old']
    );
    const recent = await store.execute('me', { action: 'list', searchPrefix: 'recent' });
    assert.equal(recent.items.length, 50);
    assert.ok(recent.next.titleSort.startsWith('recent'));
    assert.equal(
      (await store.execute('me', { action: 'list', searchPrefix: 'recent', after: recent.next })).items.length,
      1
    );
  }
);
test('other-device notice polling and Cloud Tasks keep the same delay until actual read', options, async () => {
  await store.execute('me', { action: 'presence', deviceId: 'desktop', key, generation: 1, foreground: true });
  await seed(1);
  const list = async deviceId => (await store.execute('me', { action: 'notices', deviceId })).items[0];
  const original = await list('phone');
  assert.equal(original.deliverAfter, now + 15000);
  await store.execute('me', { action: 'presence', deviceId: 'desktop', key: null, foreground: false });
  assert.equal((await list('phone')).deliverAfter, original.deliverAfter);
  let sends = 0;
  const delivery = new InboxDelivery({
    db,
    pushStore: { active: async () => [], device: () => ({ get: async () => ({ data: () => null }) }) },
    messaging: { send: async () => sends++ },
    enqueue: async () => {},
    clock: () => now
  });
  now += 5000;
  await store.execute('me', { ...read(1), operationId: 'desktop-actually-read' });
  now += 10000;
  assert.equal((await list('phone')).deliverable, false);
  await delivery.deliver({ uid: 'me', id: original.id, deviceId: 'phone', revision: 1 });
  assert.equal(sends, 0);
  const src = { action: 'assistantSource', sourceId: 'solo', sourceSecret: 'x'.repeat(32), enabled: true };
  await store.execute('me', src);
  const pub = {
    action: 'assistantPublish',
    sourceId: 'solo',
    sourceSecret: src.sourceSecret,
    conversationId: 'chat',
    generation: 1,
    sourceRevision: 1,
    baseline: 0,
    sourceCursor: 0,
    pendingActionAt: now,
    pendingActionRevision: 1,
    lastActivityAt: 0,
    state: 'waiting_for_input',
    pendingActionCount: 1,
    previewKind: null
  };
  await store.execute('me', pub);
  const noForeground = await list('phone');
  assert.equal(noForeground.deliverAfter, now + 6000);
});
test('revocation cleanup paginates independently of source access checks', options, async () => {
  const queued = [],
    enqueue = async p => queued.push(p),
    paged = new InboxStore(db, () => now, enqueue);
  const p = {
    action: 'assistantSource',
    sourceId: 'purge',
    sourceSecret: 's'.repeat(40),
    enabled: true,
    consentRevision: 1
  };
  const { generation } = await paged.execute('me', p);
  const b = db.batch();
  for (let i = 0; i < 70; i++) {
    const conversationId = 'old' + String(i).padStart(2, '0'),
      ref = { kind: 'assistant', sourceId: 'purge', conversationId, generation };
    const key = 'assistant:purge:' + conversationId;
    b.set(db.doc('users/me/inbox/' + key), {
      key,
      ref,
      title: 'Old',
      category: 'assistant',
      archived: false,
      pinRank: 1,
      lastActivityAt: 1,
      sourceRevision: 1,
      readThrough: 0,
      sourceCursor: 0
    });
  }
  await b.commit();
  await paged.execute('me', { ...p, enabled: false, consentRevision: 2 });
  assert.ok(queued.some(j => j.type === 'reconcile' && j.after));
  assert.equal((await paged.execute('me', { action: 'list' })).items.length, 0);
  const runner = new InboxJobs(paged, null, enqueue);
  while (queued.length) await runner.run(queued.shift());
  assert.equal((await db.collection('users/me/inbox').get()).size, 0);
});
test('Messages search and read batches leave Agent conversations in Projects', options, async () => {
  await seed(1);
  const src = { sourceId: 'project-device', sourceSecret: 'p'.repeat(40) };
  const { generation } = await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
  await store.execute('me', {
    action: 'assistantPublish', ...src, generation, conversationId: 'project-chat', title: 'Chat project',
    baseline: 0, sourceRevision: 1, sourceCursor: 1, lastActivityAt: now,
    previewKind: 'reply', pendingActionCount: 0, state: 'completed'
  });
  const messages = await store.execute('me', { action: 'list', category: 'messages', searchPrefix: 'chat' });
  assert.deepEqual(messages.items.map(row => row.key), [key]);
  const projects = await store.execute('me', { action: 'list', category: 'assistant' });
  assert.deepEqual(projects.items.map(row => row.key), ['assistant:project-device:project-chat']);
  const start = { action: 'markAllReadSnapshot', category: 'messages', operationId: 'message-batch', requestedAt: now };
  await store.execute('me', start);
  await new InboxJobs(store, null, async () => {}).run({ type: 'readBatch', uid: 'me', id: start.operationId });
  const remaining = await store.execute('me', { action: 'list', filter: 'unread' });
  assert.deepEqual(remaining.items.map(row => row.key), ['assistant:project-device:project-chat']);
});

test(
  'global batch freezes every unread row at one server snapshot and preserves new/unseen marks',
  options,
  async () => {
    const src = { sourceId: 'batch', sourceSecret: 's'.repeat(40) };
    await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
    const base = {
      action: 'assistantPublish',
      ...src,
      generation: 1,
      baseline: 0,
      sourceRevision: 1,
      sourceCursor: 1,
      lastActivityAt: now,
      previewKind: 'reply',
      pendingActionCount: 0,
      state: 'completed'
    };
    for (let i = 0; i < 55; i++)
      await store.execute('me', { ...base, conversationId: 'item' + String(i).padStart(2, '0') });
    const start = { action: 'markAllReadSnapshot', operationId: 'frozen-online', requestedAt: now };
    assert.equal((await store.execute('me', start)).state, 'queued');
    await store.execute('me', { ...base, conversationId: 'newborn' });
    await store.execute('me', { ...base, conversationId: 'item53', sourceRevision: 2, sourceCursor: 4 });
    await store.execute('me', {
      action: 'markUnread',
      key: 'assistant:batch:item54',
      generation: 1,
      expectedManualUnreadRevision: 0,
      operationId: 'later'
    });
    const jobs = [],
      runner = new InboxJobs(store, null, async j => jobs.push(j));
    await runner.run({ type: 'readBatch', uid: 'me', id: start.operationId });
    while (jobs.length) await runner.run(jobs.shift());
    const done = await store.execute('me', { action: 'readBatchStatus', jobId: start.operationId });
    assert.equal(done.state, 'complete');
    assert.equal(done.processed, 55);
    const remaining = (await store.execute('me', { action: 'list', filter: 'unread' })).items;
    assert.deepEqual(remaining.map(r => [r.ref.conversationId, r.unreadCount, r.manualUnread]).sort(), [
      ['item53', 3, false],
      ['item54', 0, true],
      ['newborn', 1, false]
    ]);
    assert.deepEqual(await store.execute('me', start), done);
  }
);
test(
  'per-session opt-out fences stale publications and permits explicit same-revision restore; first failure alerts without a result',
  options,
  async () => {
    const src = { sourceId: 'privacy', sourceSecret: 's'.repeat(40) };
    await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
    const p = {
      action: 'assistantPublish',
      ...src,
      generation: 1,
      conversationId: 'private',
      baseline: 0,
      sourceRevision: 1,
      sourceCursor: 0,
      lastActivityAt: 0,
      failureAt: now,
      failureRevision: 1,
      previewKind: 'failure',
      pendingActionCount: 0,
      state: 'failed',
      conversationConsentRevision: 1
    };
    await store.execute('me', p);
    const k = 'assistant:privacy:private';
    assert.equal((await store.execute('me', { action: 'get', key: k })).item.unreadCount, 0);
    assert.ok(
      (await store.execute('me', { action: 'notices' })).items.some(
        n => n.kind === 'failure' && n.deliverable && n.lastActivityAt === now
      )
    );
    await store.execute('me', { ...read(0), key: k, operationId: 'older-failure', observedFailureRevision: 0 });
    assert.equal((await store.execute('me', { action: 'notices' })).items[0].deliverable, true);
    await store.execute('me', { ...read(0), key: k, operationId: 'observed-failure', observedFailureRevision: 1 });
    assert.equal((await store.execute('me', { action: 'notices' })).items[0].deliverable, false);
    assert.equal((await store.execute('me', { action: 'get', key: k })).item.state, 'failed');
    const remove = {
      action: 'assistantRemove',
      ...src,
      generation: 1,
      conversationId: p.conversationId,
      sourceRevision: 1,
      conversationConsentRevision: 2
    };
    await store.execute('me', remove);
    await assert.rejects(store.execute('me', p), { code: 'STALE_CONVERSATION_CONSENT' });
    await store.execute('me', { ...p, conversationConsentRevision: 3 });
    await assert.rejects(store.execute('me', remove), { code: 'STALE_CONVERSATION_CONSENT' });
    await store.execute('me', {
      ...p,
      sourceRevision: 2,
      state: 'idle',
      failureRevision: 2,
      conversationConsentRevision: 3
    });
    assert.ok(!(await store.execute('me', { action: 'notices' })).items.some(n => n.deliverable));
  }
);
test(
  'capped counts share one metadata-only source index and old preferences/manual/read retries are bounded',
  options,
  async () => {
    await seed(130);
    await store.fold('peer', ref);
    assert.equal((await db.collection(thread + '/inboxCountEvents').get()).size, 130);
    assert.equal((await db.collectionGroup('occurrences').get()).size, 0);
    const op = { action: 'markUnread', key, generation: 1, operationId: 'old', expectedManualUnreadRevision: 0 };
    await store.execute('me', op);
    await store.execute('me', read(10, 1));
    await store.execute('me', op);
    for (let n = 11; n <= 17; n++) await store.execute('me', { ...read(n, 2), operationId: 'read_' + n });
    assert.equal((await db.collection('users/me/inboxOperations').get()).size, 0);
    assert.equal((await get()).manualUnread, false);
    await store.execute('me', {
      action: 'preferences',
      key,
      generation: 1,
      expectedPreferencesRevision: 0,
      patch: { archived: true }
    });
    await assert.rejects(
      store.execute('me', {
        action: 'preferences',
        key,
        generation: 1,
        expectedPreferencesRevision: 0,
        patch: { archived: false }
      }),
      { code: 'PREFERENCE_REVISION_CONFLICT' }
    );
    assert.equal((await get()).archived, true);
  }
);
test(
  'sparse shared counts stay exact through recalls at the cap, own messages, bounded mentions and legacy reads',
  options,
  async () => {
    await seed(130, true, i => ({
      ...(i >= 125 ? { mentions: ['user:me'] } : {}),
      ...(i === 128 ? { author: { uid: 'me', kind: 'user' } } : {})
    }));
    assert.equal((await get()).unreadCount, 100);
    assert.equal((await get()).mentionCount, 5);
    const b = db.batch(),
      target = 'message_0130',
      change = 'recall_last';
    b.update(db.doc(thread + '/messages/' + target), { recalled: true, text: '', changeSeq: 131, changeId: change });
    b.set(db.doc(thread + '/changes/' + change), {
      operation: 'recall',
      seq: 131,
      messageId: target,
      actorUid: 'peer',
      generation: 1
    });
    b.update(db.doc(thread), { lastChangeSeq: 131, lastMessage: { at: new Date(now), preview: '' } });
    await b.commit();
    await store.fold('me', ref);
    assert.equal((await get()).unreadCount, 100);
    assert.equal((await get()).mentionCount, 4);
    await store.execute('me', read(33));
    assert.equal((await get()).unreadCount, 95);
    await store.execute('me', read(125));
    assert.equal((await get()).unreadCount, 3);
    assert.equal((await get()).mentionCount, 3);
    assert.equal((await db.collection(thread + '/inboxCountEvents').get()).size, 129);
    assert.deepEqual(Object.keys((await db.doc(thread + '/inboxCountEvents/message_0129').get()).data()).sort(), [
      'actor',
      'at',
      'mentions',
      'seq'
    ]);
  }
);
test(
  'notice click reauthorizes exact source and target; disabled session hides history across explicit restore',
  options,
  async () => {
    await seed(1);
    const teamEvent = (await store.execute('me', { action: 'notices' })).items[0];
    assert.equal((await store.execute('me', { action: 'notice', id: teamEvent.id })).item.messageId, 'message_0001');
    await db.doc(thread + '/messages/message_0001').update({ recalled: true, text: '' });
    assert.equal((await store.execute('me', { action: 'notice', id: teamEvent.id })).item.messageId, undefined);
    const src = { sourceId: 'history', sourceSecret: 'x'.repeat(40) };
    await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
    const p = {
      action: 'assistantPublish',
      ...src,
      generation: 1,
      conversationId: 'row',
      baseline: 0,
      sourceCursor: 1,
      sourceRevision: 1,
      lastActivityAt: now,
      conversationConsentRevision: 1,
      pendingActionCount: 0,
      previewKind: 'reply',
      state: 'completed'
    };
    await store.execute('me', p);
    const earlier = (await store.execute('me', { action: 'notices' })).items.find(n =>
      n.key.startsWith('assistant:')
    );
    await store.execute('me', {
      action: 'assistantRemove',
      ...src,
      generation: 1,
      conversationId: 'row',
      sourceRevision: 1,
      conversationConsentRevision: 2
    });
    await assert.rejects(store.execute('me', { action: 'notice', id: earlier.id }), { code: 'NOTICE_MISSING' });
    await store.execute('me', { ...p, conversationConsentRevision: 3 });
    await assert.rejects(store.execute('me', { action: 'notice', id: earlier.id }), { code: 'NOTICE_MISSING' });
    assert.ok(!(await store.execute('me', { action: 'notices' })).items.some(n => n.key === earlier.key));
  }
);
test(
  'notice retention evicts one old slot, ignores duplicate admissions and enforces expiry at read time',
  options,
  async () => {
    await seed(1);
    const account = db.doc('users/me'),
      rows = account.collection('inboxNotices');
    const fresh = (await rows.get()).docs[0];
    const old = 'f'.repeat(64),
      batch = db.batch(),
      ring = account.collection('inboxRetention');
    batch.set(ring.doc('head'), { next: 1000 });
    batch.set(ring.doc('0'), { id: old });
    batch.set(rows.doc(old), { ...fresh.data(), id: old });
    await batch.commit();
    const delivery = new InboxDelivery({
      db,
      pushStore: { active: async () => [] },
      messaging: {},
      enqueue: async () => {},
      clock: () => now
    });
    await delivery.onNotice('me', fresh.id);
    await delivery.onNotice('me', fresh.id);
    assert.equal((await ring.doc('head').get()).data().next, 1001);
    assert.equal((await rows.doc(old).get()).exists, false);
    now += 31 * 86400000;
    assert.equal((await store.execute('me', { action: 'notices' })).items.length, 0);
    await assert.rejects(store.execute('me', { action: 'notice', id: fresh.id }), { code: 'NOTICE_MISSING' });
  }
);

test(
  'privacy transitions preserve personal state and fence a frozen batch and delayed preferences',
  options,
  async () => {
    const src = { sourceId: 'privacy-source', sourceSecret: 'z'.repeat(40) },
      key = 'assistant:privacy-source:session';
    await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
    const base = {
      action: 'assistantPublish',
      ...src,
      conversationId: 'session',
      generation: 1,
      baseline: 0,
      sourceCursor: 3,
      sourceRevision: 1,
      lastActivityAt: now,
      state: 'completed',
      pendingActionCount: 0,
      previewKind: 'reply',
      title: 'Private session',
      conversationConsentRevision: 1
    };
    const row = async () => (await store.execute('me', { action: 'get', key })).item;
    await store.execute('me', base);
    await store.execute('me', {
      action: 'markUnread',
      key,
      generation: 1,
      expectedManualUnreadRevision: 0,
      operationId: 'first'
    });
    assert.equal((await row()).manualUnreadRevision, 1);
    const batch = { action: 'markAllReadSnapshot', operationId: 'frozen-before-removal', requestedAt: now };
    await store.execute('me', batch);
    const remove = {
      action: 'assistantRemove',
      ...src,
      conversationId: 'session',
      generation: 1,
      sourceRevision: 1,
      conversationConsentRevision: 2
    };
    await store.execute('me', remove);
    const watermark = db.doc('users/me/inboxSources/privacy-source/checkpoints/session');
    const first = (await watermark.get()).data();
    assert.equal(JSON.stringify(first).includes('Private session'), false);
    await store.execute('me', remove);
    assert.deepEqual((await watermark.get()).data().personal, first.personal);
    await store.execute('me', { ...base, conversationConsentRevision: 3 });
    const restored = await row();
    assert.equal(restored.manualUnread, true);
    assert.ok(restored.manualUnreadRevision > 1);
    assert.ok(restored.preferencesRevision > 0);
    await assert.rejects(
      store.execute('me', {
        action: 'preferences',
        key,
        generation: 1,
        expectedPreferencesRevision: 0,
        patch: { archived: true }
      }),
      { code: 'PREFERENCE_REVISION_CONFLICT' }
    );
    await store.execute('me', {
      action: 'markUnread',
      key,
      generation: 1,
      expectedManualUnreadRevision: restored.manualUnreadRevision,
      operationId: 'fresh'
    });
    const fresh = await row();
    await new InboxJobs(store, null, async () => {}).run({ type: 'readBatch', uid: 'me', id: batch.operationId });
    assert.equal((await store.execute('me', { action: 'readBatchStatus', jobId: batch.operationId })).processed, 1);
    assert.deepEqual(await row(), fresh);
    const prefs = { pinOrder: 7, archived: true, notificationMode: 'none' };
    await store.execute('me', {
      action: 'preferences',
      key,
      generation: 1,
      expectedPreferencesRevision: fresh.preferencesRevision,
      patch: prefs
    });
    const customized = await row();
    await store.execute('me', { ...remove, conversationConsentRevision: 4 });
    await store.execute('me', { ...base, conversationConsentRevision: 5 });
    const final = await row();
    for (const [name, value] of Object.entries(prefs)) assert.equal(final[name], value);
    assert.ok(final.preferencesRevision > customized.preferencesRevision);
    assert.ok(final.manualUnreadRevision > customized.manualUnreadRevision);
    await assert.rejects(
      store.execute('me', {
        action: 'preferences',
        key,
        generation: 1,
        expectedPreferencesRevision: customized.preferencesRevision,
        patch: { notificationMode: 'all' }
      }),
      { code: 'PREFERENCE_REVISION_CONFLICT' }
    );
    await store.execute('me', { ...base, conversationConsentRevision: 7 });
    await assert.rejects(store.execute('me', { ...remove, conversationConsentRevision: 6 }), {
      code: 'STALE_CONVERSATION_CONSENT'
    });
    assert.ok((await row()).preferencesRevision > final.preferencesRevision);
  }
);
test('archived muted rows remain discoverable via unread and attention without unarchiving', options, async () => {
  const src = { sourceId: 'archive-source', sourceSecret: 'z'.repeat(40) },
    key = 'assistant:archive-source:row';
  await store.execute('me', { action: 'assistantSource', ...src, enabled: true });
  await store.execute('me', {
    action: 'assistantPublish',
    ...src,
    conversationId: 'row',
    generation: 1,
    baseline: 0,
    sourceCursor: 2,
    sourceRevision: 1,
    lastActivityAt: now,
    state: 'waiting_for_input',
    pendingActionCount: 1,
    pendingActionRevision: 1,
    pendingActionAt: now,
    previewKind: 'reply',
    title: 'Archived chat'
  });
  await store.execute('me', {
    action: 'preferences',
    key,
    generation: 1,
    expectedPreferencesRevision: 0,
    patch: { archived: true, notificationMode: 'none' }
  });
  const listing = async p => (await store.execute('me', { action: 'list', ...p })).items;
  assert.deepEqual(await listing({ filter: 'all' }), []);
  assert.deepEqual(await listing({ filter: 'all', archived: false }), []);
  assert.equal((await listing({ filter: 'all', archived: true }))[0].key, key);
  for (const filter of ['unread', 'attention']) {
    for (const archive of [{}, { archived: false }, { archived: true }]) {
      const matches = await listing({ filter, category: 'assistant', ...archive });
      assert.equal(matches[0].key, key);
      assert.equal(matches[0].archived, true);
    }
    assert.equal((await listing({ filter, category: 'assistant', searchPrefix: 'archived' }))[0].key, key);
  }
  await store.execute('me', {
    action: 'markAllReadSnapshot',
    filter: 'unread',
    archived: false,
    operationId: 'archived-filter-click',
    requestedAt: now
  });
  await new InboxJobs(store, null, async () => {}).run({ type: 'readBatch', uid: 'me', id: 'archived-filter-click' });
  assert.deepEqual(await listing({ filter: 'unread' }), []);
  const row = (await listing({ filter: 'attention' }))[0];
  assert.equal(row.key, key);
  assert.equal(row.archived, true);
  assert.equal(row.notificationMode, 'none');
  assert.equal(row.pendingActionCount, 1);
});
