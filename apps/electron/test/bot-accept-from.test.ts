import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

import { BotContent } from '../src/main/bots/content.js';
import { BotsUi, botUi } from '../src/main/bots/rendererIpc.js';
import { BotService } from '../src/main/bots/service.js';
import { MAX_ACCEPT_FROM, parseRecord, type BotRecord, type BotStore } from '../src/main/bots/store.js';
import { CH_BOTS_CREATE, CH_BOTS_UPDATE } from '../src/shared/bots.js';
import { botFixture, noTasks } from './bot-helpers.js';

const OTHER = 'Bot0000000000000000777';

function raw(store: BotStore, botId: string): Record<string, unknown> {
  return JSON.parse(readFileSync(store.recordPath(botId), 'utf8'));
}

function withAcceptFrom(store: BotStore, bot: BotRecord, acceptFrom: unknown): string {
  return JSON.stringify({ ...raw(store, bot.bot_id), accept_from: acceptFrom });
}

test('a Bot accepts tasks from no Bot until its record names some, and the record keeps them sorted', (t) => {
  const f = botFixture(t);
  const first = f.store.create({ name: 'First' });
  const second = f.store.create({ name: 'Second' });
  assert.equal('accept_from' in raw(f.store, first.bot_id), false, 'absent means none');
  const target = f.store.create({ name: 'Target', acceptFrom: [second.bot_id, first.bot_id] });
  assert.deepEqual(target.accept_from, [first.bot_id, second.bot_id]);
  assert.deepEqual(raw(f.store, target.bot_id)['accept_from'], [first.bot_id, second.bot_id]);
  assert.deepEqual(f.store.get(target.bot_id).accept_from, [first.bot_id, second.bot_id]);
  const empty = f.store.create({ name: 'Empty', acceptFrom: [] });
  assert.equal('accept_from' in raw(f.store, empty.bot_id), false, 'an empty list is the same as none');
});

test('changing who may delegate to a Bot is a change of the Bot: the revision moves, the same list does not', (t) => {
  const f = botFixture(t);
  const one = f.store.create({ name: 'One' });
  const two = f.store.create({ name: 'Two' });
  const target = f.store.create({ name: 'Target' });
  const added = f.store.update(target.bot_id, 1, { acceptFrom: [one.bot_id] });
  assert.equal(added.revision, 2);
  assert.deepEqual(added.accept_from, [one.bot_id]);
  const same = f.store.update(target.bot_id, 2, { acceptFrom: [one.bot_id] });
  assert.equal(same.revision, 2, 'the same list changes nothing');
  const reordered = f.store.update(target.bot_id, 2, { acceptFrom: [two.bot_id, one.bot_id] });
  assert.equal(reordered.revision, 3);
  const sameSet = f.store.update(target.bot_id, 3, { acceptFrom: [one.bot_id, two.bot_id] });
  assert.equal(sameSet.revision, 3, 'the order of the list does not matter');
  const untouched = f.store.update(target.bot_id, 3, { name: 'Renamed' });
  assert.deepEqual(untouched.accept_from, [one.bot_id, two.bot_id], 'a change of something else keeps the list');
  const cleared = f.store.update(target.bot_id, 4, { acceptFrom: [] });
  assert.equal(cleared.revision, 5);
  assert.equal('accept_from' in cleared, false);
  assert.equal('accept_from' in raw(f.store, target.bot_id), false, 'the field is removed, not left empty');
});

test('the list of Bots that may delegate is checked: valid ids, each once, existing, never the Bot itself, not too many', (t) => {
  const f = botFixture(t);
  const one = f.store.create({ name: 'One' });
  const target = f.store.create({ name: 'Target' });
  for (const [acceptFrom, message] of [
    [['not an id'], /invalid accept_from/],
    [[one.bot_id, one.bot_id], /invalid accept_from/],
    [[target.bot_id], /invalid accept_from/],
    [[OTHER], /does not exist/],
    [Array.from({ length: MAX_ACCEPT_FROM + 1 }, (_, index) => `Bot${String(index).padStart(19, '0')}`), /invalid accept_from/],
    [[1], /invalid accept_from/],
    ['Bot0000000000000000001', /invalid accept_from/],
  ] as Array<[unknown, RegExp]>) {
    assert.throws(() => f.store.update(target.bot_id, 1, { acceptFrom: acceptFrom as string[] }), message, JSON.stringify(acceptFrom).slice(0, 60));
  }
  assert.throws(() => f.store.create({ name: 'Bad', acceptFrom: [OTHER] }), /does not exist/);
  assert.equal(f.store.get(target.bot_id).revision, 1, 'a refused change leaves the Bot as it was');
  assert.equal(f.store.list().length, 2, 'and a refused Bot is not made');
});

test('a Bot that was deleted may stay in the list it was in, and is dropped when the list is next written', (t) => {
  const f = botFixture(t);
  const gone = f.store.create({ name: 'Gone' });
  const keep = f.store.create({ name: 'Keep' });
  const target = f.store.create({ name: 'Target', acceptFrom: [gone.bot_id, keep.bot_id] });
  f.store.trash(gone.bot_id);
  assert.deepEqual(f.store.get(target.bot_id).accept_from, [gone.bot_id, keep.bot_id], 'reading does not check that they exist');
  const same = f.store.update(target.bot_id, 1, { acceptFrom: [gone.bot_id, keep.bot_id] });
  assert.equal(same.revision, 1, 'saying the same thing again is not an error');
  const dropped = f.store.update(target.bot_id, 1, { acceptFrom: [keep.bot_id] });
  assert.deepEqual(dropped.accept_from, [keep.bot_id]);
  assert.throws(() => f.store.update(target.bot_id, 2, { acceptFrom: [gone.bot_id, keep.bot_id] }), /does not exist/, 'but a Bot that is gone is not added');
});

test('bot.json reads accept_from strictly', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Strict' });
  const read = (value: unknown) => parseRecord(withAcceptFrom(f.store, bot, value), bot.bot_id);
  assert.deepEqual(read([OTHER]).accept_from, [OTHER]);
  assert.equal('accept_from' in read([]), false, 'an empty list reads as none');
  for (const bad of ['Bot0000000000000000777', [1], ['x'], [OTHER, OTHER], [bot.bot_id], null, {}, Array.from({ length: MAX_ACCEPT_FROM + 1 }, (_, index) => `Bot${String(index).padStart(19, '0')}`)]) {
    assert.throws(() => read(bad), /invalid accept_from/, JSON.stringify(bad)?.slice(0, 60));
  }
  assert.throws(() => parseRecord(JSON.stringify({ ...raw(f.store, bot.bot_id), accept: [] }), bot.bot_id), /unsupported fields/);
});

test('a record with accept_from survives being read and written back', (t) => {
  const f = botFixture(t);
  const one = f.store.create({ name: 'One' });
  const target = f.store.create({ name: 'Target', acceptFrom: [one.bot_id] });
  writeFileSync(f.store.recordPath(target.bot_id), `${JSON.stringify(f.store.get(target.bot_id), null, 2)}\n`);
  const updated = f.store.update(target.bot_id, 1, { description: 'Changed' });
  assert.deepEqual(updated.accept_from, [one.bot_id]);
  assert.deepEqual(f.store.get(target.bot_id), updated);
});

function setupUi(t: { after(fn: () => void): void }) {
  const f = botFixture(t);
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes: { newSession: async (projectPath) => ({ projectPath, sessionId: '11111111-2222-4333-8444-555555555555' }), openSession: async () => ({}), closeProject: async () => undefined },
    catalogFor: () => ({ list: async () => ({ sessions: [] }), find: async () => undefined, workspaceSettings: async () => ({ environment: {}, configHome: '/unused' }), invalidate: () => undefined }),
  });
  const handlers = new BotsUi({ service, content: new BotContent(f.store), tasks: noTasks }).handlers();
  return { ...f, service, call: async (channel: string, ...args: unknown[]): Promise<any> => handlers[channel]!(...args) };
}

test('the window can set who may delegate to a Bot, and reads it back', async (t) => {
  const s = setupUi(t);
  const one = await s.call(CH_BOTS_CREATE, { name: 'One' });
  assert.equal('acceptFrom' in one, false, 'absent for a Bot that accepts none');
  const target = await s.call(CH_BOTS_CREATE, { name: 'Target', acceptFrom: [one.botId] });
  assert.deepEqual(target.acceptFrom, [one.botId]);
  const changed = await s.call(CH_BOTS_UPDATE, target.botId, target.revision, { acceptFrom: [] });
  assert.equal('acceptFrom' in changed, false);
  assert.equal(changed.revision, 2);
  const again = await s.call(CH_BOTS_UPDATE, target.botId, 2, { acceptFrom: [one.botId] });
  assert.deepEqual(again.acceptFrom, [one.botId]);
  assert.deepEqual(botUi(s.service.get(target.botId)).acceptFrom, [one.botId]);
});

test('what the window sends for acceptFrom is checked before it reaches a record', async (t) => {
  const s = setupUi(t);
  const target = await s.call(CH_BOTS_CREATE, { name: 'Target' });
  for (const bad of ['Bot0000000000000000001', [1], ['x'], null, {}, [[]]]) {
    await assert.rejects(s.call(CH_BOTS_UPDATE, target.botId, 1, { acceptFrom: bad }), /invalid accept list/);
    await assert.rejects(s.call(CH_BOTS_CREATE, { name: 'Bad', acceptFrom: bad }), /invalid accept list/);
  }
  await assert.rejects(s.call(CH_BOTS_UPDATE, target.botId, 1, { acceptFrom: [OTHER] }), /does not exist/);
  await assert.rejects(s.call(CH_BOTS_UPDATE, target.botId, 1, { accept_from: [] }), /unsupported fields/);
  assert.equal(s.service.get(target.botId).revision, 1);
});
