import assert from 'node:assert/strict';
import test from 'node:test';

import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BotPromptSentUi, BotSessionRefUi, BotUi } from '../src/shared/bots';
import { BotSessionsController, recentBots } from '../src/renderer/bridge/botSessionsStore';

const T0 = Date.parse('2026-10-30T12:00:00Z');

function row(uuid: string, modifiedAt: number | string, title = uuid): SessionRowDto {
  return { uuid, title, modified_rfc3339: typeof modifiedAt === 'string' ? modifiedAt : new Date(modifiedAt).toISOString(), message_count: 1, mode: 'code', path: '' } as SessionRowDto;
}

function bot(botId: string, overrides: Partial<BotUi> = {}): BotUi {
  return { botId, name: botId, description: '', createdAt: 1, updatedAt: 1, revision: 1, workspaces: [], ...overrides };
}

/** A host whose answers the test hands out: the next `listSessions` of a Bot it holds waits for `release`. */
function host(initial: Record<string, SessionRowDto[]> = {}) {
  const sessions = new Map(Object.entries(initial));
  const calls: string[] = [];
  const held = new Set<string>();
  const waiting: Array<{ botId: string; settle(): void }> = [];
  const failures = new Map<string, Error>();
  const prompts = new Set<(prompt: BotPromptSentUi) => void>();
  const shown = new Set<(session: BotSessionRefUi) => void>();
  const api = {
    listSessions(botId: string) {
      calls.push(botId);
      const answer = () => {
        const failure = failures.get(botId);
        if (failure) return Promise.reject(failure);
        return Promise.resolve({ sessions: [...(sessions.get(botId) ?? [])] });
      };
      if (!held.delete(botId)) return answer();
      return new Promise<{ sessions: SessionRowDto[] }>((resolve, reject) => {
        waiting.push({ botId, settle: () => { answer().then(resolve, reject); } });
      });
    },
    onPromptSent(callback: (prompt: BotPromptSentUi) => void) { prompts.add(callback); return () => { prompts.delete(callback); }; },
    onSessionShown(callback: (session: BotSessionRefUi) => void) { shown.add(callback); return () => { shown.delete(callback); }; },
  };
  return {
    api, calls, sessions, failures,
    hold(botId: string) { held.add(botId); },
    release(botId?: string) {
      for (const waiter of waiting.splice(0)) {
        if (botId === undefined || waiter.botId === botId) waiter.settle();
        else waiting.push(waiter);
      }
    },
    sendPrompt(sessionId: string) { for (const callback of prompts) callback({ sessionId, text: 'hello' }); },
    showSession(botId: string, sessionId: string) { for (const callback of shown) callback({ botId, projectPath: `/bots/${botId}`, sessionId }); },
    listeners: () => prompts.size + shown.size,
  };
}

/** Let the reads that were started finish. */
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

test('only the Bots on show are read, and each Bot\'s chats come newest first', async () => {
  const h = host({
    a: [row('old', T0 - 3_600_000), row('new', T0), row('mid', T0 - 60_000), row('broken', 'not a date')],
    b: [row('b1', T0)],
  });
  const store = new BotSessionsController(h.api);
  store.show(['a']);
  await settle();
  assert.deepEqual(h.calls, ['a']);
  assert.deepEqual(store.getSnapshot().entries.a?.sessions?.map((session) => session.uuid), ['new', 'mid', 'old', 'broken']);
  assert.equal(store.getSnapshot().entries.a?.loading, false);
  assert.equal(store.getSnapshot().entries.a?.error, undefined);
  assert.equal(store.getSnapshot().entries.b, undefined, 'a Bot that is not on show is not read');
});

test('a Bot already on show is not read again by saying so again, and one that left is read when it comes back', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  store.show(['a']);
  await settle();
  store.show(['a']);
  store.show(['a', 'a']);
  await settle();
  assert.deepEqual(h.calls, ['a']);
  store.show([]);
  store.show(['a']);
  await settle();
  assert.deepEqual(h.calls, ['a', 'a'], 'the sidebar coming up again reads what the Bot has now');
});

test('a read already under way answers every ask for the same Bot', async () => {
  const h = host({ a: [row('s1', T0)] });
  h.hold('a');
  const store = new BotSessionsController(h.api);
  store.show(['a']);
  const first = store.load('a');
  const second = store.load('a');
  assert.equal(first, second);
  assert.deepEqual(h.calls, ['a']);
  assert.equal(store.getSnapshot().entries.a?.loading, true);
  assert.equal(store.getSnapshot().entries.a?.sessions, undefined, 'nothing to show before the first read');
  h.release();
  await first;
  assert.equal(store.getSnapshot().entries.a?.loading, false);
  await store.load('a');
  assert.deepEqual(h.calls, ['a', 'a'], 'once it has finished, the next ask reads again');
});

test('a Bot that is not on show is not read when something asks for it', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  await store.load('a');
  assert.deepEqual(h.calls, []);
  assert.deepEqual(store.getSnapshot().entries, {});
});

test('the chats read before stay up while they are read again, and when the read fails', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  store.show(['a']);
  await settle();
  h.hold('a');
  h.sessions.set('a', [row('s1', T0), row('s2', T0 + 1)]);
  const reading = store.load('a');
  assert.deepEqual(store.getSnapshot().entries.a?.sessions?.map((session) => session.uuid), ['s1'], 'the old list is on show while the new one is read');
  assert.equal(store.getSnapshot().entries.a?.loading, true);
  h.release();
  await reading;
  assert.deepEqual(store.getSnapshot().entries.a?.sessions?.map((session) => session.uuid), ['s2', 's1']);

  h.failures.set('a', new Error('The Bot folder is missing.'));
  await store.load('a');
  const failed = store.getSnapshot().entries.a;
  assert.match(failed?.error ?? '', /The Bot folder is missing\./);
  assert.equal(failed?.loading, false);
  assert.deepEqual(failed?.sessions?.map((session) => session.uuid), ['s2', 's1'], 'a failed read does not empty the list');

  h.failures.clear();
  await store.load('a');
  assert.equal(store.getSnapshot().entries.a?.error, undefined, 'trying again and succeeding clears the problem');
});

test('a chat that finishes its turn makes its Bot be read again; a chat that starts one does not', async () => {
  const h = host({ a: [row('s1', T0)], b: [row('b1', T0)] });
  const store = new BotSessionsController(h.api);
  store.show(['a', 'b']);
  await settle();
  h.calls.length = 0;

  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: false }, { sessionId: 'b1', botId: 'b', turnActive: false }]);
  await settle();
  assert.deepEqual(h.calls, [], 'the first look at the chats is not a change');

  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: true }, { sessionId: 'b1', botId: 'b', turnActive: false }]);
  await settle();
  assert.deepEqual(h.calls, [], 'a turn that has begun has not added to the chat yet');

  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: false }, { sessionId: 'b1', botId: 'b', turnActive: false }]);
  await settle();
  assert.deepEqual(h.calls, ['a']);
});

test('a chat that disappears while its turn was going counts as a turn that ended', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  store.show(['a']);
  await settle();
  h.calls.length = 0;
  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: true }]);
  store.noteRuntimes([]);
  await settle();
  assert.deepEqual(h.calls, ['a']);
  store.noteRuntimes([]);
  await settle();
  assert.deepEqual(h.calls, ['a'], 'and only once');
});

test('a turn that ends in a Bot that is not on show reads nothing', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: true }]);
  store.noteRuntimes([{ sessionId: 's1', botId: 'a', turnActive: false }]);
  await settle();
  assert.deepEqual(h.calls, []);
});

test('the host sending a prompt reads the Bot that has the chat, or every Bot on show when the chat is new', async () => {
  const h = host({ a: [row('s1', T0)], b: [row('b1', T0)] });
  const store = new BotSessionsController(h.api);
  const stop = store.watch(undefined);
  store.show(['a', 'b']);
  await settle();
  h.calls.length = 0;

  h.sendPrompt('b1');
  await settle();
  assert.deepEqual(h.calls, ['b'], 'the chat is listed under b');
  h.calls.length = 0;
  h.sendPrompt('a-new-chat');
  await settle();
  assert.deepEqual([...h.calls].sort(), ['a', 'b'], 'nothing says whose a new chat is');
  stop();
});

test('the host showing a chat of a Bot reads that Bot, if it is on show', async () => {
  const h = host({ a: [row('s1', T0)], b: [row('b1', T0)] });
  const store = new BotSessionsController(h.api);
  const stop = store.watch(undefined);
  store.show(['a']);
  await settle();
  h.calls.length = 0;
  h.showSession('a', 's2');
  h.showSession('b', 'b2');
  await settle();
  assert.deepEqual(h.calls, ['a']);
  stop();
});

test('the owner coming back to the window reads the Bots on show', async () => {
  const h = host({ a: [row('s1', T0)], b: [row('b1', T0)], c: [] });
  const store = new BotSessionsController(h.api);
  const focusTarget = new EventTarget();
  const stop = store.watch(focusTarget);
  store.show(['a', 'c']);
  await settle();
  h.calls.length = 0;
  focusTarget.dispatchEvent(new Event('focus'));
  await settle();
  assert.deepEqual([...h.calls].sort(), ['a', 'c']);
  stop();
});

test('stopping watching stops every way a read was asked for', async () => {
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api);
  const focusTarget = new EventTarget();
  const stop = store.watch(focusTarget);
  assert.equal(h.listeners(), 2);
  store.show(['a']);
  await settle();
  stop();
  assert.equal(h.listeners(), 0);
  h.calls.length = 0;
  focusTarget.dispatchEvent(new Event('focus'));
  h.sendPrompt('s1');
  h.showSession('a', 's1');
  await settle();
  assert.deepEqual(h.calls, []);
});

test('use of a Bot after its chats were read reads them again, and use that was in the read already does not', async () => {
  let now = T0;
  const h = host({ a: [row('s1', T0)] });
  const store = new BotSessionsController(h.api, () => now);
  store.show(['a']);
  await settle();
  h.calls.length = 0;

  store.noteBots([bot('a', { lastActivityAt: T0 - 1000 })]);
  store.noteBots([bot('a')]);
  store.noteBots([bot('a', { lastActivityAt: T0 })]);
  await settle();
  assert.deepEqual(h.calls, [], 'activity at or before the start of the read is in it');

  now = T0 + 5_000;
  store.noteBots([bot('a', { lastActivityAt: T0 + 2_000 })]);
  await settle();
  assert.deepEqual(h.calls, ['a'], 'use after the read began is not in it');
  h.calls.length = 0;
  store.noteBots([bot('a', { lastActivityAt: T0 + 2_000 })]);
  await settle();
  assert.deepEqual(h.calls, [], 'the read that followed began after that use');
  now = T0 + 9_000;
  store.noteBots([bot('a', { lastActivityAt: T0 + 6_000 })]);
  await settle();
  assert.deepEqual(h.calls, ['a']);
});

test('a Bot that is gone loses its chats, is no longer on show, and a read still under way does not bring them back', async () => {
  const h = host({ a: [row('s1', T0)], b: [row('b1', T0)] });
  const store = new BotSessionsController(h.api);
  store.show(['a', 'b']);
  await settle();
  h.hold('a');
  const reading = store.load('a');
  store.noteBots([bot('b')]);
  assert.deepEqual(Object.keys(store.getSnapshot().entries), ['b']);
  h.release('a');
  await reading;
  assert.deepEqual(Object.keys(store.getSnapshot().entries), ['b'], 'the answer that comes after the Bot is gone is thrown away');

  h.calls.length = 0;
  await store.load('a');
  assert.deepEqual(h.calls, [], 'and the Bot is not on show any more');

  store.show(['a', 'b']);
  await settle();
  assert.deepEqual(h.calls, ['a'], 'a Bot of that name made again is read afresh');
  assert.deepEqual(Object.keys(store.getSnapshot().entries).sort(), ['a', 'b']);
});

test('the list of Bots changing under a Bot that was never read changes nothing', async () => {
  const h = host();
  const store = new BotSessionsController(h.api);
  store.noteBots([bot('a', { lastActivityAt: T0 })]);
  await settle();
  assert.deepEqual(h.calls, []);
  assert.deepEqual(store.getSnapshot().entries, {});
});

test('the Bots are ordered by use, and a chat newer than the list knows of puts its Bot first', () => {
  const bots = [
    bot('a', { name: 'Alpha', lastActivityAt: T0 - 5_000 }),
    bot('b', { name: 'Beta', lastActivityAt: T0 - 1_000 }),
    bot('c', { name: 'Gamma', createdAt: T0 - 9_000 }),
    bot('d', { name: 'Delta', createdAt: T0 - 9_000 }),
  ];
  const ids = (list: readonly BotUi[]) => list.map((entry) => entry.botId);
  assert.deepEqual(ids(recentBots(bots, {})), ['b', 'a', 'd', 'c'], 'by use, then by when it was made, then by name');
  const entries = {
    a: { sessions: [row('x', T0), row('y', T0 - 2_000)], loading: false, error: undefined },
    b: { sessions: undefined, loading: true, error: undefined },
    c: { sessions: [], loading: false, error: undefined },
  };
  assert.deepEqual(ids(recentBots(bots, entries)), ['a', 'b', 'd', 'c']);
  assert.equal(bots[0]!.lastActivityAt, T0 - 5_000, 'the Bots given are not changed');
});
