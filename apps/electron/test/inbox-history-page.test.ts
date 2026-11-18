import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createDesktopHistoryReader } from '../src/main/teams/historyPage.js';
import { desktopTeamsApi, registerDesktopInboxReads } from '../src/renderer/bridge/desktopTeamsApi.js';
import type { InboxCommand } from '../../../packages/inbox/contracts.js';

const scope = { teamId: 'team12345', generation: 3 };
const source = {
  seq: 3,
  changeSeq: 4,
  changeId: 'change-4',
  text: 'please approve',
  kind: 'question',
  author: { kind: 'user', uid: 'friend' },
  waitId: 'wait-1',
  taskIds: ['t-1'],
  mentions: ['me'],
  replyTo: 'older',
  readFiles: [{ attachmentId: 'a-1', name: 'notes.txt' }],
  decision: 'approve',
};
function fixture() {
  const queries: unknown[][] = [];
  let generation = 3,
    online = true,
    now = 123456789,
    current = true,
    afterRead = () => {};
  const data = (path: string) => ({
    data: () =>
      path.includes('/members/')
        ? { generation }
        : path.includes('/conversations/')
          ? { memberUids: ['me'] }
          : { state: 'active' },
  });
  const sdk = {
    doc: (_: unknown, ...parts: string[]) => parts.join('/'),
    collection: (_: unknown, ...parts: string[]) => parts.join('/'),
    getDocFromServer: async (path: string) => {
      if (!online) throw Object.assign(new Error('unavailable'), { code: 'unavailable' });
      return data(path);
    },
    getDocFromCache: async (path: string) => data(path),
    query: (...q: unknown[]) => {
      queries.push(q);
      return q;
    },
    where: (...q: unknown[]) => q,
    orderBy: (...q: unknown[]) => q,
    limit: (n: number) => n,
    getDocs: async () => {
      afterRead();
      return {
        docs: [
          { id: 'm3', data: () => source },
          { id: 'm1', data: () => ({ ...source, seq: 1 }) },
        ],
      };
    },
  };
  const reader = createDesktopHistoryReader(
    sdk as never,
    {} as never,
    'me',
    () => current,
    () => now,
  );
  return {
    reader,
    queries,
    setGeneration: (g: number) => {
      generation = g;
    },
    setOnline: (v: boolean) => {
      online = v;
    },
    advance: (n: number) => {
      now += n;
    },
    setCurrent: (v: boolean) => {
      current = v;
    },
    afterRead: (fn: () => void) => {
      afterRead = fn;
    },
  };
}

test('Desktop history returns bounded ordered TeamMessage DTOs including approval context, mentions and file references', async () => {
  const f = fixture(),
    page = await f.reader.read(scope, 'room12345', 4);
  assert.deepEqual(f.queries, [
    ['teams/team12345/conversations/room12345/messages', ['seq', '<', 4], ['seq', 'desc'], 50],
  ]);
  assert.deepEqual(
    page.map((m) => m.seq),
    [1, 3],
  );
  assert.equal(page[1].decision, 'approve');
  assert.equal(page[1].replyTo, 'older');
  assert.deepEqual(page[1].taskIds, ['t-1']);
  assert.deepEqual(page[1].readFiles, source.readFiles);
  assert.deepEqual(page[1].mentions, ['me']);
});
test('membership and account are checked again after reading a cached page', async () => {
  const f = fixture();
  f.afterRead(() => f.setGeneration(4));
  await assert.rejects(f.reader.read(scope, 'room12345', 4), /STALE_MEMBERSHIP/);
  f.setGeneration(3);
  f.afterRead(() => f.setCurrent(false));
  await assert.rejects(f.reader.read(scope, 'room12345', 4), /INVALID_SESSION/);
  await assert.rejects(f.reader.read({ ...scope, generation: 0 }, 'room12345', 4), /INVALID_INPUT/);
});
test('offline history lease expires and explicit revocation immediately invalidates cached access', async () => {
  const f = fixture();
  f.setOnline(false);
  await assert.rejects(f.reader.read(scope, 'room12345', 4), /unavailable/);
  f.setOnline(true);
  await f.reader.read(scope, 'room12345', 4);
  f.setOnline(false);
  assert.equal((await f.reader.read(scope, 'room12345', 4)).length, 2);
  f.advance(300001);
  await assert.rejects(f.reader.read(scope, 'room12345', 4), /unavailable/);
  f.setOnline(true);
  await f.reader.read(scope, 'room12345', 4);
  f.reader.revoke(scope, 'room12345');
  f.setOnline(false);
  await assert.rejects(f.reader.read(scope, 'room12345', 4), /unavailable/);
});
test('a Desktop Friends/Teams adapter binds history, visible reads and bilateral receipts to its signed-in UID', async () => {
  const requests: InboxCommand[] = [],
    history: unknown[][] = [];
  const stop = registerDesktopInboxReads('me', {
    mutate: async (command) => {
      requests.push(command);
    },
  });
  try {
    const api = desktopTeamsApi(
      {
        historyPage: async (...args: unknown[]) => {
          history.push(args);
          return [];
        },
      } as never,
      'me',
      undefined,
      {
        request: async (uid: string, command: InboxCommand) => {
          assert.equal(uid, 'me');
          requests.push(command);
          return { readThrough: 1 };
        },
      } as never,
    );
    await api.historyPage!(scope, 'dm12345', 10);
    await api.readVisible!(scope, 'dm12345', 4, 8);
    assert.deepEqual(await api.peerReceipt!(scope, 'dm12345'), { readThrough: 1 });
    assert.deepEqual(history, [['me', scope, 'dm12345', 10]]);
    const read = requests[0];
    assert.equal(read.action, 'markRead');
    if (read.action !== 'markRead') throw new Error('invalid action');
    assert.match(read.operationId!, /^[\w-]+$/);
    assert.equal(read.through, 4);
    assert.equal(read.manualUnreadRevision, 8);
    assert.deepEqual(requests[1], { action: 'peerReceipt', key: 'team:team12345:dm12345', generation: 3 });
  } finally {
    stop();
  }
});
