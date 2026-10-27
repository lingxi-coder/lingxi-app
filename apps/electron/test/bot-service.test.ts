import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { pathRule } from '../src/main/bots/botSettings.js';
import { BotService, botsAvailability, type BotCatalog, type BotRuntimes } from '../src/main/bots/service.js';
import { BotConflictError, BotNotFoundError } from '../src/main/bots/store.js';
import type { ProjectSessionCatalogRow } from '../src/main/session-catalog.js';
import { botFixture, SESSION } from './bot-helpers.js';

const OTHER_SESSION = '99999999-2222-4333-8444-555555555555';

function row(uuid: string, extra: Partial<ProjectSessionCatalogRow> = {}): ProjectSessionCatalogRow {
  return {
    uuid, title: `Session ${uuid.slice(0, 4)}`, modified_rfc3339: '2026-10-26T00:00:00Z', message_count: 2, mode: 'code', path: `/transcripts/${uuid}.jsonl`,
    empty_session: false, ...extra,
  };
}

function setup(t: { after(fn: () => void): void }) {
  const f = botFixture(t);
  const calls: unknown[][] = [];
  const warnings: string[] = [];
  let activeWork = false;
  let hold: Promise<void> | undefined;
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
    warn: (message) => warnings.push(message),
  });
  return {
    ...f, service, calls, warnings, catalogs,
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
  assert.deepEqual(s.calls, [['closeProject', workspace]]);
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
