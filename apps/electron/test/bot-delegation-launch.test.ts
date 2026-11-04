import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { BotRouter } from '../src/main/bots/router.js';
import { ROUTER_SERVER_NAME, ROUTER_TOKEN_ENV, ROUTER_URL_ENV } from '../src/main/bots/routerTools.js';
import { BotService, type BotCatalog, type BotDelegation, type BotRouterAccess } from '../src/main/bots/service.js';
import { BotTasks } from '../src/main/bots/taskService.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { botFixture, SESSION } from './bot-helpers.js';
import { FakeRuntimes } from './bot-tasks-helpers.js';

const OTHER_SESSION = '99999999-2222-4333-8444-555555555555';
const ROUTER_URL = 'http://127.0.0.1:4242/mcp';

/** A router that hands out tokens one after another and notes who it was asked to forget. */
function stubRouter(calls: string[]) {
  const issued: Array<{ botId: string; sessionId: string; token: string }> = [];
  const state: { ready: string | Error } = { ready: ROUTER_URL };
  const router: BotRouterAccess = {
    ready: async () => { if (state.ready instanceof Error) throw state.ready; return state.ready; },
    issue: (botId, sessionId) => {
      const token = `token-${issued.length + 1}`;
      issued.push({ botId, sessionId, token });
      return token;
    },
    revokeBot: (botId) => { calls.push(`revokeBot ${botId}`); },
  };
  return { router, issued, state };
}

function setup(t: { after(fn: () => void): void }, options: { router?: boolean; delegation?: (service: () => BotService, calls: string[]) => BotDelegation; botEnvironment?: Record<string, string> } = {}) {
  const f = botFixture(t);
  const calls: string[] = [];
  const warnings: string[] = [];
  const stub = stubRouter(calls);
  const failures = { close: false };
  let service!: BotService;
  service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes: {
      newSession: async () => { throw new Error('unused'); },
      openSession: async () => ({}),
      closeProject: async () => { calls.push('closeProject'); if (failures.close) throw new Error('cancel active turns and pending interactions before removing a project'); },
    },
    catalogFor: (configHome): BotCatalog => ({
      list: async () => ({ sessions: [] }), find: async () => undefined, invalidate: () => undefined,
      workspaceSettings: async () => ({ environment: { FROM_SETTINGS: 'yes', ...options.botEnvironment }, configHome }),
    }),
    routines: { isControllerSession: () => false, releaseScope: () => { calls.push('releaseScope'); } },
    ...(options.router === false ? {} : { router: stub.router }),
    ...(options.delegation !== undefined ? { delegation: options.delegation(() => service, calls) } : {}),
    warn: (message) => warnings.push(message),
  });
  return { ...f, service, calls, warnings, failures, ...stub };
}

/** Where the engine reads the Bot's configuration file. Asked for without a session, a launch writes nothing there. */
async function configPathOf(s: ReturnType<typeof setup>, botId: string): Promise<string> {
  const launch = await s.service.launchSettings(s.store.workspaceDir(botId));
  return launch.globalConfigPath!;
}
const configOf = (path: string) => JSON.parse(readFileSync(path, 'utf8'));
const SERVER_ENTRY = { type: 'http', url: '${BOT_ROUTER_URL}', headers: { Authorization: 'Bearer ${BOT_ROUTER_TOKEN}' } };

test('a session of a Bot is launched with the router\'s address and a token of its own, and the router is among the servers of the Bot\'s configuration', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Caller' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const configPath = await configPathOf(s, bot.bot_id);
  assert.ok(!existsSync(configPath));
  const launch = await s.service.launchSettings(workspace, SESSION);
  assert.equal(launch.environment['FROM_SETTINGS'], 'yes', 'what the Bot\'s settings say is kept');
  assert.equal(launch.environment[ROUTER_URL_ENV], ROUTER_URL);
  assert.equal(launch.environment[ROUTER_TOKEN_ENV], 'token-1');
  assert.equal(Object.keys(launch.environment).length, 4, 'with the Bot\'s home, nothing else');
  assert.deepEqual(s.issued, [{ botId: bot.bot_id, sessionId: SESSION, token: 'token-1' }]);
  assert.equal(launch.globalConfigPath, configPath);
  assert.deepEqual(configOf(configPath), { mcpServers: { [ROUTER_SERVER_NAME]: SERVER_ENTRY } });
  assert.ok(!readFileSync(configPath, 'utf8').includes('token-1'), 'the configuration on disk holds no secret');

  const relaunched = await s.service.launchSettings(workspace, SESSION);
  assert.equal(relaunched.environment[ROUTER_TOKEN_ENV], 'token-2', 'a session launched again gets a new token');
  const another = await s.service.launchSettings(workspace, OTHER_SESSION);
  assert.equal(another.environment[ROUTER_TOKEN_ENV], 'token-3');
  assert.deepEqual(s.issued.map((entry) => entry.sessionId), [SESSION, SESSION, OTHER_SESSION]);
  assert.deepEqual(s.warnings, []);
});

test('the Bot\'s own settings cannot name another router, and the rest of its configuration file is kept', async (t) => {
  const s = setup(t, { botEnvironment: { [ROUTER_URL_ENV]: 'http://evil.example/mcp', [ROUTER_TOKEN_ENV]: 'stolen' } });
  const bot = s.service.create({ name: 'Caller' });
  const configPath = await configPathOf(s, bot.bot_id);
  writeFileSync(configPath, JSON.stringify({ theme: 'dark', mcpServers: { files: { type: 'stdio', command: 'x' }, [ROUTER_SERVER_NAME]: { type: 'http', url: 'http://evil.example/mcp' } } }));
  const launch = await s.service.launchSettings(s.store.workspaceDir(bot.bot_id), SESSION);
  assert.equal(launch.environment[ROUTER_URL_ENV], ROUTER_URL);
  assert.equal(launch.environment[ROUTER_TOKEN_ENV], 'token-1');
  const config = configOf(configPath);
  assert.equal(config.theme, 'dark');
  assert.deepEqual(config.mcpServers.files, { type: 'stdio', command: 'x' });
  assert.deepEqual(config.mcpServers[ROUTER_SERVER_NAME], SERVER_ENTRY, 'and the entry is the router\'s, not what the file said');
});

test('the file is written only when the entry is not right already', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Caller' });
  const configPath = await configPathOf(s, bot.bot_id);
  await s.service.launchSettings(s.store.workspaceDir(bot.bot_id), SESSION);
  const first = readFileSync(configPath, 'utf8');
  writeFileSync(configPath, first.replace(/\n/g, '\r\n'));
  await s.service.launchSettings(s.store.workspaceDir(bot.bot_id), SESSION);
  assert.equal(readFileSync(configPath, 'utf8'), first.replace(/\n/g, '\r\n'), 'left as it was');
});

test('without a router, or for a launch that names no session, the environment is what it was and the configuration is untouched', async (t) => {
  const bare = setup(t, { router: false });
  const one = bare.service.create({ name: 'Bare' });
  const configOne = await configPathOf(bare, one.bot_id);
  const launch = await bare.service.launchSettings(bare.store.workspaceDir(one.bot_id), SESSION);
  assert.deepEqual(Object.keys(launch.environment).filter((name) => name.startsWith('BOT_ROUTER_')), []);
  assert.equal(Object.keys(launch.environment).length, 2, 'what the Bot\'s settings say, and the Bot\'s home');
  assert.ok(!existsSync(configOne));

  const s = setup(t);
  const two = s.service.create({ name: 'Anonymous' });
  const configTwo = await configPathOf(s, two.bot_id);
  const anonymous = await s.service.launchSettings(s.store.workspaceDir(two.bot_id));
  assert.deepEqual(Object.keys(anonymous.environment).filter((name) => name.startsWith('BOT_ROUTER_')), []);
  assert.equal(Object.keys(anonymous.environment).length, 2);
  assert.deepEqual(s.issued, []);
  assert.ok(!existsSync(configTwo));
});

test('a router that is not there does not stop a session from launching: it runs without the other Bots, and the log says why', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Cut off' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const configPath = await configPathOf(s, bot.bot_id);
  s.state.ready = new Error('listen EADDRINUSE');
  const launch = await s.service.launchSettings(workspace, SESSION);
  assert.deepEqual(Object.keys(launch.environment).filter((name) => name.startsWith('BOT_ROUTER_')), []);
  assert.equal(launch.environment['FROM_SETTINGS'], 'yes', 'and it is launched as any other');
  assert.deepEqual(s.issued, [], 'no token for a session that cannot use it');
  assert.ok(!existsSync(configPath));
  assert.equal(s.warnings.length, 1);
  assert.match(s.warnings[0]!, new RegExp(`Bot ${bot.bot_id}: the session cannot reach the other Bots: listen EADDRINUSE`));

  s.state.ready = ROUTER_URL;
  const target = join(s.root, 'elsewhere.json');
  writeFileSync(target, '{}');
  symlinkSync(target, configPath);
  const linked = await s.service.launchSettings(workspace, SESSION);
  assert.equal(linked.environment[ROUTER_TOKEN_ENV], undefined);
  assert.deepEqual(s.issued, [], 'a configuration that is a link is not followed, and no token is made');
  assert.equal(readFileSync(target, 'utf8'), '{}');
  assert.match(s.warnings[1]!, /symbolic link/);

  unlinkSync(configPath);
  const repaired = await s.service.launchSettings(workspace, SESSION);
  assert.equal(repaired.environment[ROUTER_TOKEN_ENV], 'token-1');
  assert.equal(s.warnings.length, 2);
});

test('deleting a Bot ends its tasks before anything is stopped, and takes its sessions\' tokens away once it is gone', async (t) => {
  let deletingAtTheTime: boolean | undefined;
  const s = setup(t, {
    delegation: (service, calls) => ({
      botDeleting: async (botId) => { deletingAtTheTime = service().isDeleting(botId); calls.push(`botDeleting ${botId}`); },
    }),
  });
  const gone = s.service.create({ name: 'Gone' });
  const workspace = s.store.workspaceDir(gone.bot_id);
  await s.service.delete(gone.bot_id);
  assert.deepEqual(s.calls, [`botDeleting ${gone.bot_id}`, 'releaseScope', 'closeProject', `revokeBot ${gone.bot_id}`]);
  assert.equal(deletingAtTheTime, true, 'no task is taken for the Bot from that moment');
  assert.ok(!existsSync(s.store.botDir(gone.bot_id)));
  assert.equal(s.service.ownsWorkspace(workspace), false);
  assert.equal(s.service.isDeleting(gone.bot_id), false);
});

test('a deletion that fails leaves the Bot\'s tokens as they were, and a tasks service that cannot let go stops the deletion at once', async (t) => {
  const s = setup(t, { delegation: (_service, calls) => ({ botDeleting: async (botId) => { calls.push(`botDeleting ${botId}`); } }) });
  const busy = s.service.create({ name: 'Busy' });
  s.failures.close = true;
  await assert.rejects(s.service.delete(busy.bot_id), /cancel active turns/);
  assert.deepEqual(s.calls, [`botDeleting ${busy.bot_id}`, 'releaseScope', 'closeProject'], 'no token was taken');
  assert.ok(existsSync(s.store.botDir(busy.bot_id)));
  assert.equal(s.service.isDeleting(busy.bot_id), false);

  const stuck = setup(t, { delegation: () => ({ botDeleting: async () => { throw new Error('the tasks could not be let go of'); } }) });
  const bot = stuck.service.create({ name: 'Stuck' });
  await assert.rejects(stuck.service.delete(bot.bot_id), /could not be let go of/);
  assert.deepEqual(stuck.calls, [], 'nothing of the Bot was stopped');
  assert.ok(existsSync(stuck.store.botDir(bot.bot_id)));
  assert.equal(stuck.service.isDeleting(bot.bot_id), false);
});

test('the runtime manager tells the settings which session it is launching, so that each session\'s engine gets its own token', async (t) => {
  const s = setup(t);
  const bot = s.service.create({ name: 'Caller' });
  const workspace = s.store.workspaceDir(bot.bot_id);
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: s.service.ownsWorkspace(ref.projectPath) }),
    workspaceSettings: (projectPath, ref) => s.service.launchSettings(projectPath, ref.sessionId),
  });
  t.after(() => manager.dispose());
  const first = await manager.ensure({ projectPath: workspace, sessionId: SESSION }, false);
  const second = await manager.ensure({ projectPath: workspace, sessionId: OTHER_SESSION }, false);
  const launchOf = (runtime: unknown): Promise<{ settingsEnvironment: Record<string, string> }> => (runtime as any).opts.launchConfig();
  assert.equal((await launchOf(first)).settingsEnvironment[ROUTER_TOKEN_ENV], 'token-1');
  assert.equal((await launchOf(second)).settingsEnvironment[ROUTER_TOKEN_ENV], 'token-2');
  assert.equal((await launchOf(first)).settingsEnvironment[ROUTER_TOKEN_ENV], 'token-3', 'a session launched again is told the new token, not the one it had');
  assert.deepEqual(s.issued.map((entry) => [entry.botId, entry.sessionId]), [[bot.bot_id, SESSION], [bot.bot_id, OTHER_SESSION], [bot.bot_id, SESSION]]);
});

// The whole way, with a process

const CAPTURE = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const directory = process.argv[process.argv.indexOf('--bridge-dir') + 1];
fs.writeFileSync(path.join(directory, 'launch.json'), JSON.stringify({
  url: process.env.BOT_ROUTER_URL ?? null, token: process.env.BOT_ROUTER_TOKEN ?? null,
}));
setInterval(() => {}, 1000);
`;

async function call(url: string, token: string, method: string, params?: unknown): Promise<{ status: number; body: any }> {
  const reply = await fetch(url, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, ...(params !== undefined ? { params } : {}) }),
  });
  const text = await reply.text();
  return { status: reply.status, body: reply.status === 200 ? JSON.parse(text) : text };
}

test('the engine that is started is given the router\'s address and a token that speaks for its session, until the session is closed', { skip: process.platform === 'win32' }, async (t) => {
  const f = botFixture(t);
  const warnings: string[] = [];
  const sessions = new FakeRuntimes();
  let service!: BotService;
  const tasks = new BotTasks({ store: f.store, runtimes: sessions, available: () => true, isDeleting: (botId) => service.isDeleting(botId), warn: (message) => warnings.push(message) });
  const router = new BotRouter({ backend: tasks, warn: (message) => warnings.push(message) });
  service = new BotService({
    store: f.store,
    platform: 'darwin',
    router,
    delegation: tasks,
    runtimes: { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined },
    catalogFor: (configHome): BotCatalog => ({
      list: async () => ({ sessions: [] }), find: async () => undefined, invalidate: () => undefined,
      workspaceSettings: async () => ({ environment: {}, configHome }),
    }),
    warn: (message) => warnings.push(message),
  });
  const caller = service.create({ name: 'Caller' });
  const target = service.create({ name: 'Target' });
  service.update(target.bot_id, target.revision, { acceptFrom: [caller.bot_id] });

  const root = mkdtempSync(join(tmpdir(), 'lingxi-bot-router-spawn-'));
  const serverBin = join(root, 'capture');
  writeFileSync(serverBin, CAPTURE, { mode: 0o700 });
  const manager = new SessionRuntimeManager({
    serverBin, bridgeRoot: join(root, 'runtime'), stopTimeoutMs: 500, listProcessCommands: () => [],
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: service.ownsWorkspace(ref.projectPath) }),
    workspaceSettings: (projectPath, ref) => service.launchSettings(projectPath, ref.sessionId),
  });
  router.watchRuntimes(manager);
  t.after(async () => {
    await manager.dispose();
    await router.dispose();
    rmSync(root, { recursive: true, force: true });
  });

  const ref = { projectPath: f.store.workspaceDir(caller.bot_id), sessionId: SESSION };
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  internal.waitForLockfile = async () => '/unused-test-handshake';
  internal.connectBridgeClient = async () => internal.setState({ status: 'connected' });
  await runtime.start();
  const path = join(internal.launchDir, 'launch.json');
  const deadline = Date.now() + 5000;
  let captured: { url: string | null; token: string | null };
  for (;;) {
    try { captured = JSON.parse(readFileSync(path, 'utf8')); break; }
    catch (error) { if (Date.now() >= deadline) throw error; }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.match(captured.url!, /^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  assert.match(captured.token!, /^[A-Za-z0-9_-]{43}$/);
  const configPath = (await service.launchSettings(ref.projectPath)).globalConfigPath!;
  assert.deepEqual(configOf(configPath).mcpServers[ROUTER_SERVER_NAME], SERVER_ENTRY, 'what the engine reads names the variables, which the engine fills from its environment');

  // What the engine would send with them: the Bot is the one that holds the token, and the other Bot can be asked.
  const listed = await call(captured.url!, captured.token!, 'tools/call', { name: 'list_bots', arguments: {} });
  assert.equal(listed.status, 200);
  const bots: Array<{ id: string; accepts_tasks_from_you: boolean }> = listed.body.result.structuredContent.bots;
  assert.deepEqual(bots.map((bot) => [bot.id, bot.accepts_tasks_from_you]), [[target.bot_id, true]], 'the caller is not on its own list');
  const delegated = await call(captured.url!, captured.token!, 'tools/call', { name: 'delegate_task', arguments: { bot: target.bot_id, goal: 'Summarise the notes' } });
  assert.notEqual(delegated.body.result.isError, true, JSON.stringify(delegated.body));
  const [task] = tasks.list();
  assert.deepEqual([task!.sender, task!.receiver.bot_id, task!.goal], [{ bot_id: caller.bot_id, session_id: SESSION }, target.bot_id, 'Summarise the notes']);

  // The token is that of the session: a session launched again has a new one, and a closed one has none.
  const again = (await internal.opts.launchConfig()).settingsEnvironment[ROUTER_TOKEN_ENV] as string;
  assert.notEqual(again, captured.token);
  assert.equal((await call(captured.url!, captured.token!, 'ping')).status, 401, 'the earlier token no longer speaks for the session');
  assert.equal((await call(captured.url!, again, 'ping')).status, 200);
  await manager.closeSession(ref);
  assert.equal((await call(captured.url!, again, 'ping')).status, 401, 'the session is closed and its token is worth nothing');
  assert.deepEqual(warnings, []);
});
