import { CH_BOTS_MARKET } from '../src/shared/botMarket.js';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { BotContent } from '../src/main/bots/content.js';
import { LoginItem } from '../src/main/bots/loginItem.js';
import { BotsUi, botUi, type BotsUiLoginItem, type BotsUiTasks } from '../src/main/bots/rendererIpc.js';
import { BotService } from '../src/main/bots/service.js';
import { BotConflictError, workspaceIdOf } from '../src/main/bots/store.js';
import { WorkspaceRequests } from '../src/main/bots/workspaceRequests.js';
import { HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { SettingsStore } from '../src/main/settings.js';
import {
  BOT_CONFLICT_MESSAGE, BOT_SANDBOX_UNAVAILABLE_MESSAGE, CH_BOTS_AVAILABILITY, CH_BOTS_CREATE, CH_BOTS_DELETE, CH_BOTS_GET, CH_BOTS_LIST, CH_BOTS_LOGIN_ITEM_GET, CH_BOTS_LOGIN_ITEM_SET,
  CH_BOTS_LOGIN_ITEMS_OPEN, CH_BOTS_MEMORY_DELETE, CH_BOTS_MEMORY_LIST, CH_BOTS_MEMORY_READ,
  CH_BOTS_MEMORY_WRITE, CH_BOTS_PERSONA_GET, CH_BOTS_PERSONA_SET, CH_BOTS_REACH_DATASET, CH_BOTS_REACH_LOCAL_APP, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSIONS_LIST, CH_BOTS_SKILL_DELETE, CH_BOTS_SKILL_IMPORT,
  CH_BOTS_SKILLS_IMPORTABLE, CH_BOTS_SKILLS_LIST, CH_BOTS_TASK_CANCEL, CH_BOTS_TASKS_LIST, CH_BOTS_UPDATE,
  CH_BOTS_WORKSPACE_REQUEST_DECIDE, CH_BOTS_WORKSPACE_REQUESTS_LIST,
  type BotUi,
} from '../src/shared/bots.js';
import { botFixture, noLoginItem, noTasks, SESSION } from './bot-helpers.js';

const ALL_CHANNELS = [
  CH_BOTS_MARKET,
  CH_BOTS_AVAILABILITY, CH_BOTS_LIST, CH_BOTS_GET, CH_BOTS_CREATE, CH_BOTS_UPDATE, CH_BOTS_DELETE, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSIONS_LIST,
  CH_BOTS_PERSONA_GET, CH_BOTS_PERSONA_SET, CH_BOTS_MEMORY_LIST, CH_BOTS_MEMORY_READ, CH_BOTS_MEMORY_WRITE, CH_BOTS_MEMORY_DELETE,
  CH_BOTS_SKILLS_LIST, CH_BOTS_SKILL_DELETE, CH_BOTS_SKILL_IMPORT, CH_BOTS_SKILLS_IMPORTABLE, CH_BOTS_TASKS_LIST, CH_BOTS_TASK_CANCEL,
  CH_BOTS_LOGIN_ITEM_GET, CH_BOTS_LOGIN_ITEM_SET, CH_BOTS_LOGIN_ITEMS_OPEN,
  CH_BOTS_WORKSPACE_REQUESTS_LIST, CH_BOTS_WORKSPACE_REQUEST_DECIDE,
  CH_BOTS_REACH_LOCAL_APP, CH_BOTS_REACH_DATASET,
];

/** What a Bot's service is given to run sessions with, where the test does not look at them. */
function fakeRuntimes(calls: unknown[][]) {
  return {
    newSession: async (projectPath: string, model: string | undefined, owner: { onAllocated?: (ref: { projectPath: string; sessionId: string }) => Promise<void> }) => {
      calls.push(['newSession', projectPath, model]);
      const ref = { projectPath, sessionId: SESSION };
      await owner.onAllocated?.(ref);
      return ref;
    },
    openSession: async (ref: unknown) => { calls.push(['openSession', ref]); return {}; },
    closeProject: async (projectPath: string) => { calls.push(['closeProject', projectPath]); },
    closeSession: async () => undefined,
    restart: async () => undefined,
    setSessionDirectoryGrants: () => undefined,
    runtimeSummaries: [],
  };
}

function setup(t: { after(fn: () => void): void }, tasks: BotsUiTasks = noTasks, loginItem: BotsUiLoginItem = noLoginItem) {
  const f = botFixture(t);
  const calls: unknown[][] = [];
  const service = new BotService({
    store: f.store,
    sandbox: () => ({ available: true }),
    runtimes: fakeRuntimes(calls),
    catalogFor: () => ({
      list: async () => ({ sessions: [] }),
      find: async () => undefined,
      select: async () => undefined,
      workspaceSettings: async () => ({ environment: {}, configHome: '/unused' }),
      invalidate: () => undefined,
    }),
  });
  const changed: string[][] = [];
  const workspaceRequests = new WorkspaceRequests({
    store: f.store, grant: (botId, input) => service.grantWorkspace(botId, input), usable: () => true, changed: (botIds) => { changed.push([...botIds]); },
  });
  const ui = new BotsUi({ service, content: new BotContent(f.store), tasks, loginItem, workspaceRequests });
  const handlers = ui.handlers();
  // What the window receives is whatever the handler returns, untyped on the wire.
  return { ...f, service, calls, handlers, workspaceRequests, changed, ui, call: async (channel: string, ...args: unknown[]): Promise<any> => handlers[channel]!(...args) };
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
    workspaces: [{ id: workspaceIdOf(folder), path: folder, mode: 'read', addedAt: bot.workspaces[0].addedAt }],
    sandbox: { enabled: true, allowedDomains: [] },
    scope: { localApps: [], datasets: [] },
    autoReply: { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' },
  });
  assert.equal(typeof bot.workspaces[0].addedAt, 'number');
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
  assert.deepEqual(await s.call(CH_BOTS_SESSION_OPEN, bot.botId), { botId: bot.botId, projectPath: s.store.workspaceDir(bot.botId), sessionId: SESSION, workspaceId: 'default' });
  assert.deepEqual(s.calls, [['newSession', s.store.workspaceDir(bot.botId), 'a/b']]);
  assert.deepEqual(await s.call(CH_BOTS_SESSIONS_LIST, bot.botId), { sessions: [] });
  await assert.rejects(s.call(CH_BOTS_SESSION_OPEN, bot.botId, SESSION), /source is unavailable/);
});

test('the host opens a Bot\'s session by the same checks the window\'s request goes through', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Hosted' });
  const ui = new BotsUi({ service: s.service, content: new BotContent(s.store), tasks: noTasks, loginItem: noLoginItem });
  assert.deepEqual(await ui.openSession(bot.botId), { botId: bot.botId, projectPath: s.store.workspaceDir(bot.botId), sessionId: SESSION, workspaceId: 'default' });
  await assert.rejects(ui.openSession('bad'), /invalid Bot id/);
  for (const bad of [5, null, {}, 'not-a-session', [SESSION]]) await assert.rejects(ui.openSession(bot.botId, bad), /invalid session id/);
  await assert.rejects(ui.openSession(bot.botId, SESSION), /source is unavailable/);
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

test('the window asks for the tasks of a Bot, and stops one, by ids that are checked first', async (t) => {
  const seen: unknown[][] = [];
  const s = setup(t, {
    forOwner: (botId) => { seen.push(['list', botId]); return { tasks: [], truncated: false, active: { received: 0, sent: 0 } }; },
    cancelForOwner: (botId, taskId) => { seen.push(['cancel', botId, taskId]); },
  });
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Busy' });
  const task = 'T'.repeat(22);
  assert.deepEqual(await s.call(CH_BOTS_TASKS_LIST, bot.botId), { tasks: [], truncated: false, active: { received: 0, sent: 0 } });
  assert.equal(await s.call(CH_BOTS_TASK_CANCEL, bot.botId, task), undefined);
  assert.deepEqual(seen, [['list', bot.botId], ['cancel', bot.botId, task]]);
  seen.length = 0;
  for (const bad of ['', 'short', '../../x', 'a'.repeat(23), 5, null, undefined, {}, [bot.botId]]) {
    await assert.rejects(s.call(CH_BOTS_TASKS_LIST, bad), /invalid Bot id/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_TASK_CANCEL, bad, task), /invalid Bot id/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_TASK_CANCEL, bot.botId, bad), /invalid task id/, JSON.stringify(bad));
  }
  assert.deepEqual(seen, [], 'nothing of the above reached the tasks');
});

test('the window reads the login item and turns it on or off with a true or a false, answered with how the system has it then', async (t) => {
  const seen: unknown[][] = [];
  const state = { enabled: false };
  const s = setup(t, noTasks, {
    status: () => ({ available: true, enabled: state.enabled, needsApproval: false }),
    set: (enabled) => { seen.push(['set', enabled]); state.enabled = enabled; },
    openSettings: async () => undefined,
  });
  assert.deepEqual(await s.call(CH_BOTS_LOGIN_ITEM_GET), { available: true, enabled: false, needsApproval: false });
  assert.deepEqual(await s.call(CH_BOTS_LOGIN_ITEM_SET, true), { available: true, enabled: true, needsApproval: false });
  assert.deepEqual(await s.call(CH_BOTS_LOGIN_ITEM_SET, false), { available: true, enabled: false, needsApproval: false });
  assert.deepEqual(seen, [['set', true], ['set', false]]);
  seen.length = 0;
  for (const bad of ['true', 'on', 1, 0, null, undefined, {}, [], [true], { enabled: true }]) {
    await assert.rejects(s.call(CH_BOTS_LOGIN_ITEM_SET, bad), /invalid login item setting/, JSON.stringify(bad) ?? 'undefined');
  }
  assert.deepEqual(seen, [], 'nothing of the above reached the system');
});

test('the window cannot say which place System Settings is opened at', async (t) => {
  const seen: unknown[][] = [];
  const s = setup(t, noTasks, {
    status: () => ({ available: true, enabled: true, needsApproval: true }),
    set: () => undefined,
    openSettings: async (...args: unknown[]) => { seen.push(args); },
  });
  assert.equal(await s.call(CH_BOTS_LOGIN_ITEMS_OPEN), undefined);
  assert.equal(await s.call(CH_BOTS_LOGIN_ITEMS_OPEN, 'https://attacker.example', 'file:///etc/passwd'), undefined);
  assert.deepEqual(seen, [[], []], 'whatever came with the request, the place is not the window\'s to choose');
});

test('an app that cannot be a login item refuses to become one when the window asks', async (t) => {
  const calls: unknown[] = [];
  const opened: string[] = [];
  const app = {
    isPackaged: false,
    getLoginItemSettings: () => { calls.push('get'); return { openAtLogin: false }; },
    setLoginItemSettings: (settings: unknown) => { calls.push(settings); },
  };
  const s = setup(t, noTasks, new LoginItem({ app, platform: 'darwin', openExternal: async (url) => { opened.push(url); } }));
  assert.deepEqual(await s.call(CH_BOTS_LOGIN_ITEM_GET), { available: false, enabled: false, needsApproval: false });
  await assert.rejects(s.call(CH_BOTS_LOGIN_ITEM_SET, true), /available in the installed app/);
  await assert.rejects(s.call(CH_BOTS_LOGIN_ITEMS_OPEN), /available in the installed app/);
  assert.deepEqual(calls, [], 'the system was neither read nor changed');
  assert.deepEqual(opened, []);
});

test('the host controller registers the handlers behind its sender check and removes them on dispose', async (t) => {
  const s = setup(t);
  const root = mkdtempSync(join(tmpdir(), 'lingxi-bots-ipc-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const registered = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => registered.set(name, handler), removeHandler: (name: string) => registered.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.attachBots(new BotsUi({ service: s.service, content: new BotContent(s.store), tasks: noTasks, loginItem: noLoginItem }));
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
  host.attachBots(new BotsUi({ service: s.service, content: new BotContent(s.store), tasks: noTasks, loginItem: noLoginItem }));
  for (const channel of ALL_CHANNELS) assert.ok(registered.has(channel), channel);
  host.dispose();
  assert.ok(![...registered.keys()].some((name) => name.startsWith('lingxi:bots:')));
});

// Availability

test('the answer to whether Bots can be used says whether this computer can give them a sandbox', async (t) => {
  const s = setup(t);
  assert.deepEqual(await s.call(CH_BOTS_AVAILABILITY), { available: true, sandbox: { available: true } });
  const f = botFixture(t);
  const service = new BotService({
    store: f.store, sandbox: () => ({ available: false, reason: 'bots_sandbox_why_platform' }),
    runtimes: { ...fakeRuntimes([]), newSession: async () => { throw new Error('unused'); } },
    catalogFor: () => ({ list: async () => ({ sessions: [] }), find: async () => undefined, select: async () => undefined, workspaceSettings: async () => ({ environment: {}, configHome: '/x' }), invalidate: () => undefined }),
  });
  const handlers = new BotsUi({ service, content: new BotContent(f.store), tasks: noTasks, loginItem: noLoginItem }).handlers();
  assert.deepEqual(await handlers[CH_BOTS_AVAILABILITY]!(), { available: true, sandbox: { available: false, reason: 'bots_sandbox_why_platform' } });
});

test('a Bot cannot be made with its sandbox on where the computer has none, and the window is told in the words it knows', async (t) => {
  const f = botFixture(t);
  const service = new BotService({
    store: f.store, sandbox: () => ({ available: false, reason: 'bots_sandbox_why_no_bwrap' }),
    runtimes: { ...fakeRuntimes([]), newSession: async () => { throw new Error('unused'); } },
    catalogFor: () => ({ list: async () => ({ sessions: [] }), find: async () => undefined, select: async () => undefined, workspaceSettings: async () => ({ environment: {}, configHome: '/x' }), invalidate: () => undefined }),
  });
  const handlers = new BotsUi({ service, content: new BotContent(f.store), tasks: noTasks, loginItem: noLoginItem }).handlers();
  await assert.rejects(Promise.resolve().then(() => handlers[CH_BOTS_CREATE]!({ name: 'Confined' })), new RegExp(BOT_SANDBOX_UNAVAILABLE_MESSAGE.replace('.', '\\.')));
  await assert.rejects(Promise.resolve().then(() => handlers[CH_BOTS_CREATE]!({ name: 'Confined', sandbox: { enabled: true, allowedDomains: [] } })), /isolate this Bot/);
  assert.deepEqual(readdirSync(f.store.root), [], 'and nothing was made');
  const free = await handlers[CH_BOTS_CREATE]!({ name: 'Free', sandbox: { enabled: false, allowedDomains: [] } }) as BotUi;
  assert.deepEqual(free.sandbox, { enabled: false, allowedDomains: [] }, 'the owner chose to go without, and has it');
});

// The mode, the activity and the skills to import

test('a Bot has a permission mode of its own, checked on the way in', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Moody', permissionMode: 'acceptEdits' });
  assert.equal(bot.permissionMode, 'acceptEdits');
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).permissionMode, 'acceptEdits');
  assert.ok(!('permissionMode' in await s.call(CH_BOTS_CREATE, { name: 'Plain' })));
  const planned = await s.call(CH_BOTS_UPDATE, bot.botId, 1, { permissionMode: 'plan' });
  assert.deepEqual([planned.permissionMode, planned.revision], ['plan', 2]);
  const cleared = await s.call(CH_BOTS_UPDATE, bot.botId, 2, { permissionMode: null });
  assert.ok(!('permissionMode' in cleared));
  assert.equal(JSON.parse(JSON.stringify(botUi(s.store.get(bot.botId)))).permissionMode, undefined);
  for (const bad of ['', 'yolo', 'Plan', 5, {}, [], false]) {
    await assert.rejects(s.call(CH_BOTS_CREATE, { name: 'Bad', permissionMode: bad }), /invalid permission mode/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 3, { permissionMode: bad }), /invalid permission mode/, JSON.stringify(bad));
  }
  await assert.rejects(s.call(CH_BOTS_CREATE, { name: 'Null', permissionMode: null }), /invalid permission mode/, 'a new Bot has nothing to clear');
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).revision, 3);
});

test('a Bot is reachable from the owner\'s devices only when its switch is on, which the window sets by an update and reads back', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Reachable' });
  assert.ok(!('remote' in bot), 'a new Bot is off, and the window is told nothing of it');
  const on = await s.call(CH_BOTS_UPDATE, bot.botId, 1, { remote: true });
  assert.deepEqual([on.remote, on.revision], [true, 2]);
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).remote, true);
  assert.equal((await s.call(CH_BOTS_LIST))[0].remote, true);
  assert.deepEqual(botUi(s.store.get(bot.botId)), on);
  assert.equal((await s.call(CH_BOTS_UPDATE, bot.botId, 2, { name: 'Reachable too' })).remote, true, 'a patch that does not mention it keeps it');
  const off = await s.call(CH_BOTS_UPDATE, bot.botId, 3, { remote: false });
  assert.ok(!('remote' in off), 'off is an absent key, as for the record');
  assert.equal(off.revision, 4);
  for (const bad of ['true', 'on', 1, 0, null, {}, []]) {
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 4, { remote: bad }), /invalid Remote Control setting/, JSON.stringify(bad));
  }
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).revision, 4, 'a refused value changes nothing');
  await assert.rejects(s.call(CH_BOTS_CREATE, { name: 'Eager', remote: true }), /unsupported fields/, 'a Bot is made off, and switched on afterwards');
  assert.deepEqual((await s.call(CH_BOTS_LIST)).map((entry: any) => entry.name), ['Reachable too'], 'and the refused one was not made');
});

test('the conflict a stale update causes is the one the window recognises by its text', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Raced' });
  await s.call(CH_BOTS_UPDATE, bot.botId, 1, { name: 'Won' });
  await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 1, { name: 'Lost' }), (error: Error) => error.message === BOT_CONFLICT_MESSAGE);
});

test('a Bot shows when it was last used, once it has been', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Busy' });
  assert.ok(!('lastActivityAt' in bot));
  mkdirSync(s.store.transcriptsDir(bot.botId), { recursive: true });
  const transcript = join(s.store.transcriptsDir(bot.botId), `${SESSION}.jsonl`);
  writeFileSync(transcript, '{}\n');
  utimesSync(transcript, 1_700_000_000, 1_700_000_000);
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).lastActivityAt, 1_700_000_000_000);
  assert.equal((await s.call(CH_BOTS_LIST))[0].lastActivityAt, 1_700_000_000_000);
  assert.equal((await s.call(CH_BOTS_UPDATE, bot.botId, 1, { name: 'Busier' })).lastActivityAt, 1_700_000_000_000, 'every Bot the window gets says so');
});

test('the owner\'s skills can be listed, and one of them imported', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Learner' });
  assert.deepEqual(await s.call(CH_BOTS_SKILLS_IMPORTABLE), []);
  const skill = join(s.defaultConfigHome, 'skills', 'deploy');
  mkdirSync(skill, { recursive: true });
  writeFileSync(join(skill, 'SKILL.md'), '---\ndescription: Ships it\n---\nSteps.\n');
  assert.deepEqual(await s.call(CH_BOTS_SKILLS_IMPORTABLE), [{ name: 'deploy', description: 'Ships it' }]);
  assert.deepEqual(await s.call(CH_BOTS_SKILL_IMPORT, bot.botId, 'deploy'), { name: 'deploy', description: 'Ships it' });
  assert.deepEqual(await s.call(CH_BOTS_SKILLS_LIST, bot.botId), [{ name: 'deploy', description: 'Ships it' }]);
});

// Several folders, and the ones a Bot asks for

test('the window gives a folder a label, and the folders it sends again keep their ids and when they were added', async (t) => {
  const s = setup(t);
  const one = s.folder('one');
  const two = s.folder('two');
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Labelled', workspaces: [{ path: one, mode: 'read', label: ' The repo ' }] });
  assert.equal(bot.workspaces[0].label, 'The repo');
  assert.equal(bot.workspaces[0].id, workspaceIdOf(one));
  const updated = await s.call(CH_BOTS_UPDATE, bot.botId, 1, { workspaces: [{ path: one, mode: 'write', label: 'Renamed' }, { path: two, mode: 'read' }] });
  assert.deepEqual(updated.workspaces.map((workspace: any) => [workspace.id, workspace.path, workspace.mode, workspace.label]), [
    [workspaceIdOf(one), one, 'write', 'Renamed'], [workspaceIdOf(two), two, 'read', undefined],
  ]);
  assert.equal(updated.workspaces[0].addedAt, bot.workspaces[0].addedAt, 'the same folder, whatever was changed of it');
  const left = await s.call(CH_BOTS_UPDATE, bot.botId, 2, { workspaces: [{ path: two, mode: 'read' }] });
  assert.deepEqual(left.workspaces.map((workspace: any) => workspace.path), [two]);
  for (const bad of [{ path: one, mode: 'read', label: 5 }, { path: one, mode: 'read', label: '' }, { path: one, mode: 'read', id: 'wSomething' }, { path: one, mode: 'read', addedAt: 1 }]) {
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 3, { workspaces: [bad] }), /invalid|unsupported/, JSON.stringify(bad));
  }
});

test('the sandbox, the scope and the automatic replies go through whole, and are checked by the store, not by guesswork here', async (t) => {
  const s = setup(t);
  const sandbox = { enabled: false, allowedDomains: ['example.com'] };
  const scope = { localApps: ['app-one'], datasets: [{ source: 'warehouse', dataset: 'orders', write: false, reportReview: true }] };
  const autoReply = { policy: 'Answer politely.', keywords: ['refund'], cooldownSec: 30, thresholds: { decline: 0.2, run: 0.9 }, approval: 'uncertain' as const };
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Configured', sandbox, scope, autoReply });
  assert.deepEqual([bot.sandbox, bot.scope, bot.autoReply], [sandbox, scope, autoReply]);
  const changed = await s.call(CH_BOTS_UPDATE, bot.botId, 1, { sandbox: { enabled: true, allowedDomains: [] }, autoReply: { ...autoReply, quietHours: { start: '22:00', end: '07:00', timeZone: 'Europe/Paris' } } });
  assert.deepEqual(changed.sandbox, { enabled: true, allowedDomains: [] });
  assert.deepEqual(changed.scope, scope, 'what the patch does not name stays');
  assert.deepEqual(changed.autoReply.quietHours, { start: '22:00', end: '07:00', timeZone: 'Europe/Paris' });
  assert.deepEqual(botUi(s.store.get(bot.botId)), changed);
  for (const bad of ['text', 5, [], null]) {
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 2, { sandbox: bad }), /invalid sandbox/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 2, { scope: bad }), /invalid scope/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 2, { autoReply: bad }), /invalid automatic reply settings/, JSON.stringify(bad));
  }
  await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 2, { sandbox: { enabled: 'yes', allowedDomains: [] } }), /invalid/);
  await assert.rejects(s.call(CH_BOTS_UPDATE, bot.botId, 2, { autoReply: { ...autoReply, approval: 'sometimes' } }), /invalid/);
  assert.equal((await s.call(CH_BOTS_GET, bot.botId)).revision, 2, 'nothing above changed the Bot');
});

test('the pages of a Local App and of a data source ask which Bots reach them, and are told the names of those and nothing of their scope', async (t) => {
  const s = setup(t);
  const app = 'app-one';
  const order = { source: 'warehouse', dataset: 'orders', write: false, reportReview: false };
  const first = await s.call(CH_BOTS_CREATE, { name: 'First', scope: { localApps: [app], datasets: [order] } });
  const second = await s.call(CH_BOTS_CREATE, { name: 'Second', scope: { localApps: ['app-two'], datasets: [{ ...order, dataset: 'returns', write: true, reportReview: true }] } });
  await s.call(CH_BOTS_CREATE, { name: 'Neither' });
  const third = await s.call(CH_BOTS_CREATE, { name: 'Third', scope: { localApps: [app, 'app-two'], datasets: [{ source: 'crm', dataset: 'orders', write: false, reportReview: false }] } });
  const names = (list: Array<{ botId: string; name: string }>) => list.map((bot) => bot.name);
  assert.deepEqual(await s.call(CH_BOTS_REACH_LOCAL_APP, app), [{ botId: first.botId, name: 'First' }, { botId: third.botId, name: 'Third' }], 'only the id and the name, oldest first');
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_LOCAL_APP, 'app-two')), ['Second', 'Third']);
  assert.deepEqual(await s.call(CH_BOTS_REACH_LOCAL_APP, 'app-nobody'), []);
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_DATASET, 'warehouse')), ['First', 'Second'], 'any dataset of the source');
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_DATASET, 'warehouse', 'orders')), ['First']);
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_DATASET, 'warehouse', 'returns')), ['Second']);
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_DATASET, 'crm', 'orders')), ['Third'], 'a dataset of the same name in another source is another dataset');
  assert.deepEqual(await s.call(CH_BOTS_REACH_DATASET, 'warehouse', 'invoices'), []);
  // A Bot whose record is damaged reaches nothing, and does not keep the others from being found.
  writeFileSync(join(s.store.botDir(second.botId), 'bot.json'), '{ damaged');
  assert.deepEqual(names(await s.call(CH_BOTS_REACH_LOCAL_APP, 'app-two')), ['Third']);
  for (const bad of [undefined, '', 5, null, {}, 'x'.repeat(129), 'a\nb']) {
    await assert.rejects(s.call(CH_BOTS_REACH_LOCAL_APP, bad), /invalid Local App id/, JSON.stringify(bad));
    await assert.rejects(s.call(CH_BOTS_REACH_DATASET, bad), /invalid data source/, JSON.stringify(bad));
  }
  for (const bad of ['', 5, null, {}, 'x'.repeat(129)]) await assert.rejects(s.call(CH_BOTS_REACH_DATASET, 'warehouse', bad), /invalid dataset/, JSON.stringify(bad));
});

test('a chat is started in the folder the window names, and a name that is no folder id is refused before the Bot is asked', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Wide', workspaces: [{ path: repo, mode: 'write' }] });
  const opened = await s.call(CH_BOTS_SESSION_OPEN, bot.botId, undefined, undefined, workspaceIdOf(repo));
  assert.deepEqual(opened, { botId: bot.botId, projectPath: repo, sessionId: SESSION, workspaceId: workspaceIdOf(repo) });
  assert.deepEqual(s.calls, [['newSession', repo, undefined]]);
  s.calls.length = 0;
  for (const bad of ['', 5, null, {}, '../x', 'a/b', 'x'.repeat(33)]) {
    await assert.rejects(s.call(CH_BOTS_SESSION_OPEN, bot.botId, undefined, undefined, bad), /invalid folder/, JSON.stringify(bad));
  }
  await assert.rejects(s.call(CH_BOTS_SESSION_OPEN, bot.botId, undefined, undefined, 'wNoSuchFold'), /no such folder/);
  assert.deepEqual(s.calls, []);
  assert.equal(s.ui.botIdOfSession({ projectPath: repo, sessionId: SESSION }), bot.botId);
  assert.equal(s.ui.workspaceIdAt(bot.botId, repo), workspaceIdOf(repo));
  assert.deepEqual(s.ui.workspacePathsOf(bot.botId), [s.store.workspaceDir(bot.botId), repo]);
  assert.deepEqual(s.ui.workspacePathsOf('bad'), []);
});

test('the window adds a folder the owner picked to a Bot as one to read, and the Bot is not asked for more than that', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Picker' });
  const repo = s.folder('repo');
  const added = await s.ui.addWorkspace(bot.botId, repo);
  assert.deepEqual(added.workspaces.map((workspace) => [workspace.path, workspace.mode]), [[repo, 'read']]);
  assert.equal((await s.ui.addWorkspace(bot.botId, s.folder('more'), 'write')).workspaces[1]!.mode, 'write');
  await assert.rejects(s.ui.addWorkspace('bad', repo), /invalid Bot id/);
  await assert.rejects(s.ui.addWorkspace(bot.botId, repo), /already/);
});

test('what the Bots asked for is listed with their names, and the owner answers by Bot, request and decision', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Asker' });
  const other = await s.call(CH_BOTS_CREATE, { name: 'Other' });
  const repo = s.folder('repo');
  const notes = s.folder('notes');
  const caller = { botId: bot.botId, sessionId: SESSION };
  const signal = new AbortController().signal;
  const first = await s.workspaceRequests.request(caller, { path: repo, mode: 'write', reason: 'To change it', waitSeconds: 0 }, signal);
  await s.workspaceRequests.request({ botId: other.botId, sessionId: SESSION }, { path: notes, mode: 'read', reason: 'To read', waitSeconds: 0 }, signal);
  const listed = await s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST);
  assert.deepEqual(listed.map((request: any) => [request.botName, request.path, request.mode, request.reason]), [['Asker', repo, 'write', 'To change it'], ['Other', notes, 'read', 'To read']]);
  assert.deepEqual(Object.keys(listed[0]).sort(), ['botId', 'botName', 'createdAt', 'mode', 'path', 'reason', 'requestId']);
  assert.deepEqual((await s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST, bot.botId)).map((request: any) => request.requestId), [first.request_id], 'of one Bot');
  const approved = await s.call(CH_BOTS_WORKSPACE_REQUEST_DECIDE, bot.botId, first.request_id, 'approve');
  assert.deepEqual(approved.workspaces.map((workspace: any) => [workspace.path, workspace.mode]), [[repo, 'write']], 'the Bot has the folder, as asked');
  assert.equal((await s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST, bot.botId)).length, 0);
  const second = (await s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST, other.botId))[0];
  const denied = await s.call(CH_BOTS_WORKSPACE_REQUEST_DECIDE, other.botId, second.requestId, 'deny');
  assert.deepEqual(denied.workspaces, []);
  assert.deepEqual(await s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST), []);
  assert.ok(s.changed.length >= 4, 'the window was told of each change');
});

test('an answer to a request is checked before it reaches the requests, and a window without them has none', async (t) => {
  const s = setup(t);
  const bot = await s.call(CH_BOTS_CREATE, { name: 'Checked' });
  const { request_id } = await s.workspaceRequests.request({ botId: bot.botId, sessionId: SESSION }, { path: s.folder('repo'), mode: 'read', reason: 'x', waitSeconds: 0 }, new AbortController().signal);
  for (const bad of ['', 5, null, undefined, {}, 'x'.repeat(65)]) await assert.rejects(s.call(CH_BOTS_WORKSPACE_REQUEST_DECIDE, bot.botId, bad, 'approve'), /invalid request id/, JSON.stringify(bad));
  for (const bad of ['', 'yes', 'APPROVE', 5, null, undefined, {}]) await assert.rejects(s.call(CH_BOTS_WORKSPACE_REQUEST_DECIDE, bot.botId, request_id, bad), /invalid decision/, JSON.stringify(bad));
  for (const bad of ['', 'short', '../../x', 5, null, undefined]) await assert.rejects(s.call(CH_BOTS_WORKSPACE_REQUEST_DECIDE, bad, request_id, 'approve'), /invalid Bot id/, JSON.stringify(bad));
  await assert.rejects(s.call(CH_BOTS_WORKSPACE_REQUESTS_LIST, 'bad'), /invalid Bot id/);
  assert.equal(s.workspaceRequests.pending().length, 1, 'nothing above answered it');
  const bare = new BotsUi({ service: s.service, content: new BotContent(s.store), tasks: noTasks, loginItem: noLoginItem }).handlers();
  assert.deepEqual(await bare[CH_BOTS_WORKSPACE_REQUESTS_LIST]!(), []);
  await assert.rejects(Promise.resolve().then(() => bare[CH_BOTS_WORKSPACE_REQUEST_DECIDE]!(bot.botId, request_id, 'approve')), /unavailable/);
});
