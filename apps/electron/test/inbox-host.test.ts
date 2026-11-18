import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { InboxItem, InboxPage } from '../../../packages/inbox/contracts.js';
import { InboxService, type DesktopInboxSession, type InboxOptions } from '../src/main/inbox/service.js';
import { bindInboxIpc } from '../src/main/inbox/bindIpc.js';
import { DELIVERY_RETENTION_MS, NotificationDeliveryLedger } from '../src/main/inbox/deliveryLedger.js';
import { NoticeDelivery } from '../src/main/inbox/notices.js';
import type { AccountSessionStore } from '../src/main/account.js';

const key = 'assistant:source:session';
function row(id = 'session', time = 100): DesktopInboxSession {
  return {
    ref: { sessionId: id, projectPath: '/private/path', mode: 'code', projectId: 'project' },
    row: {
      uuid: id,
      title: 'private auto-generated prompt',
      mode: 'code',
      path: '/private/path/' + id + '.jsonl',
      modified_rfc3339: 'unused',
      message_count: 99999,
      activity_json: JSON.stringify({
        sourceId: 'source',
        cursor: 12,
        baselineCursor: 10,
        lastMeaningfulAt: time,
        previewKind: 'reply',
        pendingActionCount: 1,
        failed: false,
        sourceRevision: 2,
      }),
    },
  };
}
function fixture(
  initial: string | null,
  rows: DesktopInboxSession[],
  dir: string,
  override: Partial<InboxOptions> = {},
) {
  let uid = initial,
    epoch = 0,
    secret: string | null = null;
  const listeners = new Set<() => void>(),
    calls: Record<string, unknown>[] = [];
  const vault: AccountSessionStore = {
    persistent: true,
    async read() {
      return secret;
    },
    async write(v) {
      secret = v;
    },
    async remove() {
      secret = null;
    },
  };
  const options: InboxOptions = {
    userData: dir,
    localSessions: async () => rows,
    sourceCredentials: vault,
    teams: { firestore: async () => ({ db: {}, sdk: {} }) as never },
    account: {
      snapshot: () => ({ user: uid ? { uid } : null }),
      sessionEpoch: () => epoch,
      getIdToken: async () => 'token-' + uid,
      onChanged(fn) {
        listeners.add(fn);
        return () => {
          listeners.delete(fn);
        };
      },
    },
    fetcher: async (_, init) => {
      assert.equal((init?.headers as Record<string, string>).Authorization, 'Bearer token-' + uid);
      const c = JSON.parse(String(init?.body));
      calls.push(c);
      if (c.action === 'assistantSource') return Response.json({ enabled: c.enabled, generation: c.enabled ? 1 : 2 });
      if (c.action === 'assistantPublish')
        return Response.json({ sourceRevision: c.sourceRevision, sourceCursor: c.sourceCursor });
      if (c.action === 'list' || c.action === 'notices') return Response.json({ items: [], next: null });
      return Response.json({ ok: true });
    },
    ...override,
  };
  const service = new InboxService(options);
  return {
    service,
    calls,
    setUid(next: string | null) {
      epoch++;
      uid = next;
      for (const fn of listeners) fn();
    },
  };
}

test('signed-out local reads survive restart, keep manual revision, count only semantic ordinals, preserve pending actions and exact open ref', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-local-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const rows = [row(), { ...row('legacy'), row: { ...row('legacy').row, activity_json: undefined } }];
  const f = fixture(null, rows, dir);
  const first = await f.service.request<InboxPage>(null, { action: 'list' });
  assert.equal(first.items.length, 1);
  assert.equal(first.items[0].unreadCount, 2);
  assert.equal(await f.service.badge(null), 1);
  assert.deepEqual(await f.service.localRef(key), rows[0].ref);
  await f.service.request(null, { action: 'markUnread', key, generation: 1, operationId: 'manual-1' });
  await f.service.request(null, { action: 'markUnread', key, generation: 1, operationId: 'manual-1' });
  const read = { action: 'markRead' as const, key, generation: 1, manualUnreadRevision: 0, through: 12 };
  await f.service.request(null, read);
  await assert.rejects(f.service.request(null, { ...read, through: 13 }), /INVALID_READ_CURSOR/);
  f.service.close();
  const g = fixture(null, rows, dir);
  t.after(() => g.service.close());
  const { item } = await g.service.request<{ item: InboxItem }>(null, { action: 'get', key });
  assert.deepEqual(
    [item.readThrough, item.unreadCount, item.manualUnread, item.manualUnreadRevision, item.pendingActionCount],
    [12, 0, true, 1, 1],
  );
  await g.service.request(null, { action: 'markRead', key, generation: 1, through: 11, manualUnreadRevision: 1 });
  assert.equal((await g.service.request<InboxPage>(null, { action: 'list', filter: 'unread' })).items.length, 0);
  assert.equal((await g.service.request<InboxPage>(null, { action: 'list', filter: 'attention' })).items.length, 1);
  assert.deepEqual([...f.calls, ...g.calls], []);
});

test('local pages preserve equal-time ordering and frozen batch scope after a new event', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-page-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const rows = Array.from({ length: 101 }, (_, i) => row('s' + String(i).padStart(3, '0'), 100));
  const f = fixture(null, rows, dir);
  t.after(() => f.service.close());
  const first = await f.service.request<InboxPage>(null, { action: 'list' });
  const next = await f.service.request<InboxPage>(null, { action: 'list', after: first.next! });
  assert.equal(first.items.length, 50);
  assert.equal(next.items[0].ref.conversationId, 's050');
  const entries = first.items.map((item) => ({
    key: item.key,
    through: item.sourceCursor,
    generation: 1,
    manualUnreadRevision: 0,
  }));
  const activity = JSON.parse(rows[0].row.activity_json!);
  activity.cursor++;
  activity.sourceRevision++;
  rows[0].row.activity_json = JSON.stringify(activity);
  f.service.invalidateLocal();
  await f.service.request(null, { action: 'markAllRead', entries });
  const unread = await f.service.request<InboxPage>(null, { action: 'list', filter: 'unread' });
  assert.equal(unread.items[0].unreadCount, 1);
  assert.equal(await f.service.badge(null), 101);
});

test('account switch fences a previous HTTP response and local cursor scopes', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-account-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  let finish!: (value: Response) => void, issued!: () => void;
  const started = new Promise<void>((r) => (issued = r));
  const f = fixture('a', [row()], dir, {
    fetcher: async () => {
      issued();
      return new Promise<Response>((r) => (finish = r));
    },
  });
  t.after(() => f.service.close());
  await f.service.request('a', { action: 'markRead', key, generation: 1, through: 12, manualUnreadRevision: 0 });
  const request = f.service.request('a', { action: 'accountPreferences' });
  await started;
  f.setUid('b');
  finish(Response.json({ dmReceipts: true }));
  await assert.rejects(request, /INVALID_SESSION/);
  await assert.rejects(f.service.request('a', { action: 'get', key }), /INVALID_SESSION/);
  assert.equal((await f.service.request<{ item: InboxItem }>('b', { action: 'get', key })).item.readThrough, 10);
});

test('explicit source opt-in publishes sanitized metadata and securely derived stable credentials, disable revokes it', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-publish-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row();
  r.cloudTitle = 'Owner chosen';
  r.sourceLabel = '/private/host';
  r.remoteHostId = 'a'.repeat(22);
  const f = fixture('owner', [r], dir);
  t.after(() => f.service.close());
  await f.service.request('owner', { action: 'list' });
  assert.deepEqual(
    f.calls.map((c) => c.action),
    ['list'],
  );
  await assert.rejects(f.service.request('owner', { action: 'assistantPublish' } as never), /INVALID_INPUT/);
  await f.service.setSummarySync('owner', true);
  const bind = f.calls.find((c) => c.action === 'assistantSource')!,
    publish = f.calls.find((c) => c.action === 'assistantPublish')!;
  assert.match(String(bind.sourceSecret), /^[\w-]{43}$/u);
  assert.equal(bind.sourceSecret, publish.sourceSecret);
  assert.equal(publish.title, 'Owner chosen');
  assert.equal(publish.sourceLabel, undefined);
  assert.equal(publish.remoteHostId, r.remoteHostId);
  assert.doesNotMatch(JSON.stringify(publish), /private|prompt|jsonl|message_count/);
  await f.service.setSummarySync('owner', false);
  assert.equal(f.calls.at(-1)?.enabled, false);
  assert.equal(f.service.summarySync('owner'), false);
  await assert.rejects(f.service.request('other', { action: 'list' }), /INVALID_SESSION/);
});

test('legacy archive migrates once and a unified unarchive survives repeated catalog refresh', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-archive-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row();
  r.archived = true;
  const f = fixture(null, [r], dir);
  t.after(() => f.service.close());
  assert.equal((await f.service.request<InboxPage>(null, { action: 'list' })).items.length, 0);
  await f.service.request(null, {
    action: 'preferences',
    expectedPreferencesRevision: 1,
    key,
    generation: 1,
    patch: { archived: false },
  });
  f.service.invalidateLocal();
  assert.equal((await f.service.request<InboxPage>(null, { action: 'list' })).items.length, 1);
});

test('IPC denies a subframe and foreign WebContents before dispatch', async () => {
  let handler: ((event: unknown, command: unknown) => Promise<unknown>) | undefined,
    started = false;
  const mainFrame = { url: 'file:///app/index.html' },
    webContents = { id: 11, mainFrame },
    target = { webContents, isDestroyed: () => false };
  const ipc = { handle: (_: string, fn: typeof handler) => (handler = fn), removeHandler: () => {} };
  const service = {
    start: () => {
      started = true;
    },
    close() {},
    onSignal: () => () => {},
    onNotification: () => () => {},
    badge: () => 3,
  };
  const stop = bindInboxIpc(ipc as never, service as never, () => [target as never], 'file:///app/index.html');
  assert.ok(started);
  for (const e of [
    { sender: webContents, senderFrame: {} },
    { sender: {}, senderFrame: mainFrame },
  ])
    await assert.rejects(handler!(e, { type: 'badge', uid: null }), /INVALID_SENDER/);
  assert.deepEqual(await handler!({ sender: webContents, senderFrame: mainFrame }, { type: 'badge', uid: null }), {
    value: 3,
  });
  stop();
});

test('persistent delivery ledger is account and device scoped and expires after seven days', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-ledger-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  let uid: string | null = 'a';
  const clock = 100000;
  const a = new NotificationDeliveryLedger(dir, () => uid);
  a.remember(['event'], clock);
  a.close();
  const b = new NotificationDeliveryLedger(dir, () => uid);
  t.after(() => b.close());
  assert.ok(b.has('event', clock + 1));
  uid = 'b';
  assert.equal(b.has('event', clock + 1), false);
  uid = 'a';
  assert.equal(b.has('event', clock + DELIVERY_RETENTION_MS), false);
});

test('pending cloud notices re-check server eligibility before delivery and respect per-event dedupe', async (t) => {
  const now = Date.now(),
    wait = (ms: number) => new Promise((r) => setTimeout(r, ms)),
    seen = new Set<string>(),
    alerts: string[] = [];
  let canceled = false,
    visible = false;
  const post = async (_uid: string, _epoch: number, command: { deviceId: string }) => {
    assert.equal(command.deviceId, 'device-1');
    return {
      items: [
        {
          id: 'pending',
          key,
          generation: 1,
          kind: 'input',
          lastActivityAt: now,
          deliverAfter: now + 380,
          deliverable: !canceled,
        },
        {
          id: 'direct',
          key: 'team:t:c',
          generation: 1,
          kind: 'dm',
          lastActivityAt: now,
          deliverAfter: now,
          deliverable: true,
        },
      ],
    };
  };
  const notices = new NoticeDelivery(
    post as never,
    {
      has: (k) => seen.has(k),
      remember: (keys) => {
        for (const k of keys) seen.add(k);
      },
    },
    () => true,
    () => visible,
    () => true,
    (n) => alerts.push(n.key),
    'device-1',
  );
  t.after(() => notices.reset());
  visible = true;
  notices.changed('a', 1);
  await wait(170);
  assert.deepEqual(alerts, []);
  canceled = true;
  visible = false;
  notices.changed('a', 1);
  await wait(350);
  notices.changed('a', 1);
  await wait(150);
  assert.deepEqual(alerts, ['team:t:c']);
});

test('one shared remote subscription is closed on account switch and late signals cannot update the new badge', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-watch-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const callbacks = new Map<string, (count: number) => void>();
  const closed: string[] = [];
  const f = fixture('a', [], dir, {
    watch: (_sdk, _db, uid, _dirty, badge) => {
      callbacks.set(uid, badge);
      return () => {
        closed.push(uid);
      };
    },
  });
  t.after(() => f.service.close());
  const waitFor = async (fn: () => boolean) => {
    for (let i = 0; i < 150; i++) {
      if (fn()) return;
      await new Promise((r) => setTimeout(r, 10));
    }
    throw new Error('watch did not start');
  };
  f.service.start();
  await waitFor(() => callbacks.has('a'));
  callbacks.get('a')!(5);
  assert.equal(await f.service.badge('a'), 5);
  f.setUid('b');
  await waitFor(() => callbacks.has('b'));
  callbacks.get('a')!(999);
  callbacks.get('b')!(3);
  assert.deepEqual(closed, ['a']);
  assert.equal(await f.service.badge('b'), 3);
});

test('activity page resolves only a current host catalog ref and passes the authoritative project and mode', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-activity-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row(),
    page = {
      items: [{ cursor: 11, resultUuids: ['r1'], kind: 'reply', at: 2 }],
      cursor: 12,
      baselineCursor: 10,
      hasMore: false,
    };
  const f = fixture(null, [r], dir, {
    activityPage: async (ref, after) => {
      assert.deepEqual(ref, r.ref);
      assert.equal(after, 10);
      return page;
    },
  });
  t.after(() => f.service.close());
  assert.deepEqual(await f.service.activityPage(key, 10), page);
  await assert.rejects(f.service.activityPage('assistant:other:session', 10), /SOURCE_ACCESS_REQUIRED/);
  await assert.rejects(f.service.activityPage(key, -1), /INVALID_INPUT/);
});

test('pin order and archived views paginate the same composite ordering as cloud', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-order-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const rows = Array.from({ length: 52 }, (_, i) => row('s' + i, i));
  const f = fixture(null, rows, dir);
  t.after(() => f.service.close());
  for (let i = 0; i < 51; i++)
    await f.service.request(null, {
      action: 'preferences',
      expectedPreferencesRevision: 0,
      key: 'assistant:source:s' + i,
      generation: 1,
      patch: { pinOrder: i },
    });
  const page = await f.service.request<InboxPage>(null, { action: 'list' });
  assert.equal(page.items[0].ref.conversationId, 's0');
  const next = await f.service.request<InboxPage>(null, { action: 'list', after: page.next! });
  assert.deepEqual(
    next.items.map((i) => i.ref.conversationId),
    ['s50', 's51'],
  );
  await f.service.request(null, {
    action: 'preferences',
    expectedPreferencesRevision: 1,
    key: 'assistant:source:s0',
    generation: 1,
    patch: { archived: true },
  });
  const archived = await f.service.request<InboxPage>(null, { action: 'list', archived: true });
  assert.deepEqual(
    archived.items.map((i) => i.ref.conversationId),
    ['s0'],
  );
});

test('a signed-in account can open its private Assistant catalog offline, while denial never becomes an offline page', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-partial-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  let denied = false;
  const f = fixture('owner', [row()], dir, {
    fetcher: async () => {
      if (denied) return Response.json({ error: 'SOURCE_ACCESS_REQUIRED' }, { status: 403 });
      throw new TypeError('fetch failed');
    },
  });
  t.after(() => f.service.close());
  const page = await f.service.request<InboxPage & { partialOffline: boolean }>('owner', { action: 'list' });
  assert.ok(page.partialOffline);
  assert.equal(page.items[0].key, key);
  denied = true;
  await assert.rejects(f.service.request('owner', { action: 'list' }), /SOURCE_ACCESS_REQUIRED/);
});

test('local result notifications require a new committed unread ordinal, honor mute and cancel on actual read', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-result-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row('session', Date.now() - 5100),
    received: string[] = [];
  const f = fixture(null, [r], dir, { notify: (n) => received.push(n.key) });
  t.after(() => f.service.close());
  f.service.start();
  await new Promise((resolve) => setTimeout(resolve, 220));
  assert.deepEqual(received, [key]);
  const update = (cursor: number, at: number) => {
    const a = JSON.parse(r.row.activity_json!);
    r.row.activity_json = JSON.stringify({ ...a, cursor, lastMeaningfulAt: at, sourceRevision: cursor });
    f.service.invalidateLocal();
  };
  update(13, Date.now() - 4380);
  await new Promise((resolve) => setTimeout(resolve, 390));
  await f.service.request(null, { action: 'markRead', key, generation: 1, through: 13, manualUnreadRevision: 0 });
  await new Promise((resolve) => setTimeout(resolve, 360));
  assert.deepEqual(received, [key]);
  await f.service.request(null, {
    action: 'preferences',
    expectedPreferencesRevision: 0,
    key,
    generation: 1,
    patch: { notificationMode: 'none' },
  });
  update(14, Date.now() - 5100);
  await new Promise((resolve) => setTimeout(resolve, 400));
  assert.deepEqual(received, [key]);
  assert.equal(f.service.allowsNotifications('session'), false);
});

test('title prefix paging searches beyond the first activity page in Unicode-normalized order', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-search-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const rows = Array.from({ length: 61 }, (_, i) => {
    const r = row('s' + i, i);
    r.row.title = 'Ａlpha ' + String(i).padStart(2, '0');
    return r;
  });
  const f = fixture(null, rows, dir);
  t.after(() => f.service.close());
  const first = await f.service.request<InboxPage>(null, { action: 'list', searchPrefix: ' ALPHA ' });
  assert.equal(first.items.length, 50);
  assert.equal(first.next?.titleSort, 'alpha 49');
  const next = await f.service.request<InboxPage>(null, { action: 'list', searchPrefix: 'alpha', after: first.next! });
  assert.equal(next.items.length, 11);
  assert.equal(next.items[0].ref.conversationId, 's50');
});

test('first-turn failures with zero results alert immediately, keep zero unread and dedupe across restarts', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row(),
    notices: { key: string; through?: number }[] = [];
  const activity = {
    sourceId: 'source',
    cursor: 0,
    baselineCursor: 0,
    lastMeaningfulAt: 0,
    previewKind: null,
    pendingActionCount: 0,
    failed: true,
    sourceRevision: 1,
    failureAt: Date.now(),
    failureRevision: 1,
  };
  r.row.activity_json = JSON.stringify(activity);
  const create = () => fixture(null, [r], dir, { notify: (notice) => notices.push(notice) });
  const a = create();
  a.service.start();
  await new Promise((r) => setTimeout(r, 230));
  assert.equal(notices.length, 1);
  assert.equal(notices[0].through, undefined);
  const page = await a.service.request<InboxPage>(null, { action: 'list', filter: 'attention' });
  assert.deepEqual(
    [page.items[0].sourceCursor, page.items[0].unreadCount, page.items[0].lastActivityAt],
    [0, 0, activity.failureAt],
  );
  a.service.close();
  const b = create();
  t.after(() => b.service.close());
  b.service.start();
  await new Promise((r) => setTimeout(r, 230));
  assert.equal(notices.length, 1);
  r.row.activity_json = JSON.stringify({ ...activity, sourceRevision: 3, failureAt: Date.now(), failureRevision: 3 });
  b.service.invalidateLocal();
  await new Promise((r) => setTimeout(r, 420));
  assert.equal(notices.length, 2);
});

test('per-conversation summary exclusion survives restart, forgets only the server row and supports a fenced re-enable', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-consent-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row();
  r.cloudTitle = 'Private';
  const rows = [r, row('other')];
  const f = fixture('owner', rows, dir);
  await f.service.setSummarySync('owner', true);
  await f.service.setConversationSummarySync('owner', key, false);
  const remove = f.calls.find((c) => c.action === 'assistantRemove');
  assert.equal(remove?.conversationConsentRevision, 2);
  assert.equal(remove?.conversationId, 'session');
  assert.deepEqual(await f.service.localRef(key), r.ref);
  assert.equal(
    (await f.service.request<{ item: InboxItem }>('owner', { action: 'get', key })).item.title,
    'private auto-generated prompt',
  );
  await assert.rejects(f.service.setConversationSummarySync('other', key, true), /INVALID_SESSION/);
  await assert.rejects(
    f.service.setConversationSummarySync('owner', 'assistant:remote:session', true),
    /SOURCE_ACCESS_REQUIRED/,
  );
  f.service.close();
  const g = fixture('owner', rows, dir);
  t.after(() => g.service.close());
  assert.equal(await g.service.conversationSummarySync('owner', key), false);
  await g.service.setSummarySync('owner', true);
  assert.ok(!g.calls.some((c) => c.action === 'assistantPublish' && c.conversationId === 'session'));
  await g.service.setConversationSummarySync('owner', key, true);
  assert.equal(
    g.calls.find((c) => c.action === 'assistantPublish' && c.conversationId === 'session')?.conversationConsentRevision,
    3,
  );
});

test('a never-synced excluded conversation does not disclose even its ID when global publication starts', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-first-private-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const f = fixture('owner', [row('never-publish'), row('public')], dir);
  t.after(() => f.service.close());
  await f.service.setConversationSummarySync('owner', 'assistant:source:never-publish', false);
  await f.service.setSummarySync('owner', true);
  assert.ok(!f.calls.some((c) => c.conversationId === 'never-publish'));
  assert.ok(f.calls.some((c) => c.conversationId === 'public'));
});

test('global job freezes unpaged local rows at admission, never absorbs later rows or a subsequent manual flag on replay', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-global-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const rows = Array.from({ length: 121 }, (_, i) => row('s' + i)),
    id = 'batch-id';
  let issued!: () => void, release!: () => void;
  const waiting = new Promise<void>((r) => (issued = r)),
    hold = new Promise<void>((r) => (release = r));
  const server = async (_: unknown, init?: RequestInit) => {
    const req = JSON.parse(String(init?.body));
    if (req.action === 'markAllReadSnapshot') {
      issued();
      await hold;
    }
    return Response.json({ jobId: id, state: 'complete', processed: 2, changed: 1 });
  };
  const f = fixture('owner', rows, dir, { fetcher: server });
  const start = f.service.request<{ processed: number; changed: number }>('owner', {
    action: 'markAllReadSnapshot',
    requestedAt: Date.now(),
    operationId: id,
    filter: 'unread',
  });
  await waiting;
  const changed = rows[75];
  const a = JSON.parse(changed.row.activity_json!);
  changed.row.activity_json = JSON.stringify({ ...a, cursor: 14, sourceRevision: 3 });
  rows.push(row('new'));
  f.service.invalidateLocal();
  await f.service.request('owner', {
    action: 'markUnread',
    key: 'assistant:source:s75',
    generation: 1,
    operationId: 'manual-new',
    expectedManualUnreadRevision: 0,
  });
  release();
  assert.deepEqual(await start, { jobId: id, state: 'complete', processed: 123, changed: 122 });
  const get = async (s: typeof f.service, k: string) =>
    (await s.request<{ item: InboxItem }>('owner', { action: 'get', key: 'assistant:source:' + k })).item;
  const old = await get(f.service, 's75');
  assert.deepEqual([old.readThrough, old.unreadCount, old.manualUnread], [12, 2, true]);
  assert.equal((await get(f.service, 'new')).readThrough, 10);
  assert.equal((await get(f.service, 's120')).readThrough, 12);
  f.service.close();
  const g = fixture('owner', rows, dir, { fetcher: server });
  t.after(() => g.service.close());
  const status = await g.service.request<{ processed: number }>('owner', { action: 'readBatchStatus', jobId: id });
  assert.equal(status.processed, 123);
  assert.equal((await get(g.service, 'new')).readThrough, 10);
  await assert.rejects(
    g.service.request('owner', {
      action: 'markAllReadSnapshot',
      requestedAt: Date.now(),
      operationId: id,
      category: 'team',
    }),
    /OPERATION_CONFLICT/,
  );
});

test('personal preferences reject stale or blind edits while reads and refreshes preserve the observed revision', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-preferences-cas-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row(),
    f = fixture(null, [r], dir);
  const pref = {
    action: 'preferences' as const,
    key,
    generation: 1,
    patch: { notificationMode: 'none' as const },
    expectedPreferencesRevision: 0,
  };
  const first = await f.service.request<{ item: InboxItem }>(null, pref);
  assert.equal(first.item.preferencesRevision, 1);
  await f.service.request(null, { action: 'markRead', key, generation: 1, through: 12, manualUnreadRevision: 0 });
  f.service.invalidateLocal();
  const after = await f.service.request<{ item: InboxItem }>(null, { action: 'get', key });
  assert.equal(after.item.preferencesRevision, 1);
  assert.equal(after.item.notificationMode, 'none');
  await assert.rejects(f.service.request(null, { ...pref, patch: { pinOrder: 5 } }), /PREFERENCE_REVISION_CONFLICT/);
  await assert.rejects(
    f.service.request(null, { ...pref, expectedPreferencesRevision: undefined }),
    /PREFERENCE_REVISION_CONFLICT/,
  );
  f.service.close();
  const g = fixture(null, [r], dir);
  t.after(() => g.service.close());
  const current = await g.service.request<{ item: InboxItem }>(null, {
    ...pref,
    expectedPreferencesRevision: 1,
    patch: { pinOrder: 0 },
  });
  assert.equal(current.item.preferencesRevision, 2);
  assert.equal(current.item.notificationMode, 'none');
});

test('global local batch refuses an expired initial click and cannot create a missing job through status', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-batch-expiry-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const f = fixture(null, [row()], dir);
  t.after(() => f.service.close());
  await assert.rejects(
    f.service.request(null, {
      action: 'markAllReadSnapshot',
      operationId: 'offline-click',
      requestedAt: Date.now() - 310000,
    }),
    /BATCH_REQUEST_EXPIRED/,
  );
  await assert.rejects(
    f.service.request(null, { action: 'readBatchStatus', jobId: 'offline-click' }),
    /INBOX_JOB_MISSING/,
  );
  const fresh = { action: 'markAllReadSnapshot' as const, operationId: 'fresh-click', requestedAt: Date.now() };
  assert.deepEqual(await f.service.request(null, fresh), {
    jobId: 'fresh-click',
    state: 'complete',
    processed: 1,
    changed: 1,
  });
  const newer = await f.service.request(null, {
    action: 'markUnread',
    key,
    generation: 1,
    operationId: 'later-flag',
    expectedManualUnreadRevision: 0,
  });
  assert.ok(newer);
  await f.service.request(null, { ...fresh, requestedAt: Date.now() - 310000 });
  assert.equal((await f.service.request<{ item: InboxItem }>(null, { action: 'get', key })).item.manualUnread, true);
});

test('exact local preview stays source-scoped, bounded and read-only and fences an in-flight account switch', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-exact-preview-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row();
  let size = 4,
    invalidate = false;
  const f = fixture('owner', [r], dir, {
    activityPreview: async (ref, through) => {
      assert.deepEqual(ref, r.ref);
      assert.equal(through, 11);
      if (invalidate) f.setUid('next');
      return {
        available: true,
        messages: [
          { role: 'assistant', transcript_uuid: 'result', blocks: [{ type: 'text', text: 'x'.repeat(size) }] },
        ],
      };
    },
  });
  t.after(() => f.service.close());
  assert.ok((await f.service.activityPreview(key, 11)).available);
  assert.equal((await f.service.request<{ item: InboxItem }>('owner', { action: 'get', key })).item.readThrough, 10);
  size = 32769;
  assert.deepEqual(await f.service.activityPreview(key, 11), { available: false, messages: [] });
  await assert.rejects(f.service.activityPreview('assistant:remote:session', 11), /SOURCE_ACCESS_REQUIRED/);
  invalidate = true;
  await assert.rejects(f.service.activityPreview(key, 11), /INVALID_SESSION/);
});

test('acknowledging exactly the observed failure suppresses its reminder and never clears failed attention', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-observed-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const r = row(),
    notices: string[] = [];
  const a = {
    sourceId: 'source',
    cursor: 0,
    baselineCursor: 0,
    lastMeaningfulAt: 0,
    previewKind: null,
    pendingActionCount: 0,
    failed: true,
    sourceRevision: 2,
    failureAt: Date.now(),
    failureRevision: 2,
  };
  r.row.activity_json = JSON.stringify(a);
  const f = fixture(null, [r], dir, { notify: (n) => notices.push(n.key) });
  const read = { action: 'markRead' as const, key, generation: 1, through: 0, manualUnreadRevision: 0 };
  await f.service.request(null, { ...read, observedFailureRevision: 2 });
  f.service.close();
  const g = fixture(null, [r], dir, { notify: (n) => notices.push(n.key) });
  t.after(() => g.service.close());
  g.service.start();
  await new Promise((r) => setTimeout(r, 220));
  const page = await g.service.request<InboxPage>(null, { action: 'list', filter: 'attention' });
  assert.equal(page.items[0].failureRevision, 2);
  assert.deepEqual(notices, []);
  assert.equal(page.items[0].unreadCount, 0);
  r.row.activity_json = JSON.stringify({ ...a, sourceRevision: 4, failureRevision: 4, failureAt: Date.now() });
  g.service.invalidateLocal();
  await g.service.request(null, read);
  await g.service.request(null, { ...read, observedFailureRevision: 2 });
  await new Promise((r) => setTimeout(r, 420));
  assert.deepEqual(notices, [key]);
});

test('a private row promoted during a global read receives the frozen cursor without clearing a newer cloud manual mark', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-promoted-batch-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  let start!: () => void, release!: () => void;
  const started = new Promise<void>((r) => (start = r)),
    hold = new Promise<void>((r) => (release = r));
  const calls: Record<string, unknown>[] = [];
  const f = fixture('owner', [row()], dir, {
    fetcher: async (_, init) => {
      const c = JSON.parse(String(init?.body));
      calls.push(c);
      if (c.action === 'markAllReadSnapshot') {
        start();
        await hold;
        return Response.json({ jobId: c.operationId, state: 'complete', processed: 0 });
      }
      if (c.action === 'assistantSource') return Response.json({ generation: 1, enabled: true });
      return Response.json({ ok: true });
    },
  });
  t.after(() => f.service.close());
  const batch = f.service.request('owner', {
    action: 'markAllReadSnapshot',
    operationId: 'promote-test',
    requestedAt: Date.now(),
  });
  await started;
  await f.service.setSummarySync('owner', true);
  release();
  await batch;
  for (let i = 0; i < 50 && !calls.some((c) => c.action === 'markRead'); i++)
    await new Promise((r) => setTimeout(r, 10));
  const read = calls.find((c) => c.action === 'markRead');
  assert.equal(read?.through, 12);
  assert.equal(read?.manualUnreadRevision, 0);
  assert.ok(read?.operationId);
});

test('Unread and Needs attention include muted archived chats, while All, badges and preferences retain archive behavior', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-archived-filters-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const f = fixture(null, [row(), row('active')], dir);
  t.after(() => f.service.close());
  await f.service.request(null, {
    action: 'preferences',
    key,
    generation: 1,
    expectedPreferencesRevision: 0,
    patch: { archived: true, notificationMode: 'none' },
  });
  const list = async (filter: 'all' | 'unread' | 'attention', archived?: boolean) =>
    (await f.service.request<InboxPage>(null, { action: 'list', filter, archived })).items
      .map((r) => r.ref.conversationId)
      .sort();
  assert.deepEqual(await list('all'), ['active']);
  for (const filter of ['unread', 'attention'] as const) {
    assert.deepEqual(await list(filter), ['active', 'session']);
    assert.deepEqual(await list(filter, false), ['active', 'session']);
    assert.deepEqual(await list(filter, true), ['session']);
  }
  assert.equal(await f.service.badge(null), 1);
  await f.service.request(null, {
    action: 'markAllReadSnapshot',
    operationId: 'archived-unread',
    requestedAt: Date.now(),
    filter: 'unread',
    archived: false,
  });
  assert.deepEqual(await list('unread'), []);
  assert.deepEqual(await list('attention'), ['active', 'session']);
  const { item } = await f.service.request<{ item: InboxItem }>(null, { action: 'get', key });
  assert.equal(item.archived, true);
  assert.equal(item.notificationMode, 'none');
  assert.equal(item.pendingActionCount, 1);
});

test('missing cloud Inbox deployment preserves signed-in local conversations', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-missing-cloud-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const html of [false, true]) {
    const f = fixture('owner', [row()], dir, { fetcher: async () => html
      ? new Response('<html>Not found</html>', { status: 404 })
      : Response.json({ error: { code: 'NOT_FOUND' } }, { status: 404 }) });
    const page = await f.service.request<InboxPage>('owner', { action: 'list' });
    assert.equal(page.partialOffline, true);
    assert.equal(page.cloudUnavailable, true);
    assert.equal(page.items[0].key, key);
    await assert.rejects(f.service.request('other', { action: 'list' }), /INVALID_SESSION/);
    f.service.close();
  }
});

test('Inbox refreshes a rejected token once while preserving permission denial', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'inbox-token-refresh-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const refreshes: boolean[] = [];
  let calls = 0, status = 401;
  const f = fixture('owner', [row()], dir, {
    account: { snapshot: () => ({ user: { uid: 'owner' } }), getIdToken: async force => {
      refreshes.push(Boolean(force)); return force ? 'fresh' : 'old';
    }, onChanged: () => () => {} },
    fetcher: async (_, init) => {
      calls++;
      if (status === 401 && (init?.headers as Record<string, string>).Authorization === 'Bearer fresh')
        return Response.json({ items: [], next: null });
      return Response.json({ error: status === 401 ? 'INVALID_SESSION' : 'SOURCE_ACCESS_REQUIRED' }, { status });
    },
  });
  t.after(() => f.service.close());
  await f.service.request('owner', { action: 'list' });
  assert.deepEqual(refreshes, [false, true]);
  assert.equal(calls, 2);
  status = 403; refreshes.length = 0;
  await assert.rejects(f.service.request('owner', { action: 'list' }), /SOURCE_ACCESS_REQUIRED/);
  assert.deepEqual(refreshes, [false]);
});
