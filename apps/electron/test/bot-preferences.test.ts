import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import type { ClientCommand, PermissionModeId } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { ownerPreferenceHooks } from '../src/main/bots/ownerPreferences.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { SettingsStore } from '../src/main/settings.js';

const OWNER_REF = { projectPath: '/projects/owners', sessionId: '11111111-2222-4333-8444-555555555555' };
const BOT_REF = { projectPath: '/bots/abc/workspace', sessionId: '99999999-2222-4333-8444-555555555555' };
const isBot = (ref: { projectPath: string }) => ref.projectPath.startsWith('/bots/');

/** The runtime the manager would build for a session, with a client that answers nothing until told to. */
function sessionOf(manager: SessionRuntimeManager, ref: typeof OWNER_REF) {
  const commands: ClientCommand[] = [];
  const client = Object.assign(new EventEmitter(), { sendCommand: (command: ClientCommand) => { commands.push(command); } });
  const instance = new SessionRuntime((manager as any).runtimeOptions(ref));
  Object.assign(instance, { client, activeWorkspace: ref.projectPath, activeWorkspaceTrusted: true });
  (instance as any).wireClient(client, 0);
  return { instance, client, commands };
}

function fixture(t: { after(fn: () => void): void }) {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-bot-preferences-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const settings = new SettingsStore(dir);
  settings.setLastPermissionMode('acceptEdits');
  settings.setLastFastMode(true);
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    ...ownerPreferenceHooks(settings, isBot),
  });
  return { dir, settings, manager };
}

const acknowledge = (client: EventEmitter, mode: PermissionModeId) => client.emit('event', { type: 'permission_mode_changed', mode });
const typesOf = (commands: ClientCommand[]) => commands.filter((command) => command.type !== 'get_audio_session_context');

test('the hooks give the owner\'s sessions the owner\'s choices and a Bot\'s sessions none', () => {
  const calls: unknown[][] = [];
  const hooks = ownerPreferenceHooks({
    getLastPermissionMode: () => 'plan', setLastPermissionMode: (mode) => { calls.push(['mode', mode]); },
    getLastFastMode: () => true, setLastFastMode: (enabled) => { calls.push(['fast', enabled]); },
  }, isBot);
  assert.equal(hooks.getSavedPermissionMode(OWNER_REF), 'plan');
  assert.equal(hooks.getSavedFastMode(OWNER_REF), true);
  assert.equal(hooks.getSavedPermissionMode(BOT_REF), undefined);
  assert.equal(hooks.getSavedFastMode(BOT_REF), undefined);
  hooks.onPermissionModeSelected(OWNER_REF, 'auto');
  hooks.onFastModeSelected(OWNER_REF, false);
  hooks.onPermissionModeSelected(BOT_REF, 'dontAsk');
  hooks.onFastModeSelected(BOT_REF, false);
  assert.deepEqual(calls, [['mode', 'auto'], ['fast', false]], 'only the owner\'s sessions write');
});

test('a session of the owner starts in the owner\'s last mode and Fast mode', async (t) => {
  const f = fixture(t);
  const owner = sessionOf(f.manager, OWNER_REF);
  const mode = (owner.instance as any).restorePermissionMode();
  const fast = (owner.instance as any).restoreFastMode();
  assert.deepEqual(typesOf(owner.commands), [{ type: 'set_permission_mode', mode: 'acceptEdits' }, { type: 'set_fast_mode', enabled: true }]);
  acknowledge(owner.client, 'acceptEdits');
  owner.client.emit('event', { type: 'fast_mode_changed', enabled: true });
  await Promise.all([mode, fast]);
});

test('a session of a Bot starts in neither: the Bot\'s own mode, which the engine reads from its settings, applies', async (t) => {
  const f = fixture(t);
  const bot = sessionOf(f.manager, BOT_REF);
  await (bot.instance as any).restorePermissionMode();
  await (bot.instance as any).restoreFastMode();
  assert.deepEqual(bot.commands, [], 'no mode or Fast mode is imposed on the Bot');
});

test('a mode chosen in a session of the owner is remembered, and one chosen in a Bot\'s session lasts for that session only', async (t) => {
  const f = fixture(t);
  const bot = sessionOf(f.manager, BOT_REF);
  const botPending = bot.instance.dispatchCommand({ type: 'set_permission_mode', mode: 'dontAsk' });
  assert.deepEqual(bot.commands, [{ type: 'set_permission_mode', mode: 'dontAsk' }], 'the engine is told');
  acknowledge(bot.client, 'dontAsk');
  await botPending;
  const botFast = bot.instance.dispatchCommand({ type: 'set_fast_mode', enabled: false });
  bot.client.emit('event', { type: 'fast_mode_changed', enabled: false });
  await botFast;
  assert.equal(f.settings.getLastPermissionMode(), 'acceptEdits', 'the owner\'s mode is as they left it');
  assert.equal(f.settings.getLastFastMode(), true);
  assert.equal(new SettingsStore(f.dir).getLastPermissionMode(), 'acceptEdits', 'on disk too');

  const owner = sessionOf(f.manager, OWNER_REF);
  const ownerPending = owner.instance.dispatchCommand({ type: 'set_permission_mode', mode: 'plan' });
  acknowledge(owner.client, 'plan');
  await ownerPending;
  const ownerFast = owner.instance.dispatchCommand({ type: 'set_fast_mode', enabled: false });
  owner.client.emit('event', { type: 'fast_mode_changed', enabled: false });
  await ownerFast;
  assert.equal(f.settings.getLastPermissionMode(), 'plan');
  assert.equal(f.settings.getLastFastMode(), false);
});

test('the manager asks about the session it is building the runtime for', (t) => {
  const f = fixture(t);
  const bot = (f.manager as any).runtimeOptions(BOT_REF);
  const owner = (f.manager as any).runtimeOptions(OWNER_REF);
  assert.equal(bot.getSavedPermissionMode(), undefined);
  assert.equal(owner.getSavedPermissionMode(), 'acceptEdits');
  assert.equal(bot.getSavedFastMode(), undefined);
  assert.equal(owner.getSavedFastMode(), true);
});
