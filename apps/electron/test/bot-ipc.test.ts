import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { BotContent } from '../src/main/bots/content.js';
import { BotsUi, botUi } from '../src/main/bots/rendererIpc.js';
import { BotService } from '../src/main/bots/service.js';
import { BotConflictError } from '../src/main/bots/store.js';
import { HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { SettingsStore } from '../src/main/settings.js';
import {
  CH_BOTS_CREATE, CH_BOTS_DELETE, CH_BOTS_GET, CH_BOTS_LIST, CH_BOTS_MEMORY_DELETE, CH_BOTS_MEMORY_LIST, CH_BOTS_MEMORY_READ, CH_BOTS_MEMORY_WRITE,
  CH_BOTS_PERSONA_GET, CH_BOTS_PERSONA_SET, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSIONS_LIST, CH_BOTS_SKILL_DELETE, CH_BOTS_SKILL_IMPORT, CH_BOTS_SKILLS_LIST, CH_BOTS_UPDATE,
} from '../src/shared/bots.js';
import { botFixture, SESSION } from './bot-helpers.js';

const ALL_CHANNELS = [
  CH_BOTS_LIST, CH_BOTS_GET, CH_BOTS_CREATE, CH_BOTS_UPDATE, CH_BOTS_DELETE, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSIONS_LIST,
  CH_BOTS_PERSONA_GET, CH_BOTS_PERSONA_SET, CH_BOTS_MEMORY_LIST, CH_BOTS_MEMORY_READ, CH_BOTS_MEMORY_WRITE, CH_BOTS_MEMORY_DELETE,
  CH_BOTS_SKILLS_LIST, CH_BOTS_SKILL_DELETE, CH_BOTS_SKILL_IMPORT,
];

function setup(t: { after(fn: () => void): void }) {
  const f = botFixture(t);
  const calls: unknown[][] = [];
  const service = new BotService({
    store: f.store,
    runtimes: {
      newSession: async (projectPath, model) => { calls.push(['newSession', projectPath, model]); return { projectPath, sessionId: SESSION }; },
      openSession: async (ref) => { calls.push(['openSession', ref]); return {}; },
      closeProject: async (projectPath) => { calls.push(['closeProject', projectPath]); },
    },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }),
      find: async () => undefined,
      workspaceSettings: async () => ({ environment: {}, configHome: '/unused' }),
    }),
  });
  const ui = new BotsUi({ service, content: new BotContent(f.store) });
  const handlers = ui.handlers();
  // What the window receives is whatever the handler returns, untyped on the wire.
  return { ...f, service, calls, handlers, call: async (channel: string, ...args: unknown[]): Promise<any> => handlers[channel]!(...args) };
}

test('every channel has a handler, and nothing else does', (t) => {
  const s = setup(t);
  assert.deepEqual(Object.keys(s.handlers).sort(), [...ALL_CHANNELS].sort());
  assert.equal(new Set(ALL_CHANNELS).size, ALL_CHANNELS.length);
  assert.ok(ALL_CHANNELS.every((channel) => channel.startsWith('lingxi:bots:')));
});

test('a Bot reaches the window as plain data: no record fields, no audiences, no paths but the bound folders', async (t) => {
  const s = setup(t);
  const folder = s.folder('repo');
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Window', description: 'Seen', persona: 'Hi', model: 'a/b', workspaces: [{ path: folder, mode: 'read' }] });
  assert.deepEqual(bot, {
    botId: bot.botId, name: 'Window', description: 'Seen', createdAt: bot.createdAt, updatedAt: bot.createdAt, revision: 1, model: 'a/b',
    workspaces: [{ path: folder, mode: 'read' }],
  });
  assert.match(bot.botId, /^[A-Za-z0-9_-]{22}$/);
  assert.deepEqual(await s.call(CH_BOTS_LIST), [bot]);
  assert.deepEqual(await s.call(CH_BOTS_GET, bot.botId), bot);
  assert.equal(JSON.stringify(bot).includes('audiences'), false);
  assert.equal(JSON.stringify(bot).includes(s.userData), false);
  const bare = await s.call(CH_BOTS_CREATE, { name: 'Bare' });
  assert.ok(!('model' in bare));
  assert.deepEqual(bare.workspaces, []);
  assert.deepEqual(botUi(s.store.get(bare.botId)), bare);
});

test('updating takes the revision and a patch, and a stale revision comes back as a conflict', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Original', model: 'a/b' });
  const updated = await s.call(CH_BOTS_UPDATE, bot.botId, 1, { name: 'Renamed', description: 'new', model: null, workspaces: [] });
  assert.equal(updated.revision, 2);
  assert.equal(updated.name, 'Renamed');
  assert.ok(!('model' in updated));
  await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 1, { name: 'Stale' }), BotConflictError);
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).name, 'Renamed');
});

test('a session is opened by Bot id, optionally with a session id, and listed', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Sessions', model: 'a/b' });
  assert.deepEqual(await s.call(CH_BOTS_SESSION_OPEN, bot.botId), { botId: bot.botId, projectPath: s.store.workspaceDir(bot.botId), sessionId: SESSION });
  assert.deepEqual(s.calls, [['newSession', s.store.workspaceDir(bot.botId), 'a/b']]);
  assert.deepEqual(await s.call(CH_BOTS_SESSIONS_LIST, bot.botId), { sessions: [] });
  await assert.rejects(s.call(CH_BOTS_SESSION_OPEN, bot.botId, SESSION), /does not belong to this Bot/);
});

test('persona, memory and skills go through to the Bot\'s files', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Files', persona: 'First.' });
  assert.equal(await s.call(CH_BOTS_PERSONA_GET, bot.botId), 'First.');
  await s.call(CH_BOTS_PERSONA_SET, bot.botId, 'Second.');
  assert.equal(await s.call(CH_BOTS_PERSONA_GET, bot.botId), 'Second.');
  assert.equal(await s.call(CH_BOTS_MEMORY_WRITE, bot.botId, 'fact.md', '---\ndescription: A fact\n---\nbody'), undefined);
  assert.deepEqual((await s.call(CH_BOTS_MEMORY_LIST, bot.botId)).map((entry: any) => [entry.name, entry.description]), [['fact.md', 'A fact']]);
  assert.match(await s.call(CH_BOTS_MEMORY_READ, bot.botId, 'fact.md'), /body/);
  await s.call(CH_BOTS_MEMORY_DELETE, bot.botId, 'fact.md');
  assert.deepEqual(await s.call(CH_BOTS_MEMORY_LIST, bot.botId), []);
  assert.deepEqual(await s.call(CH_BOTS_SKILLS_LIST, bot.botId), []);
  await assert.rejects(s.call(CH_BOTS_SKILL_IMPORT, bot.botId, 'absent'), /skill not found/);
  await assert.rejects(s.call(CH_BOTS_SKILL_DELETE, bot.botId, 'absent'), /skill not found/);
});

test('deleting a Bot through the window removes it from the list', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Short lived' });
  assert.equal(await s.call(CH_BOTS_DELETE, bot.botId), undefined);
  assert.deepEqual(await s.call(CH_BOTS_LIST), []);
  assert.deepEqual(s.calls, [['closeProject', s.store.workspaceDir(bot.botId)]]);
});

test('what the window sends is checked before it reaches a Bot', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Guarded' });
  const id = bot.botId;
  const badIds = ['', 'short', '../../x', 'a'.repeat(23), 5, null, undefined, {}, [id]];
  for (const channel of [CH_BOTS_GET, CH_BOTS_DELETE, CH_BOTS_SESSIONS_LIST, CH_BOTS_PERSONA_GET, CH_BOTS_MEMORY_LIST, CH_BOTS_SKILLS_LIST, CH_BOTS_SESSION_OPEN]) {
    for (const bad of badIds) await assert.rejects(s.call(channel, bad), /invalid Bot id/, `${channel} ${JSON.stringify(bad)}`);
  }
  await assert.rejects(s.call(CH_BOTS_UPDATE, 'bad', 1, {}), /invalid Bot id/);
  await assert.rejects(s.call(CH_BOTS_PERSONA_SET, 'bad', 'x'), /invalid Bot id/);
  await assert.rejects(s.call(CH_BOTS_MEMORY_READ, 'bad', 'x.md'), /invalid Bot id/);

  for (const request of [undefined, null, 'name', [], [{ name: 'x' }], {}, { name: 5 }, { name: 'x', extra: 1 }, { name: 'x', description: 1 }, { name: 'x', persona: {} },
    { name: 'x', model: 5 }, { name: 'x', workspaces: 'repo' }, { name: 'x', workspaces: [null] }, { name: 'x', workspaces: [{ path: '/tmp', mode: 'root' }] },
    { name: 'x', workspaces: [{ path: 5, mode: 'read' }] }, { name: 'x', workspaces: [{ path: '/tmp', mode: 'read', extra: 1 }] }, JSON.parse('{"name":"x","__proto__":{"admin":true}}')]) {
    await assert.rejects(s.call(CH_BOTS_CREATE, request), /invalid|unsupported/, JSON.stringify(request));
  }
  for (const revision of [0, -1, 1.5, '1', null, undefined, NaN, 2 ** 60]) await assert.rejects(s.call(CH_BOTS_UPDATE, id, revision, { name: 'y' }), /invalid revision/);
  for (const patch of [undefined, null, 'x', [], { extra: true }, { name: 5 }, { description: null }, { model: 5 }, { workspaces: {} }, { workspaces: [{ path: '/tmp' }] }]) {
    await assert.rejects(s.call(CH_BOTS_UPDATE, id, 1, patch), /invalid|unsupported/, JSON.stringify(patch));
  }
  for (const bad of [5, null, {}, 'not-a-session', 'x'.repeat(36), [SESSION]]) await assert.rejects(s.call(CH_BOTS_SESSION_OPEN, id, bad), /invalid session id/);

  for (const bad of [5, null, undefined, {}, ['x']]) {
    await assert.rejects(s.call(CH_BOTS_PERSONA_SET, id, bad), /invalid persona/);
    await assert.rejects(s.call(CH_BOTS_MEMORY_WRITE, id, 'ok.md', bad), /invalid memory content/);
  }
  for (const name of ['../x.md', 'a/b.md', 'x.txt', '', 5, null, undefined]) {
    for (const channel of [CH_BOTS_MEMORY_READ, CH_BOTS_MEMORY_DELETE]) await assert.rejects(s.call(channel, id, name), /invalid memory name/);
    await assert.rejects(s.call(CH_BOTS_MEMORY_WRITE, id, name, 'x'), /invalid memory name/);
  }
  for (const name of ['../x', '..', 'a/b', '', '.hidden', 5, null, undefined]) {
    for (const channel of [CH_BOTS_SKILL_DELETE, CH_BOTS_SKILL_IMPORT]) await assert.rejects(s.call(channel, id, name), /invalid skill name/);
  }
  assert.equal((await s.call(CH_BOTS_GET, id)).revision, 1, 'nothing above changed the Bot');
  assert.deepEqual(readdirSync(s.store.root), [id], 'and nothing above made another');
  assert.deepEqual(s.calls, []);
});

test('the host controller registers the handlers behind its sender check and removes them on dispose', async (t) => {
  const s = setup(t);
  const root = mkdtempSync(join(tmpdir(), 'lingxi-bots-ipc-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const registered = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => registered.set(name, handler), removeHandler: (name: string) => registered.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.attachBots(new BotsUi({ service: s.service, content: new BotContent(s.store) }));
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  try {
    for (const channel of ALL_CHANNELS) assert.ok(registered.has(channel), channel);
    const create = registered.get(CH_BOTS_CREATE)!;
    assert.throws(() => create({ sender, senderFrame: { url: frame.url } }, { name: 'Intruder' }), /unauthorized/);
    assert.throws(() => create({ sender: { mainFrame: frame }, senderFrame: frame }, { name: 'Intruder' }), /unauthorized/);
    assert.deepEqual(s.service.list(), [], 'a refused sender creates nothing');
    assert.equal((await create({ sender, senderFrame: frame }, { name: 'Welcome' })).name, 'Welcome');
    assert.equal((await registered.get(CH_BOTS_LIST)!({ sender, senderFrame: frame })).length, 1);
    frame.url = 'https://attacker.example';
    assert.throws(() => registered.get(CH_BOTS_LIST)!({ sender, senderFrame: frame }), /unauthorized/);
  } finally {
    host.dispose();
  }
  assert.ok(![...registered.keys()].some((name) => name.startsWith('lingxi:bots:')));
});

test('Bots attached after the handlers were registered are registered at once', (t) => {
  const s = setup(t);
  const root = mkdtempSync(join(tmpdir(), 'lingxi-bots-ipc-late-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const registered = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => registered.set(name, handler), removeHandler: (name: string) => registered.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.registerIpc();
  assert.ok(![...registered.keys()].some((name) => name.startsWith('lingxi:bots:')));
  host.attachBots(new BotsUi({ service: s.service, content: new BotContent(s.store) }));
  for (const channel of ALL_CHANNELS) assert.ok(registered.has(channel), channel);
  host.dispose();
  assert.ok(![...registered.keys()].some((name) => name.startsWith('lingxi:bots:')));
});
