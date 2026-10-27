import { test, type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';

import { BotContent } from '../src/main/bots/content.js';
import { BotsUi } from '../src/main/bots/rendererIpc.js';
import { BotService } from '../src/main/bots/service.js';
import { HostController, type WorkspaceDialogs } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { ScheduledTaskService } from '../src/main/scheduled.js';
import { SettingsStore } from '../src/main/settings.js';
import { CH_BOTS_CREATE, CH_BOTS_DELETE, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSION_SHOWN } from '../src/shared/bots.js';
import { CH_SCHEDULED } from '../src/shared/scheduled.js';
import { CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK } from '../src/shared/projectCreation.js';
import { botFixture, noTasks, SESSION } from './bot-helpers.js';

const OWNER_SESSION = 'aaaaaaaa-2222-4333-8444-555555555555';
const REPLACEMENT = 'bbbbbbbb-2222-4333-8444-555555555555';
/** A chat a routine started in a Bot's workspace. */
const RUN_SESSION = 'cccccccc-2222-4333-8444-555555555555';

function hostFixture(t: TestContext) {
  const f = botFixture(t);
  // The fixture's stand-in for the app's settings is not a settings file the store can read.
  rmSync(join(f.userData, 'settings.v1.json'));
  const settings = new SettingsStore(f.userData);
  const project = f.folder('project');
  const ownerRef = { projectPath: project, sessionId: OWNER_SESSION };
  settings.addProject(project);
  settings.activateProject(project);
  settings.setActiveSession(ownerRef);
  const picks: string[] = [];
  let pickResult: string | undefined;
  const closed: string[] = [];
  const created: string[] = [];
  let summaries: any[] = [];
  const sent: Array<[string, unknown]> = [];
  /** The sessions the Bots' own catalog has, by id. */
  const botSessions = new Map<string, { uuid: string; empty_session: boolean; resume_model?: string }>();
  const bridge = {
    registerIpc() {}, registerWindow() {},
    get runtimeSummaries() { return summaries; },
    get: () => undefined,
    isProjectClosing: () => false,
    hasActiveWork: () => false,
    closeSession: async (ref: { sessionId: string }) => { closed.push(ref.sessionId); },
    newSession: async (projectPath: string) => { created.push(projectPath); return { projectPath, sessionId: REPLACEMENT }; },
  };
  const botWorkspaces = new Map<string, string>();
  // As the app wires them: the scheduler is made first and learns of the Bots afterwards.
  let scheduled: ScheduledTaskService | undefined;
  const service = new BotService({
    store: f.store, platform: 'darwin',
    runtimes: {
      newSession: async (projectPath) => ({ projectPath, sessionId: SESSION }),
      openSession: async (ref) => ({ ...ref }),
      closeProject: async (projectPath) => { closed.push(projectPath); },
    },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }), find: async (_path, sessionId) => botSessions.get(sessionId.toLowerCase()) as any, invalidate: () => undefined,
      workspaceSettings: async () => ({ environment: {}, configHome: '/unused' }),
    }),
    routines: {
      isControllerSession: (workspace, sessionId) => scheduled?.isControllerSession(workspace, sessionId) === true,
      releaseScope: (workspace) => scheduled?.releaseScope(workspace),
    },
  });
  const registered = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => registered.set(name, handler), removeHandler: (name: string) => registered.delete(name) };
  const dialogs: WorkspaceDialogs = {
    pickDirectory: async (path) => { picks.push(path); return pickResult; },
    trustDirectory: async () => true,
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any, undefined, undefined, undefined, undefined, dialogs);
  host.attachBots(new BotsUi({ service, content: new BotContent(f.store), tasks: noTasks }));
  scheduled = new ScheduledTaskService(settings, bridge as any, {} as any, () => false, () => undefined);
  scheduled.attachBots(service);
  host.attachScheduled(scheduled);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send(channel: string, payload: unknown) { sent.push([channel, payload]); } };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  t.after(() => host.dispose());
  const event = { sender, senderFrame: frame };
  const call = async (channel: string, ...args: unknown[]): Promise<any> => registered.get(channel)!(event, ...args);
  return {
    ...f, settings, project, ownerRef, host, service, scheduled, registered, call, closed, created, picks, sent, botSessions,
    pickReturns(path: string | undefined) { pickResult = path; },
    setSummaries(value: any[]) { summaries = value; },
    freshSettings: () => new SettingsStore(f.userData),
  };
}

async function newBot(f: ReturnType<typeof hostFixture>) {
  const bot = await f.call(CH_BOTS_CREATE, { name: 'Helper' });
  return { bot, workspace: f.store.workspaceDir(bot.botId) };
}

test('opening a Bot\'s session shows it in the window without making it a project or the session to restore', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  const opened = await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  assert.deepEqual(opened, { botId: bot.botId, projectPath: workspace, sessionId: SESSION });
  assert.deepEqual(f.settings.getPublic().activeSession, { projectPath: workspace, sessionId: SESSION }, 'the window is shown the Bot\'s session');
  assert.deepEqual(f.settings.getPublic().projects, [f.project], 'and no project was added');
  assert.deepEqual(f.freshSettings().getPublic().activeSession, f.ownerRef, 'but the session the app restores is still the owner\'s');
  assert.ok(!readFileSync(f.settings.settingsPath, 'utf8').includes(workspace), 'nothing of the Bot is in the saved settings');
  assert.deepEqual((f.host as any).workspace(), { path: workspace, fingerprint: (f.host as any).workspace().fingerprint, trusted: true }, 'its folder is trusted without being a project');
});

test('the owner\'s own session shows again when the owner opens it', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  assert.equal(f.settings.getPublic().activeSession?.projectPath, workspace);
  f.settings.setActiveSession(f.ownerRef);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef);
  assert.equal((f.host as any).workspace().path, f.project);
  assert.equal((f.host as any).workspace().trusted, true);
});

test('a folder that is not a project and not a Bot\'s stays untrusted', (t) => {
  const f = hostFixture(t);
  f.settings.setActiveBotSession({ projectPath: f.folder('stranger'), sessionId: SESSION });
  assert.equal((f.host as any).workspace().trusted, false);
});

test('the sessions of the runtime manager say which of them are a Bot\'s', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  const summary = (projectPath: string, sessionId: string) => ({ projectPath, sessionId, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 });
  f.setSummaries([summary(workspace, SESSION), summary(f.project, OWNER_SESSION)]);
  const [botSession, ownerSession] = (f.host as any).runtimeSummaries();
  assert.equal(botSession.botId, bot.botId);
  assert.ok(!('botId' in ownerSession));
  assert.deepEqual(botSession.connection, { status: 'connected' });
});

test('everything a project can do stays closed to the folder of a Bot', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  const channels = [...f.registered.keys()];
  const projectChannels = channels.filter((name) => /project:sessions:list|session:new|session:open|session:archive|session:rename/.test(name));
  assert.ok(projectChannels.length >= 3, `found the project channels: ${projectChannels.join(', ')}`);
  for (const channel of projectChannels) {
    await assert.rejects(f.call(channel, workspace, SESSION), /project is not in the project list/, channel);
  }
  assert.deepEqual(f.settings.getPublic().projects, [f.project]);
});

test('deleting the Bot whose session is shown brings back the session the window had', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  assert.equal(f.settings.getPublic().activeSession?.projectPath, workspace);
  await f.call(CH_BOTS_DELETE, bot.botId);
  assert.deepEqual(f.closed, [workspace]);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef);
});

test('deleting a Bot leaves the session the window shows alone when it is not the Bot\'s', async (t) => {
  const f = hostFixture(t);
  const { bot } = await newBot(f);
  const other = await f.call(CH_BOTS_CREATE, { name: 'Other' });
  await f.call(CH_BOTS_SESSION_OPEN, other.botId);
  const shown = f.settings.getPublic().activeSession;
  await f.call(CH_BOTS_DELETE, bot.botId);
  assert.deepEqual(f.settings.getPublic().activeSession, shown);
});

test('clearing a Bot\'s chat starts a new chat of the same Bot without touching the projects', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  await f.call('lingxi:session:clear', SESSION);
  assert.deepEqual(f.created, [workspace]);
  assert.deepEqual(f.settings.getPublic().activeSession, { projectPath: workspace, sessionId: REPLACEMENT });
  assert.deepEqual(f.settings.getPublic().projects, [f.project]);
  assert.equal(f.settings.getPublic().activeProject, f.project, 'the owner\'s active project is not the Bot\'s folder');
  assert.deepEqual(f.freshSettings().getPublic().activeSession, f.ownerRef);
});

test('a Bot\'s chat cannot move to another folder or take one on', async (t) => {
  const f = hostFixture(t);
  const { bot } = await newBot(f);
  await f.call(CH_BOTS_SESSION_OPEN, bot.botId);
  for (const action of ['cd', 'add-dir'] as const) {
    await assert.rejects(f.host.updateSessionWorkspace(SESSION, { action, path: f.folder('elsewhere') }), /A Bot works in its own folder/);
  }
  assert.deepEqual(f.picks, [], 'the owner is not even asked to pick a folder');
  assert.deepEqual(f.settings.getPublic().projects, [f.project]);
});

// The app's own data is no place for a project

test('the app\'s own data folder, and the Bots in it, cannot be added as a project', async (t) => {
  const f = hostFixture(t);
  const { workspace } = await newBot(f);
  mkdirSync(join(f.userData, 'bots-trash'), { recursive: true });
  const places = [
    f.userData, f.store.root, f.store.botDir(f.service.list()[0]!.bot_id), workspace,
    join(f.userData, 'logs'), join(f.userData, 'bridge-runtime'), join(f.userData, 'bots-trash'),
  ];
  for (const place of places) {
    f.pickReturns(place);
    const selection = await f.call(CH_PROJECT_FOLDER_PICK);
    assert.equal(selection.path, realpathSync(place));
    await assert.rejects(f.call(CH_PROJECT_CREATE, selection.id), /belongs to the app's own data/, place);
  }
  assert.deepEqual(f.settings.getPublic().projects, [f.project]);
  assert.deepEqual(f.freshSettings().getPublic().projects, [f.project]);
});

test('an ordinary folder can still be added as a project', async (t) => {
  const f = hostFixture(t);
  const repo = f.folder('repo');
  f.pickReturns(repo);
  const selection = await f.call(CH_PROJECT_FOLDER_PICK);
  const workspace = await f.call(CH_PROJECT_CREATE, selection.id);
  assert.equal(workspace.path, repo);
  assert.ok(f.settings.getPublic().projects.includes(repo));
});

test('which folders are the app\'s own data: all of userData but the folder the app manages for scheduled tasks', (t) => {
  const f = botFixture(t);
  rmSync(join(f.userData, 'settings.v1.json'));
  const settings = new SettingsStore(f.userData);
  const real = realpathSync(f.userData);
  assert.equal(settings.isAppDataFolder(real), true);
  assert.equal(settings.isAppDataFolder(join(real, 'logs')), true);
  assert.equal(settings.isAppDataFolder(join(real, 'bots')), true);
  assert.equal(settings.isAppDataFolder(join(real, 'bots', 'x', 'workspace')), true);
  assert.equal(settings.isAppDataFolder(join(real, 'bots-trash')), true);
  assert.equal(settings.isAppDataFolder(join(real, 'scheduled-workspace-two')), true, 'a folder that merely starts with the same letters');
  assert.equal(settings.isAppDataFolder(settings.scheduledWorkspace), false, 'the one the app manages itself');
  assert.equal(settings.isAppDataFolder(join(settings.scheduledWorkspace, 'sub')), false);
  assert.equal(settings.isAppDataFolder(join(real, '..')), false, 'a folder that holds the app data is not part of it');
  assert.equal(settings.isAppDataFolder(f.folder('repo')), false);
  assert.equal(settings.isAppDataFolder(`${real}-sibling`), false);
});

test('the app data is recognised when it is reached through a link', (t) => {
  const f = botFixture(t);
  const link = join(f.root, 'link');
  symlinkSync(f.userData, link);
  rmSync(join(f.userData, 'settings.v1.json'));
  const settings = new SettingsStore(link);
  assert.equal(settings.isAppDataFolder(realpathSync(join(link, 'bots'))), true);
  assert.equal(settings.isAppDataFolder(realpathSync(f.folder('repo'))), false);
});

// The routines of a Bot

function runSession(f: ReturnType<typeof hostFixture>) {
  f.botSessions.set(RUN_SESSION, { uuid: RUN_SESSION, empty_session: false });
}

test('the chat of a routine\'s run opens as a chat of the Bot, not of a project', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  runSession(f);
  assert.deepEqual(f.scheduled.scopes().filter((scope) => scope.botId === bot.botId).map((scope) => scope.id), [workspace], 'the Bot\'s workspace is a scope');
  const opened = await f.call(CH_SCHEDULED, { action: 'open', scopeId: workspace, sessionId: RUN_SESSION });
  assert.deepEqual(opened, { botId: bot.botId, projectPath: workspace, sessionId: RUN_SESSION });
  assert.deepEqual(f.settings.getPublic().activeSession, { projectPath: workspace, sessionId: RUN_SESSION }, 'the window is shown the run\'s chat');
  assert.deepEqual(f.settings.getPublic().projects, [f.project], 'and no project was added');
  assert.deepEqual(f.freshSettings().getPublic().activeSession, f.ownerRef, 'the session the app restores is still the owner\'s');
});

test('a routine\'s chat is not opened through a Bot that does not have it, or that is gone', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  await assert.rejects(f.call(CH_SCHEDULED, { action: 'open', scopeId: workspace, sessionId: RUN_SESSION }), /does not belong to this Bot/);
  await assert.rejects(f.call(CH_SCHEDULED, { action: 'open', scopeId: workspace, sessionId: 'not-a-session' }), /Invalid scheduled session/);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef, 'the window keeps what it showed');
  runSession(f);
  await f.call(CH_BOTS_DELETE, bot.botId);
  await assert.rejects(f.call(CH_SCHEDULED, { action: 'open', scopeId: workspace, sessionId: RUN_SESSION }), /This Bot is unavailable/);
});

test('a routine of a project still opens its chat the way a project\'s chat opens', async (t) => {
  const f = hostFixture(t);
  await newBot(f);
  const opened: unknown[] = [];
  (f.host as any).openSessionAndActivateInternal = async (ref: unknown) => { opened.push(ref); return {}; };
  await f.call(CH_SCHEDULED, { action: 'open', scopeId: f.project, sessionId: RUN_SESSION });
  assert.deepEqual(opened, [{ projectPath: f.project, sessionId: RUN_SESSION }]);
});

test('clicking the notification of a Bot\'s routine shows its chat and tells the window', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  runSession(f);
  await f.host.showNotifiedSession({ projectPath: workspace, sessionId: RUN_SESSION });
  assert.deepEqual(f.settings.getPublic().activeSession, { projectPath: workspace, sessionId: RUN_SESSION });
  assert.deepEqual(f.sent.filter(([channel]) => channel === CH_BOTS_SESSION_SHOWN), [[CH_BOTS_SESSION_SHOWN, { botId: bot.botId, projectPath: workspace, sessionId: RUN_SESSION }]]);
  assert.deepEqual(f.settings.getPublic().projects, [f.project]);
  assert.deepEqual(f.freshSettings().getPublic().activeSession, f.ownerRef);
});

test('a notification that names a chat the Bot no longer has shows nothing and tells the window nothing', async (t) => {
  const f = hostFixture(t);
  const { workspace } = await newBot(f);
  await assert.rejects(f.host.showNotifiedSession({ projectPath: workspace, sessionId: RUN_SESSION }), /does not belong to this Bot/);
  assert.deepEqual(f.sent.filter(([channel]) => channel === CH_BOTS_SESSION_SHOWN), []);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef);
});

test('a notification of a project restores the project\'s session as it did, and the window is not told of a Bot', async (t) => {
  const f = hostFixture(t);
  await newBot(f);
  const restored: unknown[][] = [];
  (f.host as any).restoreProjectSession = async (...args: unknown[]) => { restored.push(args); return f.ownerRef; };
  await f.host.showNotifiedSession(f.ownerRef);
  assert.deepEqual(restored, [[f.project, f.ownerRef]]);
  assert.deepEqual(f.sent, []);
});

test('a notification of a Bot is taken in turn with the other navigation', async (t) => {
  const f = hostFixture(t);
  const { workspace } = await newBot(f);
  runSession(f);
  const order: string[] = [];
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  (f.host as any).navigationQueue = (f.host as any).navigationQueue.then(async () => { order.push('earlier navigation'); await blocked; });
  const notified = f.host.showNotifiedSession({ projectPath: workspace, sessionId: RUN_SESSION }).then(() => order.push('notification'));
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef, 'it waits for the navigation already under way');
  release();
  await notified;
  assert.deepEqual(order, ['earlier navigation', 'notification']);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, RUN_SESSION);
});

test('the hidden session that watches a Bot\'s clock does not count among the Bot\'s running chats', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  const controllerOf = (path: string): string => (f.scheduled as any).controllerRef(path).sessionId;
  const summary = (projectPath: string, sessionId: string) => ({ projectPath, sessionId, connection: { status: 'connected' }, turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 });
  f.setSummaries([summary(workspace, controllerOf(workspace)), summary(workspace, SESSION), summary(f.project, controllerOf(f.project))]);
  const summaries = (f.host as any).runtimeSummaries();
  assert.deepEqual(summaries.map((item: any) => [item.projectPath, item.sessionId, item.botId]), [
    [workspace, SESSION, bot.botId],
    [f.project, controllerOf(f.project), undefined],
  ], 'the Bot\'s chat stays, the Bot\'s controller goes, and a project is left as it was');
});

test('deleting a Bot lets the scheduler go of it before its sessions are stopped', async (t) => {
  const f = hostFixture(t);
  const { bot, workspace } = await newBot(f);
  const order: string[] = [];
  const release = f.scheduled.releaseScope.bind(f.scheduled);
  f.scheduled.releaseScope = (path: string) => { order.push(`release ${path}`); release(path); };
  const close = f.closed.push.bind(f.closed);
  f.closed.push = (...paths: string[]) => { order.push(`close ${paths.join()}`); return close(...paths); };
  await f.call(CH_BOTS_DELETE, bot.botId);
  assert.deepEqual(order, [`release ${workspace}`, `close ${workspace}`]);
  assert.equal(f.scheduled.scopes().some((scope) => scope.botId === bot.botId), false);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ownerRef);
});
