import assert from 'node:assert/strict';
import test from 'node:test';
import {
  freezeRead,
  inboxKey,
  type InboxCommand,
  type InboxItem,
  type InboxPage,
  type InboxRequest,
} from '../../../packages/inbox/contracts';
import { InboxController } from '../../../packages/inbox/controller';
import { emptyRecord, enqueue, type InboxRecord, type InboxStorage } from '../../../packages/inbox/storage';
import { mergeRanges, VisibleReadFrontier } from '../../../packages/inbox/visibility';
import {
  applyMessageSnapshot,
  emptyProjection,
  type MessageSnapshot,
  type TeamMessage,
} from '../../../packages/teams/messages';
import { unreadCount } from '../../../packages/teams/workspace';
import { sourceRoute } from '../src/inbox/route';

function row(id: number, extra: Partial<InboxItem> = {}): InboxItem {
  const ref = { kind: 'team' as const, sourceId: 'source', conversationId: 'C' + id, generation: 2 };
  return {
    schemaVersion: 1,
    key: inboxKey(ref),
    ref,
    category: 'team',
    title: 'Chat ' + id,
    preview: 'hello',
    lastActivityAt: 10000 - id,
    sourceRevision: 2,
    sourceCursor: 7,
    readThrough: 3,
    unreadCount: 2,
    mentionCount: 0,
    pendingActionCount: 1,
    state: 'idle',
    pinOrder: null,
    archived: false,
    manualUnread: false,
    manualUnreadRevision: 4,
    stateRevision: 2,
    offline: false,
    notificationMode: 'mentions',
    ...extra,
  };
}
function memory() {
  const records = new Map<string, InboxRecord>();
  const storage: InboxStorage = {
    async update(uid, reduce) {
      const copy = structuredClone(records.get(uid) ?? emptyRecord());
      const value = reduce(copy);
      records.set(uid, copy);
      return value;
    },
  };
  return { records, storage };
}
function controller(handler: (c: InboxCommand) => unknown, storage = memory().storage) {
  const request: InboxRequest = async <T>(cmd: InboxCommand) => (await handler(cmd)) as T;
  return new InboxController({ uid: 'alice', request, storage });
}

test('key deduplicates a Friends pair, rejects ambiguous segments, and routes by source authority', () => {
  const item = row(1, { category: 'friend', peerUid: 'bob' });
  assert.equal(item.key, 'team:source:C1');
  assert.equal(sourceRoute(item), '/friends?chat=bob');
  assert.equal(sourceRoute({ ...item, category: 'team' }), '/teams?team=source&chat=C1');
  const ref = { ...item.ref, kind: 'assistant' as const };
  assert.equal(sourceRoute({ ...item, ref, key: inboxKey(ref) }), undefined);
  assert.equal(
    sourceRoute({ ...item, ref, key: inboxKey(ref), remoteHostId: 'Host' }),
    '/remote/Host/C1?inbox=assistant%3Asource%3AC1',
  );
  assert.throws(() => inboxKey({ ...ref, sourceId: 'bad:pair' }));
  assert.equal(
    unreadCount({ lastSeq: 100, lastReadSeq: 1, unreadCount: 0 }),
    0,
    'own messages and edits leave count unchanged',
  );
});

test('visible dwell confirms contiguous content; flush uses the original manual generation', () => {
  let now = 0;
  const sent: number[][] = [];
  const f = new VisibleReadFrontier(
    2,
    3,
    (...args) => sent.push(args),
    () => now,
  );
  f.observe(5, true);
  now = 301;
  f.observe(5, false);
  assert.deepEqual(sent, []);
  f.observe(3, true);
  now += 299;
  f.leave();
  assert.deepEqual(sent, []);
  f.observe(3, true);
  f.observe(4, true);
  now += 300;
  f.observe(4, false);
  assert.deepEqual(sent, [[5, 3]]);
  f.observe(6, true);
  now += 350;
  f.observe(6, false);
  assert.equal(sent.length, 1);
  f.close();
  assert.deepEqual(sent, [
    [5, 3],
    [6, 3],
  ]);
});

test('offline intents survive restart; storage admission, auth fencing and user-visible read revision remain independent', async () => {
  const { records, storage } = memory();
  let online = false;
  const commands: InboxCommand[] = [];
  const handler = (cmd: InboxCommand) => {
    if (cmd.action === 'list') return { items: [row(1)] };
    if (!online) throw Error('offline');
    commands.push(cmd);
    return {};
  };
  const first = controller(handler, storage);
  await first.start();
  await first.markRead(row(1));
  assert.equal(records.get('alice')!.queue.length, 1);
  first.close();
  const pending = records.get('alice')!.queue[0]!;
  pending.lease = undefined;
  online = true;
  const second = controller(handler, storage);
  try {
    await second.start();
    assert.equal(records.get('alice')!.queue.length, 0);
    assert.deepEqual(commands[0], pending.command);
    assert.equal(records.has('bob'), false);
    const frozen = freezeRead(row(1));
    assert.equal(frozen.through, 7);
    assert.equal(frozen.manualUnreadRevision, 4);
    assert.ok(frozen.operationId);
  } finally {
    second.close();
  }
});
test('batch freezes all cursors before networking, dedupes shared keys, and preserves new arrivals', async () => {
  const rows = Array.from({ length: 62 }, (_, i) => row(i)),
    seen: InboxCommand[] = [];
  const c = controller((cmd) => {
    if (cmd.action === 'list') return { items: rows.slice(0, 50) };
    seen.push(structuredClone(cmd));
    rows.forEach((v) => {
      v.sourceCursor++;
      v.manualUnreadRevision++;
    });
    return { results: [] };
  });
  try {
    await c.markAllRead(rows);
    assert.deepEqual(
      seen.map((s) => (s.action === 'markAllRead' ? s.entries.length : 0)),
      [50, 12],
    );
    for (const s of seen)
      if (s.action === 'markAllRead')
        for (const entry of s.entries) {
          assert.equal(entry.through, 7);
          assert.equal(entry.manualUnreadRevision, 4);
        }
  } finally {
    c.close();
  }
});
test('filter switches fence late replies and an authoritative replacement evicts revoked rows', async () => {
  let delay: ((p: InboxPage) => void) | undefined;
  const c = controller(async (cmd) => {
    if (cmd.action !== 'list') return {};
    if (cmd.filter === 'unread')
      return await new Promise<InboxPage>((r) => {
        delay = r;
      });
    return { items: cmd.filter === 'attention' ? [] : [row(1)] };
  });
  try {
    await c.start();
    const slow = c.setFilter('unread');
    await c.setFilter('attention');
    delay!({ items: [row(3)] });
    await slow;
    assert.deepEqual(c.getSnapshot().items, []);
    assert.equal(c.getSnapshot().filter, 'attention');
  } finally {
    c.close();
  }
});
test('admitted reads coalesce only when unleased, from the same generation and on the same side of mark unread', () => {
  const record = emptyRecord(),
    r = row(1);
  enqueue(record, { action: 'markRead', ...freezeRead(r, 4) }, 'a');
  enqueue(record, { action: 'markRead', ...freezeRead(r, 5) }, 'b');
  enqueue(record, { action: 'markUnread', key: r.key, generation: 2, operationId: 'm' }, 'm');
  enqueue(record, { action: 'markRead', ...freezeRead(r, 6) }, 'c');
  assert.equal(record.queue.length, 3);
  const full = emptyRecord();
  full.queue = Array.from({ length: 10000 }, () => structuredClone(record.queue[1]!));
  assert.throws(() => enqueue(full, { action: 'markRead', ...freezeRead(r) }, 'next'), /OUTBOX_FULL/);
  assert.equal(full.queue.length, 10000);
});
test('paged transcript retains a recalled old row by ID and enforces 200 in either browsing direction', () => {
  const scope = { teamId: 'team', generation: 1 },
    id = 'chat';
  const m = (seq: number): TeamMessage => ({
    id: String(seq),
    seq,
    changeSeq: seq,
    changeId: String(seq),
    text: 'message',
    createdAt: null,
    kind: 'text',
    author: { kind: 'user', uid: 'bob' },
  });
  const snapshot: Extract<MessageSnapshot, { kind: 'messages' }> = {
    ...scope,
    uid: 'alice',
    fromCache: false,
    kind: 'messages',
    conversationId: id,
    messages: [],
    replace: false,
  };
  let state = emptyProjection('alice', scope);
  state = applyMessageSnapshot(state, {
    ...snapshot,
    messages: Array.from({ length: 200 }, (_, i) => m(101 + i)),
    replace: true,
  });
  state = applyMessageSnapshot(state, {
    ...snapshot,
    messages: Array.from({ length: 50 }, (_, i) => m(51 + i)),
    older: true,
  });
  assert.deepEqual([state.threads[id]!.messages[0]!.seq, state.threads[id]!.messages.at(-1)!.seq], [51, 250]);
  state = applyMessageSnapshot(state, {
    ...snapshot,
    updatedIds: ['52', '1'],
    messages: [{ ...m(52), recalled: true, changeSeq: 450 }, m(1)],
  });
  assert.equal(state.threads[id]!.messages.length, 200);
  assert.equal(state.threads[id]!.messages[1]!.recalled, true);
});

test('disjoint source exposures survive a reopen and close only a contiguous cursor gap', () => {
  const seen: number[] = [];
  const ledger = mergeRanges(
    [
      [8, 9],
      [6, 7],
      [12, 14],
      [1, 3],
    ],
    4,
  );
  assert.deepEqual(ledger, [
    [6, 9],
    [12, 14],
  ]);
  const reader = new VisibleReadFrontier(4, 7, (through) => seen.push(through));
  reader.restore(ledger);
  assert.deepEqual(seen, []);
  reader.restore([[5, 5]]);
  reader.close();
  assert.deepEqual(seen, [9]);
});

test('indexed title search starts from its own cursor, survives paging and restores default feed on clear', async () => {
  const calls: InboxCommand[] = [];
  const c = controller((cmd) => {
    calls.push(cmd);
    if (cmd.action !== 'list') return {};
    if (cmd.searchPrefix === 'abc')
      return {
        items: [row(cmd.after ? 101 : 100)],
        next: cmd.after ? null : { titleSort: 'abc', key: row(100).key, lastActivityAt: 10 },
      };
    return { items: [row(1)] };
  });
  try {
    await c.start();
    await c.setFilter('all', undefined, false, ' ＡbC ');
    await c.more();
    assert.deepEqual(
      c.getSnapshot().items.map((r) => r.key),
      [row(100).key, row(101).key],
    );
    const page = calls.find((cmd) => cmd.action === 'list' && cmd.after);
    assert.ok(page?.action === 'list' && page.after?.titleSort === 'abc');
    await c.setFilter('all', undefined, false, '');
    assert.equal(c.getSnapshot().items[0]?.key, row(1).key);
  } finally {
    c.close();
  }
});

test('a 100,000-message team keeps the active projection at 200 across 50-message batches', () => {
  const scope = { teamId: 'large-team', generation: 2 },
    conversationId = 'active-chat';
  let state = emptyProjection('member-1', scope);
  const members = Array.from({ length: 200 }, (_, index) => 'member-' + index);
  for (let start = 1; start <= 100000; start += 50) {
    const messages: TeamMessage[] = Array.from({ length: 50 }, (_, index) => {
      const seq = start + index;
      return {
        id: String(seq),
        seq,
        changeSeq: seq,
        changeId: 'c' + seq,
        createdAt: null,
        kind: 'text',
        author: { kind: 'user', uid: members[seq % members.length]! },
        text: 'Fixture message ' + seq,
      };
    });
    state = applyMessageSnapshot(state, {
      ...scope,
      uid: 'member-1',
      kind: 'messages',
      conversationId,
      fromCache: false,
      replace: false,
      messages,
    });
    assert.ok(state.threads[conversationId]!.messages.length <= 200);
  }
  assert.deepEqual(
    state.threads[conversationId]!.messages.map((row) => row.seq),
    Array.from({ length: 200 }, (_, index) => 99801 + index),
  );
});
