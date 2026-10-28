import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
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
      onSessionModeSelected: mode => store.setLastSessionMode(mode) }, 'code');
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


test('a fresh draft waits for the saved mode and reopening it never reapplies the device default', async () => {
  const start = SessionRuntime.prototype.start;
  const initialize = SessionRuntime.prototype.initializeSessionMode;
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const modes: string[] = [];
  let preference: 'chat' | 'code' = 'chat';
  SessionRuntime.prototype.start = async function () {};
  SessionRuntime.prototype.initializeSessionMode = async function (mode) { modes.push(mode); await gate; };
  const manager = new SessionRuntimeManager({ launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    getSavedSessionMode: () => preference });
  try {
    let settled = false;
    const opening = manager.newSession('/workspace').then(value => { settled = true; return value; });
    await tick();
    assert.deepEqual(modes, ['chat']);
    assert.equal(settled, false, 'a prompt cannot target the new draft before mode confirmation');
    release();
    const first = await opening;
    preference = 'code';
    assert.deepEqual(await manager.newSession('/workspace'), first);
    assert.deepEqual(modes, ['chat'], 'the existing draft keeps its own session state');
    await manager.newSession('/another');
    assert.deepEqual(modes, ['chat'], 'Code is the engine default for a fresh draft');
  } finally {
    await manager.dispose();
    SessionRuntime.prototype.start = start;
    SessionRuntime.prototype.initializeSessionMode = initialize;
  }
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
