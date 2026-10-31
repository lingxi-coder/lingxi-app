import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { BotService } from '../src/main/bots/service.js';
import type { BridgeLaunchConfig } from '../src/main/bridgeTypes.js';
import { buildBridgeArguments } from '../src/main/host-utils.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { botFixture, SESSION } from './bot-helpers.js';

const OTHER = '22222222-3333-4444-8555-666666666666';

test('a workspace\'s launch settings carry its directory grants unless the session has grants of its own', async (t) => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    workspaceSettings: async (projectPath) => projectPath === '/bot'
      ? { environment: {}, configHome: '/bot-home', additionalDirectories: ['/repo/one', '/repo/two'] }
      : { environment: {}, configHome: '/owner-home' },
  });
  t.after(() => manager.dispose());
  const bot = await manager.ensure({ projectPath: '/bot', sessionId: SESSION }, false);
  const project = await manager.ensure({ projectPath: '/project', sessionId: OTHER }, false);
  assert.deepEqual((await (bot as any).opts.launchConfig()).additionalDirectories, ['/repo/one', '/repo/two']);
  assert.equal((await (project as any).opts.launchConfig()).additionalDirectories, undefined, 'a project is launched as before');
  manager.setSessionDirectoryGrants(SESSION, ['/granted']);
  assert.deepEqual((await (bot as any).opts.launchConfig()).additionalDirectories, ['/granted'], 'a grant of the session wins');
  assert.equal((await (bot as any).opts.launchConfig()).configHome, '/bot-home');
});

test('the engine of a Bot is started in the Bot workspace with the Bot home, its folders and the trust the owner gave by making it', async (t) => {
  const f = botFixture(t);
  const repo = f.folder('repo');
  const docs = f.folder('docs');
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes: { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }), find: async () => undefined, invalidate: () => undefined,
      workspaceSettings: async () => ({ environment: { FROM_SETTINGS: 'x' }, configHome: '/ignored' }),
    }),
  });
  const bot = service.create({ name: 'Launch', model: 'a/b', workspaces: [{ path: repo, mode: 'write' }, { path: docs, mode: 'read' }] });
  const workspace = f.store.workspaceDir(bot.bot_id);
  const home = f.store.homeDir(bot.bot_id);
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: service.ownsWorkspace(ref.projectPath), model: service.modelFor(ref.projectPath) }),
    workspaceSettings: (projectPath) => service.launchSettings(projectPath),
  });
  t.after(() => manager.dispose());
  const runtime = await manager.ensure({ projectPath: workspace, sessionId: SESSION }, false);
  const launch: BridgeLaunchConfig = await (runtime as any).opts.launchConfig();
  assert.equal(launch.workspace, workspace);
  assert.equal(launch.trusted, true);
  assert.equal(launch.model, 'a/b');
  assert.equal(launch.configHome, home);
  assert.equal(launch.globalConfigPath, join(home, '.lingxi.json'));
  assert.deepEqual(launch.additionalDirectories, [repo, docs]);
  assert.deepEqual(launch.settingsEnvironment, { FROM_SETTINGS: 'x', LINGXI_CONFIG_DIR: home });
  assert.deepEqual(buildBridgeArguments({
    workspace: launch.workspace, bridgeDir: '/bridge', sessionId: SESSION, configHome: launch.configHome, globalConfigPath: launch.globalConfigPath,
    additionalDirectories: launch.additionalDirectories, model: launch.model, hasApiKey: false, trusted: launch.trusted,
  }), [
    '--cwd', workspace, '--bridge-dir', '/bridge', '--session-id', SESSION,
    '--session-config-home', home, '--session-global-config-path', join(home, '.lingxi.json'),
    '--add-dir', repo, '--add-dir', docs, '--model', 'a/b', '--trusted-workspace',
  ]);
});

test('the process that is started reads the Bot home, whatever the owner\'s environment says', { skip: process.platform === 'win32' }, async (t) => {
  const f = botFixture(t);
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes: { newSession: async () => { throw new Error('unused'); }, openSession: async () => ({}), closeProject: async () => undefined },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }), find: async () => undefined, invalidate: () => undefined,
      // The Bot's own settings try to point the engine back at the owner's home.
      workspaceSettings: async () => ({ environment: { LINGXI_CONFIG_DIR: f.defaultConfigHome, KEPT: 'yes' }, configHome: f.defaultConfigHome }),
    }),
  });
  const bot = service.create({ name: 'Process' });
  const root = mkdtempSync(join(tmpdir(), 'lingxi-bot-spawn-'));
  const serverBin = join(root, 'capture');
  writeFileSync(serverBin, `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const directory = process.argv[process.argv.indexOf('--bridge-dir') + 1];
fs.writeFileSync(path.join(directory, 'launch.json'), JSON.stringify({ argv: process.argv.slice(2), configDir: process.env.LINGXI_CONFIG_DIR ?? null, kept: process.env.KEPT ?? null, cwd: process.cwd() }));
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  const owner = process.env['LINGXI_CONFIG_DIR'];
  process.env['LINGXI_CONFIG_DIR'] = f.defaultConfigHome;
  const manager = new SessionRuntimeManager({
    serverBin, bridgeRoot: join(root, 'runtime'), stopTimeoutMs: 500, listProcessCommands: () => [],
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: service.ownsWorkspace(ref.projectPath) }),
    workspaceSettings: (projectPath) => service.launchSettings(projectPath),
  });
  t.after(async () => {
    await manager.dispose();
    if (owner === undefined) delete process.env['LINGXI_CONFIG_DIR']; else process.env['LINGXI_CONFIG_DIR'] = owner;
    rmSync(root, { recursive: true, force: true });
  });
  const runtime = await manager.ensure({ projectPath: f.store.workspaceDir(bot.bot_id), sessionId: SESSION }, false);
  const internal = runtime as any;
  internal.waitForLockfile = async () => '/unused-test-handshake';
  internal.connectBridgeClient = async () => internal.setState({ status: 'connected' });
  await runtime.start();
  const path = join(internal.launchDir, 'launch.json');
  const deadline = Date.now() + 5000;
  let captured: any;
  for (;;) {
    try { captured = JSON.parse(readFileSync(path, 'utf8')); break; }
    catch (error) { if (Date.now() >= deadline) throw error; }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const home = f.store.homeDir(bot.bot_id);
  assert.equal(captured.configDir, home, 'the owner\'s home, from the environment or from the Bot\'s own settings, does not reach the engine');
  assert.equal(captured.kept, 'yes');
  assert.equal(captured.cwd, f.store.workspaceDir(bot.bot_id));
  assert.deepEqual(captured.argv.slice(captured.argv.indexOf('--session-config-home'), captured.argv.indexOf('--session-config-home') + 4),
    ['--session-config-home', home, '--session-global-config-path', join(home, '.lingxi.json')]);
  assert.ok(captured.argv.includes('--trusted-workspace'));
});
