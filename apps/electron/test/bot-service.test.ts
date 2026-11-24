import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { pathRule } from '../src/main/bots/botSettings.js';
import { BotService, botsAvailability, type BotCatalog, type BotRuntimes, type BotServiceOptions } from '../src/main/bots/service.js';
import { BotConflictError, BotNotFoundError, BotStore, DEFAULT_WORKSPACE_ID, workspaceIdOf } from '../src/main/bots/store.js';
import type { ProjectSessionCatalogRow } from '../src/main/session-catalog.js';
import { botFixture, SESSION } from './bot-helpers.js';

const OTHER_SESSION = '99999999-2222-4333-8444-555555555555';
const THIRD_SESSION = '77777777-2222-4333-8444-555555555555';
/** A runtime manager that must not be asked anything. */
const NO_RUNTIMES: BotRuntimes = {
  newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined,
  closeSession: async () => undefined, restart: async () => undefined, setSessionDirectoryGrants: () => undefined, runtimeSummaries: [],
};
const NO_CATALOGS = (): BotCatalog => { throw new Error('unused'); };
/** The hidden session that watches the clock for a Bot's routines. */
const CONTROLLER = '33333333-2222-4333-8444-555555555555';

function row(uuid: string, extra: Partial<ProjectSessionCatalogRow> = {}): ProjectSessionCatalogRow {
  return {
    uuid, title: `Session ${uuid.slice(0, 4)}`, modified_rfc3339: '2026-10-26T00:00:00Z', message_count: 2, mode: 'code', path: `/transcripts/${uuid}.jsonl`,
    empty_session: false, ...extra,
  };
}

/** A chat of the Bot as its home keeps it: the transcript is in the project folder of the folder it worked in, the Bot's own unless another is given. */
function chatOf(s: { store: BotStore }, botId: string, uuid: string, folder = s.store.workspaceDir(botId), extra: Partial<ProjectSessionCatalogRow> = {}): ProjectSessionCatalogRow {
  return row(uuid, { path: join(s.store.transcriptsDirOf(botId, folder), `${uuid}.jsonl`), workspace_path: folder, ...extra });
}

function setup(t: { after(fn: () => void): void }, options: Pick<BotServiceOptions, 'openProject' | 'changed' | 'teamRevoking' | 'retryMs' | 'deleted'> = {}) {
  const f = botFixture(t);
  const calls: unknown[][] = [];
  const warnings: string[] = [];
  let activeWork = false;
  let hold: Promise<void> | undefined;
  let nextSession = SESSION;
  let newSessionFailure: Error | undefined;
  const controllers = new Set<string>();
  const liveRuntimes = new Map<string, any>();
  const runtimes: BotRuntimes = {
    newSession: async (projectPath, model, owner) => {
      calls.push(['newSession', projectPath, model, { mode: owner.mode, projectId: owner.projectId, draftKey: owner.draftKey }]);
      const ref = { projectPath, sessionId: nextSession };
      await owner.onAllocated?.(ref);
      if (newSessionFailure) throw newSessionFailure;
      return ref;
    },
    openSession: async (ref, empty, resumeModel, activate, path) => { calls.push(['openSession', ref, empty, resumeModel, activate, path]); return {}; },
    get: (id) => liveRuntimes.get(id),
    closeProject: async (projectPath) => {
      calls.push(['closeProject', projectPath]);
      await hold;
      if (activeWork) throw new Error('cancel active turns and pending interactions before removing a project');
    },
    closeSession: async (ref) => { calls.push(['closeSession', ref]); liveRuntimes.delete(ref.sessionId); },
    restart: async (ref) => { calls.push(['restart', ref]); },
    setSessionDirectoryGrants: (sessionId, directories) => { calls.push(['grants', sessionId, [...directories]]); },
    get runtimeSummaries() { return [...liveRuntimes].map(([sessionId, runtime]) => ({ projectPath: runtime.projectPath as string, sessionId })); },
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
        list: async (projectPath, scope, include) => { calls.push(['list', configHome, projectPath, scope]); return { sessions: state.sessions.filter((session) => include?.(session) ?? true) }; },
        find: async (projectPath, sessionId) => { calls.push(['find', configHome, projectPath, sessionId]); return state.sessions.find((session) => session.uuid === sessionId.toLowerCase()); },
        select: async (projectPath, sessionId, selection) => { calls.push(['select', configHome, projectPath, sessionId, selection]); return state.sessions.find((session) => session.uuid === sessionId.toLowerCase() && session.path === selection.transcriptPath); },
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
    openProject: (sessionId) => liveRuntimes.get(sessionId)?.projectPath,
    ...options,
  });
  return {
    ...f, service, calls, warnings, catalogs, liveRuntimes,
    /** Make a session the controller of a Bot's routines. */
    controllerOf(workspace: string, sessionId: string) { controllers.add(`${workspace}:${sessionId}`); },
    /** The id the next new session is given, and whether starting one fails after it has its id. */
    nextSession(id: string) { nextSession = id; },
    failNewSession(error: Error | undefined) { newSessionFailure = error; },
    /** A session that is open, in the folder it works in. */
    open(sessionId: string, projectPath: string, extra: Record<string, unknown> = {}) {
      const runtime = { projectPath, sessionId, sessionMode: 'code', hasActiveWork: false, connectionState: { status: 'connected' }, ...extra };
      liveRuntimes.set(sessionId, runtime);
      return runtime;
    },
    /** What the runtime manager was asked, of the kinds given. */
    asked(...kinds: string[]) { return calls.filter((call) => kinds.includes(call[0] as string)); },
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

test('changing a Bot rewrites its settings, and a stale revision is a conflict', async (t) => {
  const s = setup(t);
  const one = s.folder('one');
  const bot = s.service.create({ name: 'Changing', workspaces: [{ path: one, mode: 'write' }] });
  assert.deepEqual(s.settingsOf(bot.bot_id).permissions.additionalDirectories, [s.store.workspaceDir(bot.bot_id), one]);
  const updated = await s.service.update(bot.bot_id, 1, { workspaces: [{ path: one, mode: 'read' }], model: 'a/b' });
  assert.equal(updated.revision, 2);
  const settings = s.settingsOf(bot.bot_id);
  assert.ok(settings.permissions.deny.includes(pathRule('Edit', one)));
  assert.equal(settings.model, 'a/b');
  await assert.rejects(s.service.update(bot.bot_id, 1, { name: 'Stale' }), BotConflictError);
  assert.equal(s.service.get(bot.bot_id).name, 'Changing');
  await assert.rejects(s.service.update('Bot0000000000000000999', 1, { name: 'Nobody' }), BotNotFoundError);
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
    userSettingsOnly: true,
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

test('only the workspace of a Bot, or a session it opened, gets Bot launch settings', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Only mine' });
  const owns = (projectPath: string) => s.service.ownsSession({ projectPath, sessionId: SESSION });
  assert.equal(owns(s.store.workspaceDir(bot.bot_id)), true);
  assert.equal(owns(s.store.homeDir(bot.bot_id)), false);
  assert.equal(owns(s.folder('project')), false);
  assert.equal(owns(s.home), false);
  await assert.rejects(s.service.launchSettings(s.folder('project')), /not the workspace of a Bot/);
  await assert.rejects(s.service.launchSettings(s.store.homeDir(bot.bot_id)), /not the workspace of a Bot/);
});

test('the model of a Bot is what its workspace answers', async (t) => {
  const s = setup(t);
  const modelled = s.service.create({ name: 'Modelled', model: 'anthropic/claude-test' });
  const plain = s.service.create({ name: 'Plain' });
  assert.equal(s.service.modelFor(s.store.workspaceDir(modelled.bot_id)), 'anthropic/claude-test');
  assert.equal(s.service.modelFor(s.store.workspaceDir(plain.bot_id)), undefined);
  assert.equal(s.service.modelFor(s.folder('project')), undefined);
  await s.service.update(modelled.bot_id, 1, { model: null });
  assert.equal(s.service.modelFor(s.store.workspaceDir(modelled.bot_id)), undefined);
});

test('a new session starts in the Bot workspace with the Bot model', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Starter', model: 'a/b' });
  const opened = await s.service.openSession(bot.bot_id);
  assert.deepEqual(opened, { botId: bot.bot_id, projectPath: s.store.workspaceDir(bot.bot_id), sessionId: SESSION, workspaceId: DEFAULT_WORKSPACE_ID });
  assert.deepEqual(s.calls, [['newSession', s.store.workspaceDir(bot.bot_id), 'a/b', { mode: 'code', projectId: null, draftKey: `bot:${bot.bot_id}:default` }]]);
  assert.equal(s.service.botIdForSession({ projectPath: s.store.workspaceDir(bot.bot_id), sessionId: SESSION }), bot.bot_id);
});

test('an existing session is opened only if the Bot\'s own catalog has it', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Resumer', model: 'bot/model' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const home = s.store.homeDir(bot.bot_id);
  await s.service.listSessions(bot.bot_id);
  const mine = chatOf(s, bot.bot_id, SESSION, workspace, { resume_model: 'session/model' });
  const empty = chatOf(s, bot.bot_id, OTHER_SESSION, workspace, { empty_session: true });
  s.catalogs.get(home)!.sessions.push(mine, empty);
  s.calls.length = 0;
  assert.deepEqual(await s.service.openSession(bot.bot_id, SESSION.toUpperCase(), mine.path), { botId: bot.bot_id, projectPath: workspace, sessionId: SESSION, workspaceId: DEFAULT_WORKSPACE_ID });
  assert.deepEqual(await s.service.openSession(bot.bot_id, OTHER_SESSION, empty.path), { botId: bot.bot_id, projectPath: workspace, sessionId: OTHER_SESSION, workspaceId: DEFAULT_WORKSPACE_ID });
  assert.deepEqual(s.calls.filter((call) => call[0] === 'openSession'), [
    ['openSession', { projectPath: workspace, sessionId: SESSION, mode: 'code' }, false, 'session/model', true, mine.path],
    ['openSession', { projectPath: workspace, sessionId: OTHER_SESSION, mode: 'code' }, true, 'bot/model', true, empty.path],
  ]);
  await assert.rejects(s.service.openSession(bot.bot_id, '11111111-2222-4333-8444-000000000000'), /source is unavailable/);
  await assert.rejects(s.service.openSession(bot.bot_id, 'not-a-session'), /invalid session id/);
  await assert.rejects(s.service.openSession('Bot0000000000000000999'), BotNotFoundError);
});

test('a Bot stores its exact selected source privately and rejects missing or conflicting copies', async t => {
  const s = setup(t), bot = s.service.create({ name: 'Exact source' });
  const workspace = s.store.workspaceDir(bot.bot_id), home = s.store.homeDir(bot.bot_id);
  await s.service.listSessions(bot.bot_id);
  const selected = chatOf(s, bot.bot_id, SESSION, workspace);
  s.catalogs.get(home)!.sessions.push(selected);
  await assert.rejects(s.service.openSession(bot.bot_id, SESSION), /source is unavailable/);
  await s.service.openSession(bot.bot_id, SESSION, selected.path);
  assert.deepEqual(s.store.sessionSource(bot.bot_id, SESSION), { path: selected.path, mode: 'code' });
  assert.deepEqual(await s.service.resolveSavedTranscript({ projectPath: workspace, sessionId: SESSION, mode: 'code', projectId: null }), { path: selected.path, mode: 'code' });
  await s.service.openSession(bot.bot_id, SESSION);
  await assert.rejects(s.service.openSession(bot.bot_id, SESSION, `/other/${SESSION}.jsonl`), /different saved source/);
  s.catalogs.get(home)!.sessions = [{ ...selected, mode: 'chat' }];
  await assert.rejects(s.service.resolveSavedTranscript({ projectPath: workspace, sessionId: SESSION, mode: 'code' }), /source is unavailable/);
});

test('a disconnected Bot writer is reconnected through its existing owner without a catalog lookup', async t => {
  const s = setup(t), bot = s.service.create({ name: 'Disconnected owner' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  s.liveRuntimes.set(SESSION, { projectPath: workspace, sessionMode: 'code', connectionState: { status: 'disconnected' } });
  assert.deepEqual(await s.service.openSession(bot.bot_id, SESSION), { botId: bot.bot_id, projectPath: workspace, sessionId: SESSION, workspaceId: DEFAULT_WORKSPACE_ID });
  assert.deepEqual(s.calls.filter(call => call[0] === 'openSession'), [['openSession', { projectPath: workspace, sessionId: SESSION, mode: 'code' }, false, undefined, true, undefined]]);
  assert.equal(s.calls.some(call => call[0] === 'select'), false);
});

test('an explicit Bot selection reconnects its writer and refuses a different live copy', async t => {
  const s = setup(t), bot = s.service.create({ name: 'Selected owner' });
  const workspace = s.store.workspaceDir(bot.bot_id), selected = chatOf(s, bot.bot_id, SESSION, workspace);
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(selected);
  const live = { projectPath: workspace, sessionMode: 'code', connectionState: { status: 'disconnected' },
    getWorkspaceState: async () => ({ transcript_path: `/other/${SESSION}.jsonl` }) };
  s.liveRuntimes.set(SESSION, live);
  (s.service as any).options.runtimes.openSession = async (...args: unknown[]) => {
    s.calls.push(['openSession', ...args]);
    live.connectionState = { status: 'connected' };
  };
  await assert.rejects(s.service.openSession(bot.bot_id, SESSION, selected.path), /different runtime/);
  assert.equal(s.calls.filter(call => call[0] === 'openSession').length, 1, 'only the existing owner was reconnected');
  assert.equal(s.store.sessionSource(bot.bot_id, SESSION), undefined, 'a rejected copy was not remembered');
});

test('a Bot first durable prompt captures its own live writer, mode and workspace', async t => {
  const s = setup(t), bot = s.service.create({ name: 'Live source' });
  const workspace = s.store.workspaceDir(bot.bot_id), directory = s.store.transcriptsDir(bot.bot_id);
  mkdirSync(directory, { recursive: true });
  const path = join(directory, `${SESSION}.jsonl`); writeFileSync(path, '{}\n');
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(row(SESSION, { path, workspace_path: workspace }));
  const runtime = { sessionId: SESSION, projectPath: workspace, sessionMode: 'code',
    getWorkspaceState: async () => ({ cwd: workspace, transcript_path: path }), rememberTranscriptSelection: (value: string) => assert.equal(value, path) };
  s.liveRuntimes.set(SESSION, runtime);
  assert.equal(await s.service.captureSessionSource({ projectPath: workspace, sessionId: SESSION, mode: 'code', projectId: null }, runtime as any), path);
  assert.deepEqual(s.store.sessionSource(bot.bot_id, SESSION), { path, mode: 'code' });
  await assert.rejects(s.service.captureSessionSource({ projectPath: workspace, sessionId: SESSION, mode: 'chat', projectId: null }, runtime as any), /owner changed/);
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
  assert.equal(sessions[0]!.removedPath, '/somewhere', 'a chat that worked in a folder the Bot does not have is told so, and by where it worked');
  assert.equal(sessions[0]!.workspaceId, undefined);
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
  assert.equal(s.service.ownsSession({ projectPath: workspace, sessionId: SESSION }), false);
});

test('a deletion stops the Bot\'s shared-Bot runs before it waits and again once the Bot is trashed', async (t) => {
  const stopped: Array<[string, boolean]> = [];
  const s = setup(t, { teamRevoking: (botId) => { stopped.push([botId, existsSync(s.store.botDir(botId))]); } });
  const busy = s.service.create({ name: 'Busy' });
  s.setActiveWork(true);
  await assert.rejects(s.service.delete(busy.bot_id), /cancel active turns/);
  assert.deepEqual(stopped, [[busy.bot_id, true]], 'a deletion that failed stops them once, before it waits');
  s.setActiveWork(false);
  stopped.length = 0;
  await s.service.delete(busy.bot_id);
  // A shared-Bot run may start while the deletion waits, when the Bot is still there.
  assert.deepEqual(stopped, [[busy.bot_id, true], [busy.bot_id, false]]);
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
  await assert.rejects(s.service.update(bot.bot_id, 1, { name: 'Late' }), /being deleted/);
  await assert.rejects(s.service.addWorkspace(bot.bot_id, { path: s.folder('late'), mode: 'read' }), /being deleted/);
  await assert.rejects(s.service.removeWorkspace(bot.bot_id, 'wNoSuchFold'), /being deleted/);
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
      runtimes: NO_RUNTIMES,
      catalogFor: () => ({ list: async () => ({ sessions: [] }), find: async () => undefined, select: async () => undefined, workspaceSettings: async () => ({ environment: {}, configHome: '/x' }), invalidate: () => undefined }),
    });
    assert.deepEqual(elsewhere.availability(), { available: false, reason: 'Bots are available on macOS only.' });
    assert.throws(() => elsewhere.assertAvailable(), REFUSAL);
    assert.throws(() => elsewhere.list(), REFUSAL);
    assert.throws(() => elsewhere.get(bot.bot_id), REFUSAL);
    assert.throws(() => elsewhere.create({ name: 'Nowhere' }), REFUSAL);
    await assert.rejects(elsewhere.update(bot.bot_id, 1, { name: 'Renamed' }), REFUSAL);
    await assert.rejects(elsewhere.addWorkspace(bot.bot_id, { path: s.folder('x'), mode: 'read' }), REFUSAL);
    await assert.rejects(elsewhere.grantWorkspace(bot.bot_id, { path: s.folder('x'), mode: 'read' }), REFUSAL);
    await assert.rejects(elsewhere.changeWorkspace(bot.bot_id, 'wNoSuchFold', { mode: 'read' }), REFUSAL);
    await assert.rejects(elsewhere.removeWorkspace(bot.bot_id, 'wNoSuchFold'), REFUSAL);
    await assert.rejects(elsewhere.delete(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.openSession(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.listSessions(bot.bot_id), REFUSAL);
    await assert.rejects(elsewhere.launchSettings(s.store.workspaceDir(bot.bot_id)), REFUSAL);
    await assert.rejects(elsewhere.lastActivityAt(bot.bot_id), REFUSAL);
    // And the app does not treat the folder of a Bot as one.
    assert.equal(elsewhere.ownsSession({ projectPath: s.store.workspaceDir(bot.bot_id), sessionId: SESSION }), false);
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
  const mine = chatOf(s, bot.bot_id, SESSION, workspace);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(mine, chatOf(s, bot.bot_id, CONTROLLER, workspace));
  assert.deepEqual((await s.service.listSessions(bot.bot_id)).map((session) => session.uuid), [SESSION]);
  s.calls.length = 0;
  await assert.rejects(s.service.openSession(bot.bot_id, CONTROLLER), /does not belong to this Bot/);
  await assert.rejects(s.service.openSession(bot.bot_id, CONTROLLER.toUpperCase()), /does not belong to this Bot/);
  assert.deepEqual(s.calls, [], 'nothing was looked up or started for it');
  assert.equal((await s.service.openSession(bot.bot_id, SESSION, mine.path)).sessionId, SESSION);

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
  assert.deepEqual(s.service.routineBots().map((bot) => ({ botId: bot.botId, name: bot.name, workspace: bot.workspace, ...(bot.model !== undefined ? { model: bot.model } : {}) })), [
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
  const elsewhere = new BotService({ store: s.store, platform: 'linux', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS });
  assert.deepEqual(elsewhere.routineBots(), []);
  assert.equal(elsewhere.routineBotFor(workspace), undefined);
  assert.equal(elsewhere.isBotWorkspace(workspace), true);
  assert.equal(elsewhere.nameFor(workspace), undefined);
});

test('a Bot is named by its folder wherever the app has only the folder', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Named' });
  assert.equal(s.service.nameFor(s.store.workspaceDir(bot.bot_id)), 'Named');
  assert.equal(s.service.nameFor(s.folder('project')), undefined);
  await s.service.update(bot.bot_id, 1, { name: 'Renamed' });
  assert.equal(s.service.nameFor(s.store.workspaceDir(bot.bot_id)), 'Renamed');
});

// What Remote Control asks

test('a Bot stands for remote devices as off until the owner switches it on, and is gone when it cannot be read or is being deleted', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Reachable', description: 'Helps' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  assert.deepEqual(s.service.remoteBot(bot.bot_id), { state: 'off' }, 'a new Bot is off');
  await s.service.update(bot.bot_id, 1, { remote: true });
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
  for (const bot of [first, second, third, fourth]) await s.service.update(bot.bot_id, 1, { remote: true });
  assert.deepEqual(s.service.remoteBots().map((bot) => bot.name), ['First', 'Second', 'Third', 'Fourth']);
  assert.deepEqual(s.service.remoteBots()[0], { botId: first.bot_id, name: 'First', description: '', workspace: s.store.workspaceDir(first.bot_id) });
  await s.service.update(second.bot_id, 2, { remote: false });
  writeFileSync(s.store.recordPath(third.bot_id), '{ not json');
  let release!: () => void;
  s.holdClose(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = s.service.delete(fourth.bot_id);
  assert.deepEqual(s.service.remoteBots().map((bot) => bot.name), ['First'], 'switched off, unreadable and being deleted are all left out');
  release();
  await deleting;
});

test('the Bots offered to remote devices are ordered by when they were made, not by their ids', async (t) => {
  const f = botFixture(t);
  const ids = ['Bot0000000000000000003', 'Bot0000000000000000002', 'Bot0000000000000000001'];
  let now = 1000;
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, now: () => now, newId: () => ids.shift()! });
  const service = new BotService({ store, platform: 'darwin', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS });
  for (const name of ['Oldest', 'Middle', 'Newest']) {
    const bot = service.create({ name });
    await service.update(bot.bot_id, 1, { remote: true });
    now += 1000;
  }
  assert.deepEqual(service.remoteBots().map((bot) => bot.name), ['Oldest', 'Middle', 'Newest']);
});

test('where Bots cannot run, none is offered to remote devices and none is a chat\'s Bot', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Mac Bot' });
  await s.service.update(bot.bot_id, 1, { remote: true });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const elsewhere = new BotService({ store: s.store, platform: 'linux', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS, openProject: () => workspace });
  assert.deepEqual(elsewhere.remoteBots(), []);
  assert.deepEqual(elsewhere.remoteBot(bot.bot_id), { state: 'unknown' });
  assert.equal(elsewhere.chat(SESSION), undefined);
  assert.equal(elsewhere.remoteEnabled(workspace), false);
});

test('a chat is a Bot\'s when it is open in the Bot\'s workspace or its transcript is in the Bot\'s home', async (t) => {
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
  await s.service.update(bot.bot_id, 1, { remote: true });
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
  await s.service.update(bot.bot_id, 1, { remote: true });
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
  await s.service.update(bot.bot_id, 1, { remote: true });
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
  await s.service.update(bot.bot_id, 1, { remote: true });
  assert.equal(heard, 2, 'switched on');
  await s.service.update(bot.bot_id, 2, { remote: true });
  await s.service.update(bot.bot_id, 2, { name: 'Heard' });
  assert.equal(heard, 2, 'nothing changed');
  await assert.rejects(s.service.update(bot.bot_id, 1, { name: 'Stale' }), BotConflictError);
  await assert.rejects(s.service.update(bot.bot_id, 2, { remote: 'yes' as any }), /invalid remote/);
  assert.throws(() => s.service.create({ name: '' }), /invalid name/);
  assert.equal(heard, 2, 'a refused change is not news');
  await s.service.update(bot.bot_id, 2, { name: 'Renamed' });
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
  assert.equal((await s.service.update(bot.bot_id, 1, { remote: true })).remote, true);
  await s.service.delete(bot.bot_id);
  assert.equal(s.warnings.filter((message) => message.includes('could not be passed on: listener broke')).length, 3);
});

test('preparing a scheduled Bot history source never activates it during native startup', async t => {
  const s = setup(t), bot = s.service.create({ name: 'Prepared history' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  await s.service.listSessions(bot.bot_id);
  const history = chatOf(s, bot.bot_id, SESSION, workspace);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(history);
  await s.service.openSession(bot.bot_id, SESSION, history.path, false);
  assert.equal(s.calls.filter(call => call[0] === 'openSession').at(-1)?.[4], false);
  s.calls.length = 0;
  s.liveRuntimes.set(SESSION, { projectPath: workspace, sessionMode: 'code' });
  await s.service.openSession(bot.bot_id, SESSION, undefined, false);
  assert.equal(s.calls.filter(call => call[0] === 'openSession').at(-1)?.[4], false);
});

// A Bot with more than one folder

/** A Bot with one folder of the owner's bound to it, and that folder's id. */
function boundBot(s: ReturnType<typeof setup>, mode: 'read' | 'write' = 'write', name = 'Wide') {
  const repo = s.folder('repo');
  const bot = s.service.create({ name, workspaces: [{ path: repo, mode }] });
  return { bot, repo, own: s.store.workspaceDir(bot.bot_id), folder: s.store.get(bot.bot_id).workspaces[0]! };
}

async function until(condition: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail('the condition did not come true');
}

test('a session is a Bot\'s because the Bot opened it, wherever it works; the owner\'s sessions in a bound folder stay the owner\'s', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  assert.equal(s.service.ownsSession({ projectPath: repo, sessionId: SESSION }), false, 'a folder of the owner\'s says nothing of whose session it is');
  assert.equal(s.service.ownsSession({ projectPath: own, sessionId: OTHER_SESSION }), true, 'the Bot\'s own folder is nobody else\'s, and is never given the owner\'s configuration');
  const opened = await s.service.openSession(bot.bot_id, undefined, undefined, true, folder.id);
  assert.deepEqual(opened, { botId: bot.bot_id, projectPath: repo, sessionId: SESSION, workspaceId: folder.id });
  assert.deepEqual(s.asked('newSession'), [['newSession', repo, undefined, { mode: 'code', projectId: null, draftKey: `bot:${bot.bot_id}:${folder.id}` }]], 'the draft is the Bot\'s own for that folder');
  assert.equal(s.service.botIdForSession({ projectPath: repo, sessionId: SESSION }), bot.bot_id);
  assert.equal(s.service.botIdForSession({ projectPath: repo, sessionId: SESSION.toUpperCase() }), bot.bot_id, 'the spelling does not matter');
  assert.equal(s.service.botIdForSession({ projectPath: repo, sessionId: THIRD_SESSION }), undefined, 'another session in the same folder is the owner\'s');
  assert.equal(s.service.modelFor(repo, SESSION), undefined);
  const launch = await s.service.launchSettings(repo, SESSION);
  assert.equal(launch.configHome, s.store.homeDir(bot.bot_id));
  assert.deepEqual(launch.additionalDirectories, [own], 'the folder it works in is its working directory, the others are granted');
  await assert.rejects(s.service.launchSettings(repo, THIRD_SESSION), /not the workspace of a Bot/);
  await assert.rejects(s.service.launchSettings(repo), /not the workspace of a Bot/);
});

test('a session belongs to one Bot only, and is forgotten with the Bot', async (t) => {
  const s = setup(t);
  const one = s.service.create({ name: 'One' });
  const two = s.service.create({ name: 'Two' });
  s.service.claimSession(one.bot_id, SESSION);
  s.service.claimSession(one.bot_id, SESSION.toUpperCase());
  assert.throws(() => s.service.claimSession(two.bot_id, SESSION), /does not belong to this Bot/);
  assert.equal(s.service.botIdForSession({ projectPath: s.folder('anywhere'), sessionId: SESSION }), one.bot_id);
  await s.service.delete(one.bot_id);
  assert.equal(s.service.botIdForSession({ projectPath: s.folder('anywhere'), sessionId: SESSION }), undefined);
  s.service.claimSession(two.bot_id, SESSION);
});

test('a session that could not be started is not left claimed, and a draft that is reused is claimed once more', async (t) => {
  const s = setup(t);
  const { bot, repo, folder } = boundBot(s);
  s.failNewSession(new Error('the engine could not start'));
  await assert.rejects(s.service.openSession(bot.bot_id, undefined, undefined, true, folder.id), /could not start/);
  assert.equal(s.service.ownsSession({ projectPath: repo, sessionId: SESSION }), false);
  s.failNewSession(undefined);
  await s.service.openSession(bot.bot_id, undefined, undefined, true, folder.id);
  s.open(SESSION, repo);
  await s.service.openSession(bot.bot_id, undefined, undefined, true, folder.id);
  assert.equal(s.service.ownsSession({ projectPath: repo, sessionId: SESSION }), true);
  assert.equal(s.asked('newSession').length, 3, 'asked each time: the manager hands back the draft it has');
});

test('a chat is opened in the folder named, and only in one the Bot has and may still use', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s, 'read');
  await assert.rejects(s.service.openSession(bot.bot_id, undefined, undefined, true, 'wNoSuchFold'), /no such folder/);
  await assert.rejects(s.service.openSession(bot.bot_id, SESSION, undefined, true, folder.id), /keeps the folder it works in/);
  assert.equal((await s.service.openSession(bot.bot_id, undefined, undefined, true, DEFAULT_WORKSPACE_ID)).projectPath, own);
  assert.equal((await s.service.openSession(bot.bot_id)).projectPath, own, 'the Bot\'s own folder when none is named');
  rmSync(repo, { recursive: true });
  await assert.rejects(s.service.openSession(bot.bot_id, undefined, undefined, true, folder.id), /no longer a folder a Bot may work in/);
});

test('an existing chat is opened in the folder its transcript is kept for, and one of a folder the Bot has not any more is not', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  await s.service.listSessions(bot.bot_id);
  const inRepo = chatOf(s, bot.bot_id, SESSION, repo);
  const inOwn = chatOf(s, bot.bot_id, OTHER_SESSION, own);
  const elsewhere = chatOf(s, bot.bot_id, THIRD_SESSION, s.folder('elsewhere'));
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(inRepo, inOwn, elsewhere);
  s.calls.length = 0;
  assert.deepEqual(await s.service.openSession(bot.bot_id, SESSION, inRepo.path), { botId: bot.bot_id, projectPath: repo, sessionId: SESSION, workspaceId: folder.id });
  assert.deepEqual(await s.service.openSession(bot.bot_id, OTHER_SESSION, inOwn.path), { botId: bot.bot_id, projectPath: own, sessionId: OTHER_SESSION, workspaceId: DEFAULT_WORKSPACE_ID });
  assert.deepEqual(s.asked('openSession').map((call) => (call[1] as { projectPath: string }).projectPath), [repo, own]);
  assert.equal(s.service.ownsSession({ projectPath: repo, sessionId: SESSION }), true, 'claimed before its engine started');
  assert.deepEqual(s.store.sessionSource(bot.bot_id, SESSION), { path: inRepo.path, mode: 'code' });
  await assert.rejects(s.service.openSession(bot.bot_id, THIRD_SESSION, elsewhere.path), /not one of the Bot's any more/);
  await assert.rejects(s.service.openSession(bot.bot_id, THIRD_SESSION, `/other/${THIRD_SESSION}.jsonl`), /not one of the Bot's any more/);
  assert.equal(s.asked('openSession').length, 2);
});

test('what a Bot recorded of a session tells a new run of the app whose it is, but only in the folder its transcript is kept for', async (t) => {
  const s = setup(t);
  const { bot, repo } = boundBot(s);
  await s.service.listSessions(bot.bot_id);
  const inRepo = chatOf(s, bot.bot_id, SESSION, repo);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(inRepo);
  await s.service.openSession(bot.bot_id, SESSION, inRepo.path);
  const restarted = () => new BotService({ store: s.store, platform: 'darwin', runtimes: NO_RUNTIMES, catalogFor: NO_CATALOGS });
  assert.equal(restarted().botIdForSession({ projectPath: repo, sessionId: SESSION }), bot.bot_id, 'nothing was claimed in this run: the record is what says so');
  assert.equal(restarted().botIdForSession({ projectPath: s.folder('elsewhere'), sessionId: SESSION }), undefined, 'the same id in a folder its transcript is not kept for');
  assert.equal(restarted().botIdForSession({ projectPath: repo, sessionId: OTHER_SESSION }), undefined, 'and the owner\'s other session in that folder is still the owner\'s');
});

test('the chats of a Bot are listed from every folder it has had, each under its folder or as a folder that is gone', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  const gone = s.folder('gone');
  s.catalogs.clear();
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(
    chatOf(s, bot.bot_id, SESSION, own), chatOf(s, bot.bot_id, OTHER_SESSION, repo), chatOf(s, bot.bot_id, THIRD_SESSION, gone), chatOf(s, bot.bot_id, CONTROLLER, own),
  );
  s.controllerOf(own, CONTROLLER);
  s.calls.length = 0;
  const listed = await s.service.listSessions(bot.bot_id);
  assert.deepEqual(listed.map((chat) => [chat.uuid, chat.workspaceId, chat.removedPath]), [
    [SESSION, DEFAULT_WORKSPACE_ID, undefined], [OTHER_SESSION, folder.id, undefined], [THIRD_SESSION, undefined, gone],
  ]);
  assert.deepEqual(s.asked('list'), [['list', s.store.homeDir(bot.bot_id), own, 'all']], 'one read of the whole home, not one for each folder');
});

test('a Bot was last active when the newest transcript of any of its folders last changed', async (t) => {
  const s = setup(t);
  const { bot, repo, own } = boundBot(s);
  const touch = (folder: string, name: string, seconds: number) => {
    const directory = s.store.transcriptsDirOf(bot.bot_id, folder);
    mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, name), '{}\n');
    utimesSync(join(directory, name), seconds, seconds);
  };
  touch(own, `${SESSION}.jsonl`, 1_700_000_000);
  assert.equal(await s.service.lastActivityAt(bot.bot_id), 1_700_000_000_000);
  touch(repo, `${OTHER_SESSION}.jsonl`, 1_700_000_800);
  assert.equal(await s.service.lastActivityAt(bot.bot_id), 1_700_000_800_000, 'a chat in a folder of the owner\'s is use of the Bot');
  s.controllerOf(own, CONTROLLER);
  touch(own, `${CONTROLLER}.jsonl`, 1_700_009_999);
  assert.equal(await s.service.lastActivityAt(bot.bot_id), 1_700_000_800_000, 'and the clock watcher is not');
});

// Giving and taking folders while chats are open

/** A Bot whose chat is open in its own folder with its engine launched, as it is while the owner is using it. */
async function launchedChat(s: ReturnType<typeof setup>, mode: 'read' | 'write' = 'write') {
  const bound = boundBot(s, mode);
  s.service.claimSession(bound.bot.bot_id, SESSION);
  const runtime = s.open(SESSION, bound.own);
  await s.service.launchSettings(bound.own, SESSION);
  s.calls.length = 0;
  return { ...bound, runtime };
}

test('a folder that is added is given to the open chats, which are started again for the engine to read it', async (t) => {
  const s = setup(t);
  const { bot, own, repo } = await launchedChat(s);
  const added = s.folder('added');
  await s.service.addWorkspace(bot.bot_id, { path: added, mode: 'read' });
  await s.service.settled();
  assert.deepEqual(s.asked('grants', 'restart'), [
    ['grants', SESSION, [repo, added]],
    ['restart', { projectPath: own, sessionId: SESSION, mode: 'code' }],
  ]);
  assert.deepEqual(s.settingsOf(bot.bot_id).permissions.additionalDirectories, [own, repo, added]);
  assert.ok(s.settingsOf(bot.bot_id).permissions.deny.includes(pathRule('Edit', added)), 'a folder given to read is not to be written');
  // The engine that was started again was launched with what is now, so another change is what it is given next.
  await s.service.launchSettings(own, SESSION);
  s.calls.length = 0;
  await s.service.syncChatsForTest?.(bot.bot_id);
});

test('a folder is changed to read or write for the open chats, and a label is nothing to start a chat again for', async (t) => {
  const s = setup(t);
  const { bot, own, folder } = await launchedChat(s);
  await s.service.changeWorkspace(bot.bot_id, folder.id, { label: 'The repo' });
  await s.service.settled();
  assert.deepEqual(s.asked('grants', 'restart'), [], 'what the engine was given has not changed');
  await s.service.changeWorkspace(bot.bot_id, folder.id, { mode: 'read' });
  await s.service.settled();
  assert.deepEqual(s.asked('restart'), [['restart', { projectPath: own, sessionId: SESSION, mode: 'code' }]]);
  assert.ok(s.settingsOf(bot.bot_id).permissions.deny.includes(pathRule('Edit', folder.path)));
});

test('a chat that is working keeps what it has and is looked at again until it is idle', async (t) => {
  const s = setup(t, { retryMs: 5 });
  const { bot, own, runtime } = await launchedChat(s);
  runtime.hasActiveWork = true;
  await s.service.addWorkspace(bot.bot_id, { path: s.folder('added'), mode: 'write' });
  await s.service.settled();
  assert.deepEqual(s.asked('restart'), [], 'it is working');
  assert.equal(s.asked('grants').length, 1, 'the next launch is given the folder all the same');
  runtime.hasActiveWork = false;
  await until(() => s.asked('restart').length === 1);
  assert.deepEqual(s.asked('restart'), [['restart', { projectPath: own, sessionId: SESSION, mode: 'code' }]]);
});

test('a chat whose engine has not started yet is only told what to start with, and one that has the folders already is left alone', async (t) => {
  const s = setup(t);
  const { bot, own } = boundBot(s);
  s.service.claimSession(bot.bot_id, SESSION);
  s.open(SESSION, own);
  await s.service.addWorkspace(bot.bot_id, { path: s.folder('added'), mode: 'write' });
  await s.service.settled();
  assert.equal(s.asked('grants').length, 1);
  assert.deepEqual(s.asked('restart'), [], 'never launched, so it reads the settings when it is');
  await s.service.launchSettings(own, SESSION);
  s.calls.length = 0;
  await s.service.changeWorkspace(bot.bot_id, s.store.get(bot.bot_id).workspaces[0]!.id, { mode: 'read' });
  await s.service.settled();
  s.calls.length = 0;
  await s.service.changeWorkspace(bot.bot_id, s.store.get(bot.bot_id).workspaces[0]!.id, { mode: 'read' });
  await s.service.settled();
  assert.deepEqual(s.asked('grants', 'restart'), [], 'nothing changed');
});

test('the chats of a Bot are the only ones that are looked after, wherever they work', async (t) => {
  const s = setup(t);
  const { bot, own, repo } = await launchedChat(s);
  s.open(OTHER_SESSION, repo);
  s.open(THIRD_SESSION, own);
  s.controllerOf(own, THIRD_SESSION);
  await s.service.addWorkspace(bot.bot_id, { path: s.folder('added'), mode: 'write' });
  await s.service.settled();
  assert.deepEqual(s.asked('restart').map((call) => (call[1] as { sessionId: string }).sessionId), [SESSION], 'not the owner\'s chat in the folder bound to the Bot, and not the one that watches the clock');
});

test('what the Bot already may use is not taken away by an answer to a request made earlier', async (t) => {
  const s = setup(t);
  const { bot, repo, folder } = boundBot(s, 'write');
  const kept = await s.service.grantWorkspace(bot.bot_id, { path: repo, mode: 'read' });
  assert.equal(kept.workspaces[0]!.mode, 'write', 'a request for read, answered when the Bot has write');
  const added = s.folder('added');
  const granted = await s.service.grantWorkspace(bot.bot_id, { path: added, mode: 'read' });
  assert.deepEqual(granted.workspaces.map((workspace) => [workspace.path, workspace.mode]), [[repo, 'write'], [added, 'read']]);
  const more = await s.service.grantWorkspace(bot.bot_id, { path: added, mode: 'write' });
  assert.equal(more.workspaces[1]!.mode, 'write', 'and a request for more is what changes the mode');
  assert.equal(more.workspaces[1]!.id, workspaceIdOf(added));
  assert.equal(folder.id, workspaceIdOf(repo));
  await assert.rejects(s.service.grantWorkspace(bot.bot_id, { path: s.home, mode: 'read' }), /home folder/);
  await assert.rejects(s.service.grantWorkspace(bot.bot_id, { path: join(s.root, 'nowhere'), mode: 'read' }), /does not exist/);
});

// Taking a folder away

test('a folder that is removed ends the Bot\'s chats there and only those, keeps what was said, and is the same folder when it is added again', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  s.service.claimSession(bot.bot_id, SESSION);
  s.service.claimSession(bot.bot_id, THIRD_SESSION);
  s.open(SESSION, repo);
  s.open(OTHER_SESSION, repo);
  s.open(THIRD_SESSION, own);
  await s.service.listSessions(bot.bot_id);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(chatOf(s, bot.bot_id, SESSION, repo), chatOf(s, bot.bot_id, THIRD_SESSION, own));
  s.calls.length = 0;
  const removed = await s.service.removeWorkspace(bot.bot_id, folder.id);
  assert.deepEqual(removed.workspaces, []);
  assert.deepEqual(s.asked('closeSession', 'closeProject'), [['closeSession', { projectPath: repo, sessionId: SESSION, mode: 'code' }]], 'not the owner\'s chat in the same folder, not the Bot\'s in its own, and the folder is not closed as a project');
  assert.equal(s.service.ownsSession({ projectPath: repo, sessionId: SESSION }), false, 'a session that is gone is nobody\'s to claim');
  assert.ok(!s.settingsOf(bot.bot_id).permissions.additionalDirectories.includes(repo));
  const listed = await s.service.listSessions(bot.bot_id);
  assert.deepEqual(listed.map((chat) => [chat.uuid, chat.workspaceId, chat.removedPath]), [[SESSION, undefined, repo], [THIRD_SESSION, DEFAULT_WORKSPACE_ID, undefined]], 'its history is listed under the folder that is gone');
  await assert.rejects(s.service.openSession(bot.bot_id, SESSION, chatOf(s, bot.bot_id, SESSION, repo).path), /not one of the Bot's any more/, 'but it is not continued');
  const again = await s.service.addWorkspace(bot.bot_id, { path: repo, mode: 'read' });
  assert.equal(again.workspaces[0]!.id, folder.id, 'the same folder, so its chats are where they were');
  assert.deepEqual((await s.service.listSessions(bot.bot_id)).map((chat) => [chat.uuid, chat.workspaceId]), [[SESSION, folder.id], [THIRD_SESSION, DEFAULT_WORKSPACE_ID]]);
  assert.equal((await s.service.openSession(bot.bot_id, SESSION, chatOf(s, bot.bot_id, SESSION, repo).path)).projectPath, repo, 'and can be continued');
});

test('a folder is not taken away while a chat of the Bot is working in it, and nothing was changed', async (t) => {
  const s = setup(t);
  const { bot, repo, folder } = boundBot(s);
  s.service.claimSession(bot.bot_id, SESSION);
  s.open(SESSION, repo, { hasActiveWork: true });
  s.open(OTHER_SESSION, repo, { hasActiveWork: true });
  const before = s.store.get(bot.bot_id);
  await assert.rejects(s.service.removeWorkspace(bot.bot_id, folder.id), /A chat is working in repo/);
  await assert.rejects(s.service.update(bot.bot_id, before.revision, { workspaces: [] }), /A chat is working in repo/);
  assert.deepEqual(s.store.get(bot.bot_id), before);
  assert.deepEqual(s.asked('closeSession'), []);
  assert.ok(s.settingsOf(bot.bot_id).permissions.additionalDirectories.includes(repo));
});

test('a patch that leaves a folder out ends its chats the way removing it does, and one that keeps it does not', async (t) => {
  const s = setup(t);
  const { bot, repo, own } = boundBot(s);
  s.service.claimSession(bot.bot_id, SESSION);
  s.open(SESSION, repo);
  await s.service.update(bot.bot_id, 1, { name: 'Renamed', workspaces: [{ path: repo, mode: 'write' }] });
  assert.deepEqual(s.asked('closeSession'), []);
  const after = await s.service.update(bot.bot_id, 2, { workspaces: [{ path: s.folder('another'), mode: 'read' }] });
  assert.deepEqual(after.workspaces.map((workspace) => workspace.path), [s.folder('another')]);
  assert.deepEqual(s.asked('closeSession'), [['closeSession', { projectPath: repo, sessionId: SESSION, mode: 'code' }]]);
  assert.equal(s.service.workspaceOf(bot.bot_id), own);
});

test('the Bot\'s own folder is not one that can be given up', async (t) => {
  const s = setup(t);
  const { bot } = boundBot(s);
  await assert.rejects(s.service.removeWorkspace(bot.bot_id, DEFAULT_WORKSPACE_ID), /own folder cannot be removed/);
  await assert.rejects(s.service.changeWorkspace(bot.bot_id, DEFAULT_WORKSPACE_ID, { mode: 'read' }), /own folder cannot be changed/);
});

test('deleting a Bot ends its chats in the owner\'s folders one by one, and waits for none of them to be working', async (t) => {
  const s = setup(t);
  const { bot, repo, own } = boundBot(s);
  s.service.claimSession(bot.bot_id, SESSION);
  const working = s.open(SESSION, repo, { hasActiveWork: true });
  s.open(OTHER_SESSION, repo);
  await assert.rejects(s.service.delete(bot.bot_id), /cancel active turns/);
  assert.deepEqual(s.asked('closeSession', 'closeProject'), [], 'nothing was stopped');
  working.hasActiveWork = false;
  await s.service.delete(bot.bot_id);
  assert.deepEqual(s.asked('closeSession', 'closeProject'), [['closeProject', own], ['closeSession', { projectPath: repo, sessionId: SESSION, mode: 'code' }]], 'the folder of the owner\'s is not closed as a project: its other sessions are the owner\'s');
});

test('the folders of a Bot are told apart by the app for what the owner sees, and a Bot that is not there has none', (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  assert.deepEqual(s.service.workspacePathsOf(bot.bot_id), [own, repo]);
  assert.equal(s.service.workspaceIdAt(bot.bot_id, repo), folder.id);
  assert.equal(s.service.workspaceIdAt(bot.bot_id, own), DEFAULT_WORKSPACE_ID);
  assert.equal(s.service.workspaceIdAt(bot.bot_id, s.folder('other')), undefined);
  assert.equal(s.service.workspaceIdAt('Bot0000000000000000999', repo), undefined);
});

// What the routine scheduler asks, with several folders

test('the scheduler is told which folders a routine can run in, where each routine runs, and that a run is the Bot\'s', async (t) => {
  const s = setup(t);
  const { bot, repo, own, folder } = boundBot(s);
  const routine = s.service.routineBotFor(own)!;
  assert.deepEqual(routine.folders(), [{ id: DEFAULT_WORKSPACE_ID, path: own, label: '' }, { id: folder.id, path: repo, label: 'repo' }], 'its own folder is named by the window, a bound one by its label or by where it is');
  await s.service.changeWorkspace(bot.bot_id, folder.id, { label: 'The repo' });
  assert.equal(s.service.routineBotFor(own)!.folders()[1]!.label, 'The repo');
  assert.deepEqual(routine.folderOf('task-1'), { id: DEFAULT_WORKSPACE_ID, path: own }, 'a routine nobody said anything of runs in the Bot\'s own folder');
  routine.setFolder('task-1', folder.id);
  assert.deepEqual(routine.folderOf('task-1'), { id: folder.id, path: repo });
  assert.deepEqual(routine.chosenFolders(), { 'task-1': folder.id });
  rmSync(repo, { recursive: true });
  assert.equal(routine.folderOf('task-1'), undefined, 'a folder that is not what it was is not run in');
  assert.deepEqual(routine.folders().map((entry) => entry.id), [DEFAULT_WORKSPACE_ID]);
  routine.setFolder('task-1', undefined);
  assert.deepEqual(routine.folderOf('task-1'), { id: DEFAULT_WORKSPACE_ID, path: own });
  routine.claim(THIRD_SESSION);
  assert.equal(s.service.botIdForSession({ projectPath: s.folder('anywhere'), sessionId: THIRD_SESSION }), bot.bot_id);
});

test('the chats a routine can target are the Bot\'s in every folder it has, and a chat is where it works', async (t) => {
  const s = setup(t);
  const { bot, repo, own } = boundBot(s);
  await s.service.listSessions(bot.bot_id);
  const gone = s.folder('gone');
  const inRepo = chatOf(s, bot.bot_id, SESSION, repo);
  s.catalogs.get(s.store.homeDir(bot.bot_id))!.sessions.push(inRepo, chatOf(s, bot.bot_id, OTHER_SESSION, own), chatOf(s, bot.bot_id, THIRD_SESSION, gone));
  const routine = s.service.routineBotFor(own)!;
  s.calls.length = 0;
  const listed = await routine.catalog().list(own, 'current', (candidate) => candidate.uuid !== OTHER_SESSION);
  assert.deepEqual(listed.sessions.map((chat) => chat.uuid), [SESSION], 'every folder the Bot has, not the folder asked about; not one it has not; the filter still applies');
  assert.deepEqual(s.asked('list'), [['list', s.store.homeDir(bot.bot_id), own, 'all']]);
  s.open(SESSION, repo);
  s.service.claimSession(bot.bot_id, SESSION);
  assert.equal(routine.folderOfChat(SESSION), repo, 'a chat that is open is where it is open');
  assert.equal(routine.folderOfChat(OTHER_SESSION), undefined, 'one that is not open and has no transcript yet is nobody\'s');
});
