import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { CH_COMMAND, ipcMain } from '../src/main/bridgeIpc.js';
import { SettingsStore } from '../src/main/settings.js';
import { validateClientCommand } from '../src/main/validation.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const tick = () => new Promise(resolve => setImmediate(resolve));

test('mode commands persist the device preference only after the same session confirms', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-session-mode-'));
  try {
    const store = new SettingsStore(dir);
    const sent: unknown[] = [];
    const client = Object.assign(new EventEmitter(), { sendCommand: (command: unknown) => sent.push(command) });
    const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace',
      launchConfig: () => ({ workspace: '/workspace', trusted: true }),
      onSessionModeSelected: mode => store.setLastSessionMode(mode) });
    runtime.sessionMode = 'code';
    Object.assign(runtime, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true, activeTurn: true });
    (runtime as any).wireClient(client, 0);
    const switching = runtime.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    await tick();
    assert.deepEqual(sent, [{ type: 'set_session_mode', mode: 'chat' }]);
    assert.equal(store.getLastSessionMode(), 'code');
    client.emit('event', { type: 'session_mode_changed', session_id: 'other', mode: 'chat', revision: 1 });
    assert.equal(store.getLastSessionMode(), 'code');
    client.emit('event', { type: 'session_mode_changed', session_id: sessionId, mode: 'code', pending_mode: 'chat', revision: 1 });
    await switching;
    assert.equal(runtime.sessionMode, 'code');
    assert.deepEqual(runtime.replaySnapshot().map(({ event }) => event), [{ type: 'session_mode_changed', session_id: sessionId, mode: 'code', pending_mode: 'chat', revision: 1 }]);
    assert.equal(new SettingsStore(dir).getLastSessionMode(), 'chat');
    client.emit('event', { type: 'session_mode_changed', session_id: sessionId, mode: 'chat', revision: 1 });
    assert.equal(runtime.sessionMode, 'chat');
    client.emit('event', { type: 'session_mode_changed', session_id: sessionId, mode: 'code', pending_mode: 'chat', revision: 1 });
    assert.equal(runtime.sessionMode, 'chat', 'a delayed pending echo cannot undo an applied boundary');
    assert.deepEqual(runtime.replaySnapshot().map(({ event }) => event), [{ type: 'session_mode_changed', session_id: sessionId, mode: 'chat', revision: 1 }]);
    const failing = runtime.dispatchCommand({ type: 'set_session_mode', mode: 'code' });
    await tick();
    client.emit('event', { type: 'error', kind: { type: 'internal' }, message: 'set_session_mode failed: disk error' });
    await assert.rejects(failing, /disk error/);
    assert.equal(new SettingsStore(dir).getLastSessionMode(), 'chat');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('desktop command validation restricts the mode vocabulary and rejects extra fields', () => {
  assert.deepEqual(validateClientCommand({ type: 'set_session_mode', mode: 'chat' }), { type: 'set_session_mode', mode: 'chat' });
  assert.throws(() => validateClientCommand({ type: 'set_session_mode', mode: 'admin' }));
  assert.throws(() => validateClientCommand({ type: 'set_session_mode', mode: 'chat', session_id: 'other' }));
});


type Mode = 'chat' | 'code';
type ModeSnapshot = { mode: Mode; revision: number; pending_mode?: Mode };

/** Exercise the real start/restart and manager logic with a persistent fake engine. */
function installDraftEngines() {
  const prototype = SessionRuntime.prototype as any;
  const previousStart = prototype.startInternal;
  const previousStop = prototype.stopBridge;
  const durable = new Map<string, ModeSnapshot>();
  const engines = new Map<string, { client: EventEmitter; commands: any[]; prompts: ModeSnapshot[] }[]>();
  let holdSnapshots = false;
  const held: (() => void)[] = [];
  prototype.startInternal = async function () {
    this.generation++;
    this.draftModeRestorationRequired = this.draftSessionMode !== undefined;
    const session = this.sessionId as string;
    let snapshot = { ...(durable.get(session) ?? { mode: 'code', revision: 0 }) } as ModeSnapshot;
    const commands: any[] = [];
    const prompts: ModeSnapshot[] = [];
    const client = Object.assign(new EventEmitter(), {
      sendCommand: (command: any) => {
        commands.push(command);
        if (command.type === 'set_session_mode') {
          if (command.mode !== (snapshot.pending_mode ?? snapshot.mode)) {
            snapshot = { mode: snapshot.mode, revision: snapshot.revision + 1,
              ...(command.mode !== snapshot.mode ? { pending_mode: command.mode } : {}) };
            durable.set(session, { ...snapshot });
          }
          queueMicrotask(() => client.emit('event', { type: 'session_mode_changed', session_id: session, ...snapshot }));
        } else if (command.type === 'get_context_manifest') {
          const publish = () => client.emit('event', { type: 'session_mode_changed', session_id: session, ...snapshot });
          if (holdSnapshots) held.push(publish); else queueMicrotask(publish);
        }
      },
      sendPrompt: () => { prompts.push({ mode: snapshot.pending_mode ?? snapshot.mode, revision: snapshot.revision }); },
      close: () => undefined,
    });
    const entries = engines.get(session) ?? [];
    entries.push({ client, commands, prompts });
    engines.set(session, entries);
    this.activeWorkspace = this.projectPath;
    this.activeWorkspaceTrusted = true;
    this.wireClient(client, this.generation);
    this.setState({ status: 'connected' });
  };
  prototype.stopBridge = async function () { this.generation++; this.client = null; };
  return {
    durable,
    engines,
    hold: () => { holdSnapshots = true; },
    release: () => { holdSnapshots = false; for (const publish of held.splice(0)) publish(); },
    restore: () => { prototype.startInternal = previousStart; prototype.stopBridge = previousStop; },
  };
}

test('an evicted draft and the same runtime after restart recover their own durable profile before prompts', async () => {
  const fake = installDraftEngines();
  let preference: Mode = 'chat';
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    getSavedSessionMode: () => preference, onSessionModeSelected: mode => { preference = mode; } });
  try {
    const draft = await manager.newSession('/a');
    const first = manager.require(draft);
    await first.dispatchCommand({ type: 'set_session_mode', mode: 'code' });
    await first.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    assert.deepEqual(fake.durable.get(draft.sessionId), { mode: 'code', pending_mode: 'chat', revision: 3 });
    preference = 'code';
    await manager.newSession('/b');
    assert.equal(manager.get(draft.sessionId), undefined);

    fake.hold();
    const reopening = manager.newSession('/a');
    await tick();
    const reopened = manager.require(draft);
    assert.throws(() => reopened.sendPrompt('too early'), /conversation mode/);
    assert.deepEqual(fake.engines.get(draft.sessionId)!.at(-1)!.prompts, []);
    fake.release();
    assert.deepEqual(await reopening, draft);
    const secondEngine = fake.engines.get(draft.sessionId)!.at(-1)!;
    assert.deepEqual(secondEngine.commands, [{ type: 'get_context_manifest' }], 'restoring a draft must not write a new mode revision');
    assert.equal(reopened.sessionMode, 'code');
    assert.deepEqual(reopened.replaySnapshot().map(({ event }) => event), [
      { type: 'session_mode_changed', session_id: draft.sessionId, mode: 'code', pending_mode: 'chat', revision: 3 },
    ]);
    assert.equal(preference, 'code', 'restoration must not change the last explicit device selection');

    fake.hold();
    const restarting = manager.restart(draft);
    await tick();
    assert.throws(() => reopened.sendPrompt('during restart'), /conversation mode/);
    fake.release();
    await restarting;
    assert.strictEqual(manager.require(draft), reopened);
    const thirdEngine = fake.engines.get(draft.sessionId)!.at(-1)!;
    assert.deepEqual(thirdEngine.commands, [{ type: 'get_context_manifest' }]);
    assert.deepEqual(reopened.replaySnapshot().map(({ event }) => event), [
      { type: 'session_mode_changed', session_id: draft.sessionId, mode: 'code', pending_mode: 'chat', revision: 3 },
    ]);
    await reopened.sendPrompt('hello');
    assert.deepEqual(thirdEngine.prompts, [{ mode: 'chat', revision: 3 }]);
  } finally { await manager.dispose(); fake.restore(); }
});

test('a missing or regressed restored profile cannot admit a draft prompt', async () => {
  const fake = installDraftEngines();
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    getSavedSessionMode: () => 'chat' });
  try {
    const draft = await manager.newSession('/a');
    await manager.newSession('/b');
    fake.durable.delete(draft.sessionId);
    await assert.rejects(manager.newSession('/a'), /restored draft conversation mode/);
    assert.deepEqual(fake.engines.get(draft.sessionId)!.at(-1)!.prompts, []);
  } finally { await manager.dispose(); fake.restore(); }
});

test('a newer durable draft selection survives a lost acknowledgement without changing the device default', async () => {
  const fake = installDraftEngines();
  let preference: Mode = 'chat';
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    getSavedSessionMode: () => preference, onSessionModeSelected: mode => { preference = mode; } });
  try {
    const draft = await manager.newSession('/a');
    await manager.newSession('/b');
    // The engine persisted this selection after the connection was interrupted,
    // so no successful explicit acknowledgement reached the device store.
    fake.durable.set(draft.sessionId, { mode: 'code', revision: 2 });
    await manager.newSession('/a');
    const runtime = manager.require(draft);
    assert.equal(runtime.sessionMode, 'code');
    assert.equal(preference, 'chat');
    assert.deepEqual(fake.engines.get(draft.sessionId)!.at(-1)!.commands, [{ type: 'get_context_manifest' }]);
    await runtime.sendPrompt('hello');
    assert.deepEqual(fake.engines.get(draft.sessionId)!.at(-1)!.prompts, [{ mode: 'code', revision: 2 }]);
  } finally { await manager.dispose(); fake.restore(); }
});

test('same-UUID workspace remount carries an unsent draft and its durable mode revision', async () => {
  const fake = installDraftEngines();
  let preference: Mode = 'chat';
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    getSavedSessionMode: () => preference, onSessionModeSelected: mode => { preference = mode; } });
  try {
    const source = await manager.newSession('/a');
    const original = manager.require(source);
    await original.dispatchCommand({ type: 'set_session_mode', mode: 'code' });
    await original.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    preference = 'code';
    const moved = { ...source, projectPath: '/b' };
    const relocate = manager.prepareDraftRelocation(source, moved.projectPath);
    relocate();
    await manager.closeSession(source, { replacing: true });
    const replacement = await manager.openSession(moved, true);
    assert.notStrictEqual(replacement, original);
    assert.deepEqual(await manager.newSession('/b'), moved);
    assert.deepEqual(replacement.replaySnapshot().map(({ event }) => event), [
      { type: 'session_mode_changed', session_id: source.sessionId, mode: 'code', pending_mode: 'chat', revision: 3 },
    ]);
    assert.deepEqual(fake.engines.get(source.sessionId)!.at(-1)!.commands, [{ type: 'get_context_manifest' }]);
    assert.equal(preference, 'code');
    await replacement.sendPrompt('continue');
    assert.deepEqual(fake.engines.get(source.sessionId)!.at(-1)!.prompts, [{ mode: 'chat', revision: 3 }]);
  } finally { await manager.dispose(); fake.restore(); }
});

test('a workspace remount cannot replace another project’s unsent draft', async () => {
  const fake = installDraftEngines();
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    getSavedSessionMode: () => 'chat' });
  try {
    const source = await manager.newSession('/a');
    const target = await manager.newSession('/b');
    assert.throws(() => manager.prepareDraftRelocation(source, '/b'), /already has an unsent draft/);
    assert.deepEqual(await manager.newSession('/a'), source);
    assert.deepEqual(await manager.newSession('/b'), target);
  } finally { await manager.dispose(); fake.restore(); }
});

test('last explicit device mode follows selection order across runtimes and ignores failed choices', async () => {
  let preference: Mode = 'code';
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onSessionModeSelected: mode => { preference = mode; } });
  const aRef = { projectPath: '/a', sessionId };
  const bRef = { projectPath: '/b', sessionId: '22222222-3333-4444-8555-666666666666' };
  const a = await manager.ensure(aRef, false);
  const b = await manager.ensure(bRef, false);
  function connect(runtime: SessionRuntime) {
    const client = Object.assign(new EventEmitter(), { sendCommand: () => undefined, close: () => undefined });
    Object.assign(runtime, { activeWorkspace: runtime.projectPath, activeWorkspaceTrusted: true });
    (runtime as any).wireClient(client, 0);
    (runtime as any).setState({ status: 'connected' });
    return client;
  }
  const ac = connect(a);
  const bc = connect(b);
  const ack = (client: EventEmitter, ref: typeof aRef, mode: Mode, revision: number) =>
    client.emit('event', { type: 'session_mode_changed', session_id: ref.sessionId, mode, revision });
  try {
    const older = a.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    const newer = b.dispatchCommand({ type: 'set_session_mode', mode: 'code' });
    ack(bc, bRef, 'code', 1);
    await newer;
    ack(ac, aRef, 'chat', 1);
    await older;
    assert.equal(preference, 'code', 'the earlier selection cannot overwrite a later successful one');

    const earlierSuccess = a.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    const laterFailure = b.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
    bc.emit('event', { type: 'error', kind: { type: 'internal' }, message: 'set_session_mode failed: disk error' });
    await assert.rejects(laterFailure, /disk error/);
    ack(ac, aRef, 'chat', 2);
    await earlierSuccess;
    assert.equal(preference, 'chat', 'a failed later intent does not suppress an earlier successful one');
  } finally { await manager.dispose(); }
});

test('IPC records selection order before waiting for another runtime to finish opening', async () => {
  const handles = new Map<string, (...args: any[]) => any>();
  const previousHandle = ipcMain.handle;
  const previousRemove = ipcMain.removeHandler;
  ipcMain.handle = ((channel: string, handler: (...args: any[]) => any) => { handles.set(channel, handler); }) as any;
  ipcMain.removeHandler = ((channel: string) => { handles.delete(channel); }) as any;
  let preference: Mode = 'chat';
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onSessionModeSelected: mode => { preference = mode; } });
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const aRef = { projectPath: '/a', sessionId };
  const bRef = { projectPath: '/b', sessionId: '22222222-3333-4444-8555-666666666666' };
  const a = await manager.ensure(aRef, false);
  const b = await manager.ensure(bRef, false);
  function connect(runtime: SessionRuntime) {
    const client = Object.assign(new EventEmitter(), { sendCommand: () => undefined, close: () => undefined });
    Object.assign(runtime, { activeWorkspace: runtime.projectPath, activeWorkspaceTrusted: true });
    (runtime as any).wireClient(client, 0);
    (runtime as any).setState({ status: 'connected' });
    return client;
  }
  const ac = connect(a), bc = connect(b);
  a.waitForLifecycle = () => gate;
  const sender = Object.assign(new EventEmitter(), { mainFrame: { url: 'app://desktop/index.html' },
    isDestroyed: () => false, send: () => undefined });
  manager.registerWindow(sender as any, sender.mainFrame.url);
  manager.registerIpc();
  const event = { sender, senderFrame: sender.mainFrame };
  try {
    const earlier = handles.get(CH_COMMAND)!(event, aRef.sessionId, { type: 'set_session_mode', mode: 'chat' });
    const later = handles.get(CH_COMMAND)!(event, bRef.sessionId, { type: 'set_session_mode', mode: 'code' });
    await tick();
    bc.emit('event', { type: 'session_mode_changed', session_id: bRef.sessionId, mode: 'code', revision: 1 });
    await later;
    release();
    await tick();
    ac.emit('event', { type: 'session_mode_changed', session_id: aRef.sessionId, mode: 'chat', revision: 1 });
    await earlier;
    assert.equal(preference, 'code');
  } finally {
    release();
    await manager.dispose();
    ipcMain.handle = previousHandle;
    ipcMain.removeHandler = previousRemove;
  }
});

test('an in-flight mode selection pins its session until the acknowledgement triggers cache trimming', async () => {
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  const aRef = { projectPath: '/a', sessionId };
  const bRef = { projectPath: '/b', sessionId: '22222222-3333-4444-8555-666666666666' };
  const a = await manager.ensure(aRef, false);
  const client = Object.assign(new EventEmitter(), { sendCommand: () => undefined, close: () => undefined });
  Object.assign(a, { activeWorkspace: '/a', activeWorkspaceTrusted: true });
  (a as any).wireClient(client, 0);
  (a as any).setState({ status: 'connected' });
  await manager.openSession(aRef, true);
  const selection = a.dispatchCommand({ type: 'set_session_mode', mode: 'chat' });
  void selection.catch(() => undefined);
  const release = manager.retainBackgroundSession(bRef);
  const b = await manager.ensure(bRef, false);
  (b as any).setState({ status: 'connected' });
  try {
    await manager.openSession(bRef, true);
    release();
    assert.strictEqual(manager.get(aRef.sessionId), a);
    assert.equal(a.hasActiveWork, true);
    await assert.rejects(manager.restart(aRef), /active work/);
    await assert.rejects(manager.closeSession(aRef), /active work/);
    client.emit('event', { type: 'session_mode_changed', session_id: aRef.sessionId, mode: 'chat', revision: 1 });
    await selection;
    await tick();
    assert.equal(manager.get(aRef.sessionId), undefined, 'settlement schedules another cache trim');
    assert.strictEqual(manager.get(bRef.sessionId), b);
  } finally { release(); await manager.dispose(); }
});

test('restoring a historical chat updates only that session, not the default for new drafts', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-session-mode-resume-'));
  try {
    const store = new SettingsStore(dir);
    store.setLastSessionMode('chat');
    const client = Object.assign(new EventEmitter(), { sendCommand: () => undefined });
    const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace',
      launchConfig: () => ({ workspace: '/workspace', trusted: true }),
      onSessionModeSelected: mode => store.setLastSessionMode(mode) });
    Object.assign(runtime, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true });
    (runtime as any).wireClient(client, 0);
    client.emit('event', { type: 'session_mode_changed', session_id: sessionId, mode: 'chat', revision: 7 });
    client.emit('event', { type: 'session_resumed', session_id: sessionId, mode: 'code', messages: [] });
    client.emit('event', { type: 'session_mode_changed', session_id: sessionId, mode: 'code', pending_mode: 'chat', revision: 7 });
    assert.equal(runtime.sessionMode, 'code');
    assert.deepEqual(runtime.replaySnapshot().map(({ event }) => event), [
      { type: 'session_resumed', session_id: sessionId, mode: 'code', messages: [] },
      { type: 'session_mode_changed', session_id: sessionId, mode: 'code', pending_mode: 'chat', revision: 7 },
    ]);
    assert.equal(new SettingsStore(dir).getLastSessionMode(), 'chat');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
