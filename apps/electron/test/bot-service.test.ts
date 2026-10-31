import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { pathRule } from '../src/main/bots/botSettings.js';
import { BotService, botsAvailability, type BotCatalog, type BotRuntimes, type BotServiceOptions } from '../src/main/bots/service.js';
import { BotConflictError, BotNotFoundError, BotStore } from '../src/main/bots/store.js';
import type { ProjectSessionCatalogRow } from '../src/main/session-catalog.js';
import { botFixture, SESSION } from './bot-helpers.js';

const OTHER_SESSION = '99999999-2222-4333-8444-555555555555';
/** The hidden session that watches the clock for a Bot's routines. */
const CONTROLLER = '33333333-2222-4333-8444-555555555555';

function row(uuid: string, extra: Partial<ProjectSessionCatalogRow> = {}): ProjectSessionCatalogRow {
  return {
    uuid, title: `Session ${uuid.slice(0, 4)}`, modified_rfc3339: '2026-10-26T00:00:00Z', message_count: 2, mode: 'code', path: `/transcripts/${uuid}.jsonl`,
    empty_session: false, ...extra,
  };
}

function setup(t: { after(fn: () => void): void }, options: Pick<BotServiceOptions, 'openProject' | 'changed'> = {}) {
  const f = botFixture(t);
  const calls: unknown[][] = [];
  const warnings: string[] = [];
  let activeWork = false;
  let hold: Promise<void> | undefined;
  const controllers = new Set<string>();
  const runtimes: BotRuntimes = {
    newSession: async (projectPath, model) => { calls.push(['newSession', projectPath, model]); return { projectPath, sessionId: SESSION }; },
    openSession: async (ref, empty, resumeModel) => { calls.push(['openSession', ref, empty, resumeModel]); return {}; },
    closeProject: async (projectPath) => {
      calls.push(['closeProject', projectPath]);
      await hold;
      if (activeWork) throw new Error('cancel active turns and pending interactions before removing a project');
    },
  };
  const catalogs = new Map<string, { configHome: string; sessions: ProjectSessionCatalogRow[]; environment: Record<string, string> }>();
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes,
    catalogFor: (configHome): BotCatalog => {
      const state = { configHome, sessions: [] as ProjectSessionCatalogRow[], environment: { FROM_SETTINGS: 'yes' } as Record<string, string> };
      catalogs.set(configHome, state);
      return {
        list: async (projectPath) => { calls.push(['list', configHome, projectPath]); return { sessions: state.sessions }; },
        find: async (projectPath, sessionId) => { calls.push(['find', configHome, projectPath, sessionId]); return state.sessions.find((session) => session.uuid === sessionId.toLowerCase()); },
        workspaceSettings: async (projectPath) => {
          calls.push(['workspaceSettings', configHome, projectPath]);
          return { environment: state.environment, configHome, globalConfigPath: join(configHome, '.lingxi.json') };
        },
        invalidate: () => { calls.push(['invalidate', configHome]); },
      };
    },
    routines: {
      isControllerSession: (workspace, sessionId) => controllers.has(`${workspace}:${sessionId}`),
      releaseScope: (workspace) => { calls.push(['releaseScope', workspace]); },
    },
    warn: (message) => warnings.push(message),
    ...options,
  });
  return {
    ...f, service, calls, warnings, catalogs,
    /** Make a session the controller of a Bot's routines. */
    controllerOf(workspace: string, sessionId: string) { controllers.add(`${workspace}:${sessionId}`); },
    setActiveWork(value: boolean) { activeWork = value; },
    holdClose(promise: Promise<void>) { hold = promise; },
    settingsOf: (botId: string) => JSON.parse(readFileSync(join(f.store.homeDir(botId), 'settings.json'), 'utf8')),
  };
}

test('making a Bot writes its settings, and every other Bot is told to keep out of it', (t) => {
  const s = setup(t);
  const first = s.service.create({ name: 'First' });
  assert.ok(existsSync(join(s.store.homeDir(first.bot_id), 'settings.json')));
  const second = s.service.create({ name: 'Second' });
  assert.ok(s.settingsOf(first.bot_id).permissions.deny.includes(pathRule('Read', s.store.botDir(second.bot_id))));
  assert.ok(s.settingsOf(second.bot_id).permissions.deny.includes(pathRule('Read', s.store.botDir(first.bot_id))));
  assert.deepEqual(s.service.list().map((bot) => bot.name), ['First', 'Second']);
  assert.equal(s.service.get(second.bot_id).name, 'Second');
  assert.deepEqual(s.warnings, []);
});

test('a Bot whose bound folder cannot be used is refused before anything is made', (t) => {
  const s = setup(t);
  assert.throws(() => s.service.create({ name: 'Home', workspaces: [{ path: s.home, mode: 'write' }] }), /home folder/);
  assert.deepEqual(readdirSync(s.store.root), []);
});

test('changing a Bot rewrites its settings, and a stale revision is a conflict', (t) => {
  const s = setup(t);
  const one = s.folder('one');
  const bot = s.service.create({ name: 'Changing', workspaces: [{ path: one, mode: 'write' }] });
  assert.deepEqual(s.settingsOf(bot.bot_id).permissions.additionalDirectories, [one]);
  const updated = s.service.update(bot.bot_id, 1, { workspaces: [{ path: one, mode: 'read' }], model: 'a/b' });
  assert.equal(updated.revision, 2);
  const settings = s.settingsOf(bot.bot_id);
  assert.ok(settings.permissions.deny.includes(pathRule('Edit', one)));
  assert.equal(settings.model, 'a/b');
  assert.throws(() => s.service.update(bot.bot_id, 1, { name: 'Stale' }), BotConflictError);
  assert.equal(s.service.get(bot.bot_id).name, 'Changing');
  assert.throws(() => s.service.update('Bot0000000000000000999', 1, { name: 'Nobody' }), BotNotFoundError);
});

test('a session of a Bot is launched in its workspace, with its home, its folders and nothing of the owner\'s settings', async (t) => {
  const s = setup(t);
  const one = s.folder('one');
  const two = s.folder('two');
  const bot = s.service.create({ name: 'Launched', workspaces: [{ path: one, mode: 'write' }, { path: two, mode: 'read' }] });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const home = s.store.homeDir(bot.bot_id);
  const launch = await s.service.launchSettings(workspace);
  assert.deepEqual(launch, {
    environment: { FROM_SETTINGS: 'yes', LINGXI_CONFIG_DIR: home },
    configHome: home,
    globalConfigPath: join(home, '.lingxi.json'),
    additionalDirectories: [one, two],
  });
  assert.deepEqual(s.calls.filter((call) => call[0] === 'workspaceSettings'), [['workspaceSettings', home, workspace]], 'the settings are read from the Bot\'s home');
  assert.deepEqual([...s.catalogs.keys()], [home], 'the catalog reads the Bot\'s home and no other');
  assert.notEqual(home, s.defaultConfigHome);
});

test('the Bot home wins over a settings file that names another configuration home', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Anchored' });
  const home = s.store.homeDir(bot.bot_id);
  await s.service.launchSettings(s.store.workspaceDir(bot.bot_id));
  s.catalogs.get(home)!.environment = { LINGXI_CONFIG_DIR: s.defaultConfigHome, KEPT: '1' };
  const launch = await s.service.launchSettings(s.store.workspaceDir(bot.bot_id));
  assert.equal(launch.environment['LINGXI_CONFIG_DIR'], home);
  assert.equal(launch.environment['KEPT'], '1');
});

test('the settings are made current at every launch, so the isolation is that of now', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Relaunched' });
  const settingsFile = join(s.store.homeDir(bot.bot_id), 'settings.json');
  rmSync(settingsFile);
  await s.service.launchSettings(s.store.workspaceDir(bot.bot_id));
  assert.ok(existsSync(settingsFile));
  const newcomer = join(s.userData, 'Preferences');
  assert.ok(!s.settingsOf(bot.bot_id).sandbox.filesystem.denyRead.includes(newcomer));
  mkdirSync(newcomer);
  await s.service.launchSettings(s.store.workspaceDir(bot.bot_id));
  assert.ok(s.settingsOf(bot.bot_id).sandbox.filesystem.denyRead.includes(newcomer), 'what the app made since is walled off too');
});

test('a bound folder that is gone is left out of the launch and reported', async (t) => {
  const s = setup(t);
  const kept = s.folder('kept');
  const gone = s.folder('gone');
  const bot = s.service.create({ name: 'Missing a folder', workspaces: [{ path: kept, mode: 'write' }, { path: gone, mode: 'write' }] });
  rmSync(gone, { recursive: true });
  const launch = await s.service.launchSettings(s.store.workspaceDir(bot.bot_id));
  assert.deepEqual(launch.additionalDirectories, [kept]);
  assert.equal(s.warnings.length, 1);
  assert.match(s.warnings[0]!, new RegExp(`Bot ${bot.bot_id}: ${gone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')} is no longer`));
});

test('only the workspace of a Bot gets Bot launch settings', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Only mine' });
  assert.equal(s.service.ownsWorkspace(s.store.workspaceDir(bot.bot_id)), true);
  assert.equal(s.service.ownsWorkspace(s.store.homeDir(bot.bot_id)), false);
  assert.equal(s.service.ownsWorkspace(s.folder('project')), false);
  assert.equal(s.service.ownsWorkspace(s.home), false);
  await assert.rejects(s.service.launchSettings(s.folder('project')), /not the workspace of a Bot/);
  await assert.rejects(s.service.launchSettings(s.store.homeDir(bot.bot_id)), /not the workspace of a Bot/);
});

test('the model of a Bot is what its workspace answers', (t) => {
  const s = setup(t);
  const modelled = s.service.create({ name: 'Modelled', model: 'anthropic/claude-test' });
  const plain = s.service.create({ name: 'Plain' });
  assert.equal(s.service.modelFor(s.store.workspaceDir(modelled.bot_id)), 'anthropic/claude-test');
  assert.equal(s.service.modelFor(s.store.workspaceDir(plain.bot_id)), undefined);
  assert.equal(s.service.modelFor(s.folder('project')), undefined);
  s.service.update(modelled.bot_id, 1, { model: null });
  assert.equal(s.service.modelFor(s.store.workspaceDir(modelled.bot_id)), undefined);
});

test('a new session starts in the Bot workspace with the Bot model', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Starter', model: 'a/b' });
  const opened = await s.service.openSession(bot.bot_id);
  assert.deepEqual(opened, { botId: bot.bot_id, projectPath: s.store.workspaceDir(bot.bot_id), sessionId: SESSION });
  assert.deepEqual(s.calls, [['newSession', s.store.workspaceDir(bot.bot_id), 'a/b']]);
});

test('an existing session is opened only if the Bot\'s own catalog has it', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Resumer', model: 'bot/model' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const home = s.store.homeDir(bot.bot_id);
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(home)!.sessions.push(row(SESSION, { resume_model: 'session/model' }), row(OTHER_SESSION, { empty_session: true }));
  s.calls.length = 0;
  assert.deepEqual(await s.service.openSession(bot.bot_id, SESSION.toUpperCase()), { botId: bot.bot_id, projectPath: workspace, sessionId: SESSION });
  assert.deepEqual(await s.service.openSession(bot.bot_id, OTHER_SESSION), { botId: bot.bot_id, projectPath: workspace, sessionId: OTHER_SESSION });
  assert.deepEqual(s.calls.filter((call) => call[0] === 'openSession'), [
    ['openSession', { projectPath: workspace, sessionId: SESSION }, false, 'session/model'],
    ['openSession', { projectPath: workspace, sessionId: OTHER_SESSION }, true, 'bot/model'],
  ]);
  await assert.rejects(s.service.openSession(bot.bot_id, '11111111-2222-4333-8444-000000000000'), /does not belong to this Bot/);
  await assert.rejects(s.service.openSession(bot.bot_id, 'not-a-session'), /invalid session id/);
  await assert.rejects(s.service.openSession('Bot0000000000000000999'), BotNotFoundError);
});

test('the sessions listed are the Bot\'s own, without the host\'s private fields', async (t) => {
  const s = setup(t);
  const one = s.service.create({ name: 'One' });
  const two = s.service.create({ name: 'Two' });
  await s.service.listSessions(one.bot_id);
  await s.service.listSessions(two.bot_id);
  s.catalogs.get(s.store.homeDir(one.bot_id))!.sessions.push(row(SESSION, { resume_model: 'x/y', workspace_path: '/somewhere', resume_title: 'Named' }));
  const sessions = await s.service.listSessions(one.bot_id);
  assert.equal(sessions.length, 1);
  assert.ok(!('empty_session' in sessions[0]!) && !('resume_model' in sessions[0]!) && !('workspace_path' in sessions[0]!));
  assert.equal(sessions[0]!.uuid, SESSION);
  assert.deepEqual(await s.service.listSessions(two.bot_id), [], 'another Bot sees none of them');
  assert.deepEqual([...s.catalogs.keys()].sort(), [s.store.homeDir(one.bot_id), s.store.homeDir(two.bot_id)].sort());
  await assert.rejects(s.service.listSessions('Bot0000000000000000999'), BotNotFoundError);
});

test('deleting a Bot stops its sessions, then moves it to the trash, and frees the others of it', async (t) => {
  const s = setup(t);
  const gone = s.service.create({ name: 'Gone' });
  const stays = s.service.create({ name: 'Stays' });
  const workspace = s.store.workspaceDir(gone.bot_id);
  assert.ok(s.settingsOf(stays.bot_id).permissions.deny.includes(pathRule('Read', s.store.botDir(gone.bot_id))));
  await s.service.delete(gone.bot_id);
  assert.deepEqual(s.calls, [['releaseScope', workspace], ['closeProject', workspace]], 'the scheduler lets go before the sessions are stopped');
  assert.ok(!existsSync(s.store.botDir(gone.bot_id)));
  assert.equal(readdirSync(s.store.locations.trashRoot).length, 1);
  assert.deepEqual(s.service.list().map((bot) => bot.bot_id), [stays.bot_id]);
  const after = s.settingsOf(stays.bot_id);
  assert.ok(!after.permissions.deny.some((rule: string) => rule.includes(gone.bot_id)));
  assert.ok(after.permissions.deny.includes(pathRule('Read', s.store.locations.trashRoot)));
  await assert.rejects(s.service.delete(gone.bot_id), BotNotFoundError);
  assert.equal(s.service.ownsWorkspace(workspace), false);
});

test('a Bot that is working is not deleted', async (t) => {
  const s = setup(t);
  const busy = s.service.create({ name: 'Busy' });
  s.setActiveWork(true);
  await assert.rejects(s.service.delete(busy.bot_id), /cancel active turns/);
  assert.ok(existsSync(s.store.botDir(busy.bot_id)));
  assert.ok(!existsSync(s.store.locations.trashRoot));
  s.setActiveWork(false);
  assert.equal((await s.service.openSession(busy.bot_id)).botId, busy.bot_id, 'and it is not left half deleted');
  await s.service.delete(busy.bot_id);
  assert.ok(!existsSync(s.store.botDir(busy.bot_id)));
});

test('nothing starts, changes or launches in a Bot while it is being deleted', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Leaving' });
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(bot.bot_id);
  await assert.rejects(s.service.openSession(bot.bot_id), /being deleted/);
  await assert.rejects(s.service.launchSettings(s.store.workspaceDir(bot.bot_id)), /being deleted/);
  await assert.rejects(s.service.delete(bot.bot_id), /being deleted/);
  assert.throws(() => s.service.update(bot.bot_id, 1, { name: 'Late' }), /being deleted/);
  release();
  await deleting;
  assert.ok(!existsSync(s.store.botDir(bot.bot_id)));
});

// Where Bots can run

const REFUSAL = /Bots are available on macOS only\./;

test('Bots are available on macOS and nowhere else', () => {
  assert.deepEqual(botsAvailability('darwin'), { available: true });
  for (const platform of ['linux', 'win32', 'freebsd', 'android'] as const) {
    assert.deepEqual(botsAvailability(platform), { available: false, reason: 'Bots are available on macOS only.' }, platform);
  }
  assert.deepEqual(botsAvailability(), botsAvailability(process.platform), 'this computer by default');
});

test('where Bots cannot run every operation is refused in words the owner can read', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Mac Bot' });
  for (const platform of ['linux', 'win32'] as const) {
    const elsewhere = new BotService({
      store: s.store, platform,
      runtimes: { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => { throw new Error('unused'); } },
      catalogFor: () => ({ list: async () => ({ sessions: [] }), find: async () => undefined, workspaceSettings: async () => ({ environment: {}, configHome: '/x' }), invalidate: () => undefined }),
    });
    assert.deepEqual(elsewhere.availability(), { available: false, reason: 'Bots are available on macOS only.' });
    assert.throws(() => elsewhere.assertAvailable(), REFUSAL);
    assert.throws(() => elsewhere.list(), REFUSAL);
    assert.throws(() => elsewhere.get(bot.bot_id), REFUSAL);
    assert.throws(() => elsewhere.create({ name: 'Nowhere' }), REFUSAL);
    assert.throws(() => elsewhere.update(bot.bot_id, 1, { name: 'Renamed' }), REFUSAL);
    await assert.rejects(elsewhere.delete(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.openSession(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.listSessions(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.launchSettings(s.store.workspaceDir(bot.bot_id)), REFUSAL);
    await assert.rejects(elsewhere.lastActivityAt(bot.bot_id), REFUSAL);
    // And the app does not treat the folder of a Bot as one.
    assert.equal(elsewhere.ownsWorkspace(s.store.workspaceDir(bot.bot_id)), false);
    assert.equal(elsewhere.botIdFor(s.store.workspaceDir(bot.bot_id)), undefined);
    assert.equal(elsewhere.modelFor(s.store.workspaceDir(bot.bot_id)), undefined);
  }
  assert.deepEqual(s.service.list().map((record) => record.bot_id), [bot.bot_id], 'nothing above touched the Bot');
  assert.equal(s.store.get(bot.bot_id).revision, 1);
  assert.deepEqual(s.calls, []);
});

test('the Bot whose folder a working directory is can be told from the directory', (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Found' });
  assert.equal(s.service.botIdFor(s.store.workspaceDir(bot.bot_id)), bot.bot_id);
  assert.equal(s.service.workspaceOf(bot.bot_id), s.store.workspaceDir(bot.bot_id));
  assert.equal(s.service.botIdFor(s.folder('repo')), undefined);
  assert.equal(s.service.botIdFor(s.store.homeDir(bot.bot_id)), undefined);
});

// What the Bots page shows

test('listing the sessions reads the home again instead of what was listed a few seconds ago', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Fresh' });
  await s.service.listSessions(bot.bot_id);
  const home = s.store.homeDir(bot.bot_id);
  assert.deepEqual(s.calls.map((call) => call[0]), ['invalidate', 'list']);
  assert.deepEqual(s.calls[0], ['invalidate', home]);
});

test('a Bot was last active when its newest transcript last changed', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Active' });
  assert.equal(await s.service.lastActivityAt(bot.bot_id), undefined, 'a Bot that has had no session has no time');
  const directory = s.store.transcriptsDir(bot.bot_id);
  mkdirSync(join(directory, 'memdir'), { recursive: true });
  const touch = (name: string, seconds: number) => {
    const path = join(directory, name);
    writeFileSync(path, '{}\n');
    utimesSync(path, seconds, seconds);
  };
  touch('11111111-2222-4333-8444-555555555555.jsonl', 1_700_000_000);
  touch('99999999-2222-4333-8444-555555555555.jsonl', 1_700_000_500);
  touch('notes.txt', 1_800_000_000);
  writeFileSync(join(directory, 'memdir', 'later.md'), 'x');
  utimesSync(join(directory, 'memdir', 'later.md'), 1_800_000_000, 1_800_000_000);
  assert.equal(await s.service.lastActivityAt(bot.bot_id), 1_700_000_500_000, 'only transcripts count, and the newest of them');
});

// What the routine scheduler asks of the Bots

test('a Bot\'s controller session is not one of its chats, to list, to open or to date its activity by', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Scheduled' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  s.controllerOf(workspace, CONTROLLER);
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(row(SESSION), row(CONTROLLER));
  assert.deepEqual((await s.service.listSessions(bot.bot_id)).map((session) => session.uuid), [SESSION]);
  s.calls.length = 0;
  await assert.rejects(s.service.openSession(bot.bot_id, CONTROLLER), /does not belong to this Bot/);
  await assert.rejects(s.service.openSession(bot.bot_id, CONTROLLER.toUpperCase()), /does not belong to this Bot/);
  assert.deepEqual(s.calls, [], 'nothing was looked up or started for it');
  assert.equal((await s.service.openSession(bot.bot_id, SESSION)).sessionId, SESSION);

  const directory = s.store.transcriptsDir(bot.bot_id);
  mkdirSync(directory, { recursive: true });
  const touch = (name: string, seconds: number) => { writeFileSync(join(directory, name), '{}\n'); utimesSync(join(directory, name), seconds, seconds); };
  touch(`${SESSION}.jsonl`, 1_700_000_000);
  touch(`${CONTROLLER}.jsonl`, 1_700_000_900);
  assert.equal(await s.service.lastActivityAt(bot.bot_id), 1_700_000_000_000, 'a controller that wrote a transcript after the last chat is not use of the Bot');
});

test('a Bot that is being deleted is let go of by the scheduler first, and is no routine Bot while it is', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Leaving' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  assert.deepEqual(s.service.routineBots().map((routine) => routine.botId), [bot.bot_id]);
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(bot.bot_id);
  assert.deepEqual(s.calls, [['releaseScope', workspace], ['closeProject', workspace]]);
  assert.deepEqual(s.service.routineBots(), [], 'not offered while it is being deleted');
  assert.equal(s.service.routineBotFor(workspace), undefined);
  assert.equal(s.service.isBotWorkspace(workspace), true, 'though its folder still looks like a Bot\'s');
  release();
  await deleting;
  assert.equal(s.service.routineBotFor(workspace), undefined);
  assert.equal(s.service.isBotWorkspace(workspace), true, 'and still does after the Bot is gone');
});

test('a Bot whose deletion fails is a routine Bot again', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Busy' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  s.setActiveWork(true);
  await assert.rejects(s.service.delete(bot.bot_id), /cancel active turns/);
  assert.equal(s.service.routineBotFor(workspace)?.botId, bot.bot_id);
});

test('the scheduler is told of each Bot by its workspace, name, model and own catalog', async (t) => {
  const s = setup(t);
  const modelled = s.service.create({ name: 'Modelled', model: 'a/b' });
  const plain = s.service.create({ name: 'Plain' });
  const workspace = s.store.workspaceDir(modelled.bot_id);
  assert.deepEqual(s.service.routineBots().map(({ catalog: _catalog, ...bot }) => bot), [
    { botId: modelled.bot_id, name: 'Modelled', workspace, model: 'a/b' },
    { botId: plain.bot_id, name: 'Plain', workspace: s.store.workspaceDir(plain.bot_id) },
  ]);
  const found = s.service.routineBotFor(workspace)!;
  assert.equal(found.workspace, workspace);
  assert.equal(s.service.routineBotFor(`${workspace}${'/'}.`)?.workspace, workspace, 'a spelling of the folder is answered with the folder');
  assert.equal(s.service.routineBotFor(s.folder('project')), undefined);
  assert.equal(s.service.routineBotFor(s.store.homeDir(modelled.bot_id)), undefined);
  await found.catalog().find(workspace, SESSION);
  assert.deepEqual(s.calls.filter((call) => call[0] === 'find'), [['find', s.store.homeDir(modelled.bot_id), workspace, SESSION]], 'its chats are looked up in its home, not the owner\'s');
});

test('a Bot that cannot be read is left out, and the others are still offered', (t) => {
  const s = setup(t);
  const sound = s.service.create({ name: 'Sound' });
  const damaged = s.service.create({ name: 'Damaged' });
  writeFileSync(s.store.recordPath(damaged.bot_id), '{ not json');
  assert.deepEqual(s.service.routineBots().map((routine) => routine.botId), [sound.bot_id]);
  assert.equal(s.service.routineBotFor(s.store.workspaceDir(damaged.bot_id)), undefined);
  assert.equal(s.service.isBotWorkspace(s.store.workspaceDir(damaged.bot_id)), true);
});

test('where Bots cannot run the scheduler is told of none, yet still knows a Bot\'s folder when it sees one', (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Mac Bot' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const elsewhere = new BotService({
    store: s.store, platform: 'linux',
    runtimes: { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined },
    catalogFor: () => { throw new Error('unused'); },
  });
  assert.deepEqual(elsewhere.routineBots(), []);
  assert.equal(elsewhere.routineBotFor(workspace), undefined);
  assert.equal(elsewhere.isBotWorkspace(workspace), true);
  assert.equal(elsewhere.nameFor(workspace), undefined);
});

test('a Bot is named by its folder wherever the app has only the folder', (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Named' });
  assert.equal(s.service.nameFor(s.store.workspaceDir(bot.bot_id)), 'Named');
  assert.equal(s.service.nameFor(s.folder('project')), undefined);
  s.service.update(bot.bot_id, 1, { name: 'Renamed' });
  assert.equal(s.service.nameFor(s.store.workspaceDir(bot.bot_id)), 'Renamed');
});

// What Remote Control asks

const NO_RUNTIMES: BotRuntimes = { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined };
const NO_CATALOGS = (): BotCatalog => { throw new Error('unused'); };

test('a Bot stands for remote devices as off until the owner switches it on, and is gone when it cannot be read or is being deleted', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Reachable', description: 'Helps' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  assert.deepEqual(s.service.remoteBot(bot.bot_id), { state: 'off' }, 'a new Bot is off');
  s.service.update(bot.bot_id, 1, { remote: true });
  assert.deepEqual(s.service.remoteBot(bot.bot_id), { state: 'on', bot: { botId: bot.bot_id, name: 'Reachable', description: 'Helps', workspace } });
  assert.deepEqual(s.service.remoteBot('Bot0000000000000000999'), { state: 'unknown' });
  assert.deepEqual(s.service.remoteBot('../../etc'), { state: 'unknown' }, 'an id that is none never reaches the disk');
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(bot.bot_id);
  assert.deepEqual(s.service.remoteBot(bot.bot_id), { state: 'deleting' });
  release();
  await deleting;
  assert.deepEqual(s.service.remoteBot(bot.bot_id), { state: 'unknown' });
  const damaged = s.service.create({ name: 'Damaged' });
  writeFileSync(s.store.recordPath(damaged.bot_id), '{ not json');
  assert.deepEqual(s.service.remoteBot(damaged.bot_id), { state: 'unknown' }, 'it never throws');
});

test('the Bots offered to remote devices are those whose switch is on, oldest first, none being deleted or unreadable', async (t) => {
  const s = setup(t);
  const first = s.service.create({ name: 'First' });
  const second = s.service.create({ name: 'Second' });
  const third = s.service.create({ name: 'Third' });
  const fourth = s.service.create({ name: 'Fourth' });
  assert.deepEqual(s.service.remoteBots(), []);
  for (const bot of [first, second, third, fourth]) s.service.update(bot.bot_id, 1, { remote: true });
  assert.deepEqual(s.service.remoteBots().map((bot) => bot.name), ['First', 'Second', 'Third', 'Fourth']);
  assert.deepEqual(s.service.remoteBots()[0], { botId: first.bot_id, name: 'First', description: '', workspace: s.store.workspaceDir(first.bot_id) });
  s.service.update(second.bot_id, 2, { remote: false });
  writeFileSync(s.store.recordPath(third.bot_id), '{ not json');
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(fourth.bot_id);
  assert.deepEqual(s.service.remoteBots().map((bot) => bot.name), ['First'], 'switched off, unreadable and being deleted are all left out');
  release();
  await deleting;
});

test('the Bots offered to remote devices are ordered by when they were made, not by their ids', (t) => {
  const f = botFixture(t);
  const ids = ['Bot0000000000000000003', 'Bot0000000000000000002', 'Bot0000000000000000001'];
  let now = 1000;
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, now: () => now, newId: () => ids.shift()! });
  const service = new BotService({ store, platform: 'darwin', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS });
  for (const name of ['Oldest', 'Middle', 'Newest']) {
    const bot = service.create({ name });
    service.update(bot.bot_id, 1, { remote: true });
    now += 1000;
  }
  assert.deepEqual(service.remoteBots().map((bot) => bot.name), ['Oldest', 'Middle', 'Newest']);
});

test('where Bots cannot run, none is offered to remote devices and none is a chat\'s Bot', (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Mac Bot' });
  s.service.update(bot.bot_id, 1, { remote: true });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const elsewhere = new BotService({ store: s.store, platform: 'linux', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS, openProject: () => workspace });
  assert.deepEqual(elsewhere.remoteBots(), []);
  assert.deepEqual(elsewhere.remoteBot(bot.bot_id), { state: 'unknown' });
  assert.equal(elsewhere.chat(SESSION), undefined);
  assert.equal(elsewhere.remoteEnabled(workspace), false);
});

test('a chat is a Bot\'s when it is open in the Bot\'s workspace or its transcript is in the Bot\'s home', (t) => {
  const open = new Map<string, string>();
  const s = setup(t, { openProject: (sessionId) => open.get(sessionId) });
  const bot = s.service.create({ name: 'Chatty' });
  const other = s.service.create({ name: 'Other' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const transcript = (botId: string, sessionId: string) => {
    mkdirSync(s.store.transcriptsDir(botId), { recursive: true });
    writeFileSync(join(s.store.transcriptsDir(botId), `${sessionId}.jsonl`), '');
  };
  assert.equal(s.service.chat(SESSION), undefined, 'a session nobody knows');

  transcript(bot.bot_id, SESSION);
  assert.deepEqual(s.service.chat(SESSION), { botId: bot.bot_id, ref: { projectPath: workspace, sessionId: SESSION }, remote: false }, 'found by its transcript');
  s.service.update(bot.bot_id, 1, { remote: true });
  assert.equal(s.service.chat(SESSION)?.remote, true, 'and follows the switch');

  open.set(OTHER_SESSION, s.store.workspaceDir(other.bot_id));
  assert.deepEqual(s.service.chat(OTHER_SESSION), { botId: other.bot_id, ref: { projectPath: s.store.workspaceDir(other.bot_id), sessionId: OTHER_SESSION }, remote: false }, 'found by where it is open, before any transcript');

  open.set(SESSION, s.folder('repo'));
  assert.equal(s.service.chat(SESSION), undefined, 'open in a project of the owner\'s, it is no Bot\'s, whatever is in a Bot\'s home');
  open.delete(SESSION);

  s.controllerOf(workspace, CONTROLLER);
  transcript(bot.bot_id, CONTROLLER);
  assert.equal(s.service.chat(CONTROLLER), undefined, 'the scheduler\'s hidden session is no chat');

  const lettered = 'abcdefab-2222-4333-8444-abcdefabcdef';
  transcript(bot.bot_id, lettered);
  assert.equal(s.service.chat(lettered)?.botId, bot.bot_id);
  assert.equal(s.service.chat(lettered.toUpperCase()), undefined, 'only the spelling the engine writes: a file system that ignores case would find the transcript');
  for (const bad of ['', 'not-a-session', '../x', `${SESSION}.jsonl`, 5 as any]) assert.equal(s.service.chat(bad), undefined, JSON.stringify(bad));
});

test('a chat of a Bot that is being deleted is not reachable, and one of a Bot that is gone is nobody\'s', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Leaving' });
  s.service.update(bot.bot_id, 1, { remote: true });
  mkdirSync(s.store.transcriptsDir(bot.bot_id), { recursive: true });
  writeFileSync(join(s.store.transcriptsDir(bot.bot_id), `${SESSION}.jsonl`), '');
  assert.equal(s.service.chat(SESSION)?.remote, true);
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(bot.bot_id);
  assert.equal(s.service.chat(SESSION)?.remote, false, 'still its chat, but not reachable');
  release();
  await deleting;
  assert.equal(s.service.chat(SESSION), undefined);
});

test('Remote Control is on for a folder when the Bot that works in it has its switch on', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Folder' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  assert.equal(s.service.remoteEnabled(workspace), false);
  s.service.update(bot.bot_id, 1, { remote: true });
  assert.equal(s.service.remoteEnabled(workspace), true);
  assert.equal(s.service.remoteEnabled(s.folder('repo')), false, 'a folder of the owner\'s');
  assert.equal(s.service.remoteEnabled(s.store.homeDir(bot.bot_id)), false, 'the home is no workspace');
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(bot.bot_id);
  assert.equal(s.service.remoteEnabled(workspace), false, 'not while it is being deleted');
  release();
  await deleting;
  assert.equal(s.service.remoteEnabled(workspace), false);
});

test('Remote Control hears of a Bot made, changed or deleted, and of nothing that changed nothing or failed', async (t) => {
  let heard = 0;
  const s = setup(t, { changed: () => { heard += 1; } });
  const bot = s.service.create({ name: 'Heard' });
  assert.equal(heard, 1, 'made');
  s.service.update(bot.bot_id, 1, { remote: true });
  assert.equal(heard, 2, 'switched on');
  s.service.update(bot.bot_id, 2, { remote: true });
  s.service.update(bot.bot_id, 2, { name: 'Heard' });
  assert.equal(heard, 2, 'nothing changed');
  assert.throws(() => s.service.update(bot.bot_id, 1, { name: 'Stale' }), BotConflictError);
  assert.throws(() => s.service.update(bot.bot_id, 2, { remote: 'yes' as any }), /invalid remote/);
  assert.throws(() => s.service.create({ name: '' }), /invalid name/);
  assert.equal(heard, 2, 'a refused change is not news');
  s.service.update(bot.bot_id, 2, { name: 'Renamed' });
  assert.equal(heard, 3, 'renamed');
  s.setActiveWork(true);
  await assert.rejects(s.service.delete(bot.bot_id), /cancel active turns/);
  assert.equal(heard, 3, 'a deletion that failed is not news');
  s.setActiveWork(false);
  await s.service.delete(bot.bot_id);
  assert.equal(heard, 4, 'deleted');
});

test('a listener that fails does not fail the change, and is reported', async (t) => {
  const s = setup(t, { changed: () => { throw new Error('listener broke'); } });
  const bot = s.service.create({ name: 'Unbothered' });
  assert.equal(s.service.update(bot.bot_id, 1, { remote: true }).remote, true);
  await s.service.delete(bot.bot_id);
  assert.equal(s.warnings.filter((message) => message.includes('could not be passed on: listener broke')).length, 3);
});
