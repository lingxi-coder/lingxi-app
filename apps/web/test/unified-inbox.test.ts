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
import { loadTeamsModule } from './support/loadTeamsModule';
import { noticeTargetFromSearch, sourceRoute } from '../src/inbox/route';

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

test('mark unread carries the manual revision seen on the row', async () => {
  let command: InboxCommand | undefined;
  const model = controller((cmd) => {
    if (cmd.action === 'markUnread') command = cmd;
    return cmd.action === 'list' ? { items: [] } : {};
  });
  try {
    await model.markUnread(row(2, { manualUnreadRevision: 9 }));
    assert.ok(command?.action === 'markUnread');
    assert.equal(command.expectedManualUnreadRevision, 9);
  } finally {
    model.close();
  }
});
test('global read snapshots the server scope without listing every page and request identity survives a lost reply', async () => {
  const seen: InboxCommand[] = [];
  const { records, storage } = memory();
  const model = controller((cmd) => {
    seen.push(cmd);
    if (cmd.action === 'list') return { items: [row(1)], next: { key: row(1).key, lastActivityAt: 100 } };
    if (cmd.action === 'markAllReadSnapshot') throw Error('connection dropped after commit');
    if (cmd.action === 'readBatchStatus')
      return { jobId: cmd.jobId, state: 'complete', processed: 10000, changed: 9999 };
    return {};
  }, storage);
  try {
    await model.start();
    await model.setFilter('unread', 'team', false, 'Fix');
    await model.markAllReadSnapshot();
    const [issued] = seen.filter((cmd) => cmd.action === 'markAllReadSnapshot');
    assert.ok(issued?.action === 'markAllReadSnapshot');
    assert.equal(issued.searchPrefix, 'fix');
    assert.equal(issued.category, 'team');
    assert.ok(issued.requestedAt > 0);
    assert.ok(seen.some((cmd) => cmd.action === 'readBatchStatus' && cmd.jobId === issued.operationId));
    assert.equal(
      seen.some((cmd) => cmd.action === 'list' && cmd.after),
      false,
    );
    assert.equal(records.get('alice')?.batch, undefined);
    assert.equal(model.getSnapshot().batchBusy, false);
  } finally {
    model.close();
  }
});
test('restart resumes an existing batch without creating a new read boundary', async () => {
  const { storage } = memory();
  await storage.update('alice', (r) => {
    r.batch = { jobId: 'existing', startedAt: 1 };
  });
  const seen: InboxCommand[] = [];
  const model = controller((cmd) => {
    seen.push(cmd);
    return cmd.action === 'list' ? { items: [] } : { jobId: 'existing', state: 'complete' };
  }, storage);
  try {
    await model.start();
    assert.ok(seen.some((c) => c.action === 'readBatchStatus'));
    assert.ok(!seen.some((c) => c.action === 'markAllReadSnapshot'));
  } finally {
    model.close();
  }
});
test('stale preference and its dependent offline edits never overwrite the remote device', async () => {
  let connection = false,
    revision = 4;
  const tried: number[] = [];
  const { records, storage } = memory(),
    initial = row(1, { preferencesRevision: 3 });
  const model = controller((cmd) => {
    if (cmd.action === 'list')
      return { items: [{ ...initial, preferencesRevision: connection ? revision : 3 }] };
    if (cmd.action === 'preferences') {
      if (!connection) throw Error('offline');
      tried.push(cmd.expectedPreferencesRevision!);
      if (cmd.expectedPreferencesRevision !== revision) throw Error('PREFERENCE_REVISION_CONFLICT');
      return { item: { ...initial, ...cmd.patch, preferencesRevision: ++revision } };
    }
    return {};
  }, storage);
  try {
    await model.start();
    await model.preferences(initial, { notificationMode: 'none' });
    await model.preferences(initial, { archived: true });
    assert.deepEqual(
      records
        .get('alice')!
        .queue.map((q) =>
          q.command.action === 'preferences' ? q.command.expectedPreferencesRevision : undefined,
        ),
      [3, 4],
    );
    connection = true;
    await model.markRead(initial);
    assert.deepEqual(tried, [3]);
    assert.equal(revision, 4);
    assert.equal(records.get('alice')!.queue.length, 0);
    assert.equal(model.getSnapshot().error, 'PREFERENCE_REVISION_CONFLICT');
  } finally {
    model.close();
  }
});
test('moving a pinned conversation keeps source keys stable and changes only its rank when there is space', async () => {
  const rows = [row(1, { pinOrder: 100 }), row(2, { pinOrder: 200 }), row(3, { pinOrder: 300 })],
    seen: InboxCommand[] = [];
  const model = controller((cmd) => {
    if (cmd.action === 'list') return { items: rows };
    if (cmd.action === 'preferences') {
      seen.push(cmd);
      const item = rows.find((r) => r.key === cmd.key)!;
      Object.assign(item, cmd.patch);
      return { item };
    }
    return {};
  });
  try {
    await model.start();
    await model.movePinned(rows[2]!, -1);
    assert.equal(seen.length, 1);
    assert.equal(rows[2]!.pinOrder, 150);
  } finally {
    model.close();
  }
});

test('a notification preview verifies generation, conversation, ID, and source cursor without writing a read', async () => {
  const { readSourceNotice } = loadTeamsModule<typeof import('../../../packages/inbox/SourceNoticePreview')>(
    '../inbox/SourceNoticePreview',
  );
  const scope = { kind: 'team' as const, sourceId: 'source', conversationId: 'C3', generation: 2 };
  const calls: unknown[] = [];
  const api = {
    messages: async <T>(command: unknown) => {
      calls.push(command);
      return {
        messages: [
          { id: 'new', seq: 5, text: 'later content' },
          { id: 'wanted', seq: 4, text: 'source entry' },
        ],
      } as T;
    },
  };
  const target = { key: 'team:source:C3', generation: 2, through: 4, messageId: 'wanted' };
  assert.deepEqual(await readSourceNotice(api, scope, target), { text: 'source entry', recalled: undefined });
  await assert.rejects(
    readSourceNotice(api, scope, { ...target, messageId: 'not-present' }),
    /TARGET_UNAVAILABLE/,
  );
  await assert.rejects(readSourceNotice(api, scope, { ...target, generation: 1 }), /STALE_MEMBERSHIP/);
  assert.ok(calls.every((c) => (c as { action: string }).action === 'history'));
  const url = new URL(sourceRoute(row(3), target)!, 'http://test.invalid');
  assert.deepEqual(noticeTargetFromSearch(url.search), target);
  assert.equal(noticeTargetFromSearch('?preview=1&through=3.1')?.through, -1);
});
test('the source task preview refuses a task from a different conversation', async () => {
  const { readSourceNotice } = loadTeamsModule<typeof import('../../../packages/inbox/SourceNoticePreview')>(
    '../inbox/SourceNoticePreview',
  );
  const scope = {
    kind: 'service' as const,
    sourceId: 's_abcdefghijkl',
    conversationId: 'room',
    generation: 1,
  };
  const api = {
    serviceMessages: async <T>() =>
      ({ taskId: 'task-012345', conversationId: 'other', state: 'running' }) as T,
  };
  await assert.rejects(
    readSourceNotice(api, scope, { through: 0, taskId: 'task-012345' }),
    /TARGET_UNAVAILABLE/,
  );
});
test('a failed source retains attention after a read observes the failure revision', () => {
  const failed = row(1, {
    failureRevision: 9,
    pendingActionRevision: 4,
    state: 'failed',
    attentionKind: 'failure',
  });
  assert.equal(freezeRead(failed).observedFailureRevision, 9);
  assert.equal(failed.attentionKind, 'failure');
});

test('All hides archived conversations while Unread and Attention can include them without unarchiving', async () => {
  const muted = row(1, { archived: true, notificationMode: 'none', unreadCount: 2, pendingActionCount: 0 });
  const pending = row(2, { archived: true, pendingActionCount: 1, unreadCount: 0 });
  const normal = row(3, { unreadCount: 0, pendingActionCount: 0 });
  const all = [muted, pending, normal],
    commands: InboxCommand[] = [];
  const model = controller((cmd) => {
    commands.push(cmd);
    if (cmd.action !== 'list') return {};
    const items = all.filter((item) => {
      if (cmd.archived === true && !item.archived) return false;
      if (cmd.archived === false && item.archived) return false;
      if (cmd.filter === 'unread') return item.unreadCount > 0;
      if (cmd.filter === 'attention') return item.pendingActionCount > 0;
      return cmd.archived === true || !item.archived;
    });
    return { items };
  });
  try {
    await model.start();
    assert.deepEqual(
      model.getSnapshot().items.map((r) => r.key),
      [normal.key],
    );
    await model.setFilter('unread');
    assert.deepEqual(model.getSnapshot().items, [muted]);
    await model.setFilter('attention');
    assert.deepEqual(model.getSnapshot().items, [pending]);
    assert.ok(commands.every((c) => c.action === 'list' && c.archived === undefined));
    assert.equal(muted.archived, true);
    assert.equal(pending.archived, true);
  } finally {
    model.close();
  }
});
