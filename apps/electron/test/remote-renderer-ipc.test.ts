import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { RemotePolicy } from '../src/main/remoteControl/policy.js';
import { RemoteControlUi, type RemoteControlUiDeps } from '../src/main/remoteControl/rendererIpc.js';
import { SettingsStore } from '../src/main/settings.js';
import {
  CH_REMOTE_AT_STARTUP_GET,
  CH_REMOTE_AT_STARTUP_SET,
  CH_REMOTE_DEVICE_REVOKE,
  CH_REMOTE_DEVICES_LIST,
  CH_REMOTE_ENABLED_SET,
  CH_REMOTE_PROMPT_ECHO,
  CH_REMOTE_SESSION_SET,
  CH_REMOTE_SESSIONS_EVENT,
  CH_REMOTE_SESSIONS_GET,
  CH_REMOTE_STATUS_EVENT,
  CH_REMOTE_STATUS_GET,
} from '../src/shared/remoteControl.js';
import { FakeSettings, PROJECT, SESSION_A, SESSION_B } from './remote-session-helpers.js';

function fixture(options: { managedDisabled?: boolean; signedIn?: boolean } = {}) {
  const settings = new FakeSettings();
  let enabled = false;
  const policy = new RemotePolicy({ settings });
  if (options.managedDisabled) policy.allows = () => false;
  const statusListeners = new Set<(status: any) => void>();
  const viewerListeners = new Set<() => void>();
  const devices = [{ device_id: 'dev-1', key: 'SECRET-KEY-MATERIAL', name: 'Pixel', platform: 'android' as const, approved_at: 10, last_seen: 20 }];
  const revoked: string[] = [];
  const toggles: boolean[] = [];
  let status: any = { state: 'off', connections: [] };
  const sent: Array<{ channel: string; payload: any }> = [];
  const attached = new Map<string, number>();
  const deps: RemoteControlUiDeps = {
    service: {
      status: () => status,
      onStatus: (callback) => { statusListeners.add(callback); return () => { statusListeners.delete(callback); }; },
      setEnabled: async (value) => { enabled = value; toggles.push(value); status = { ...status, state: value ? 'online' : 'off' }; for (const l of statusListeners) l(status); },
      listDevices: () => devices,
      revokeDevice: async (id) => { revoked.push(id); devices.splice(0, devices.length, ...devices.filter((d) => d.device_id !== id)); },
    },
    settings: { getRemoteControlEnabled: () => enabled },
    policy,
    gateway: {
      attachedViewers: (id) => (id === undefined ? 0 : attached.get(id) ?? 0),
      onViewersChanged: (listener) => { viewerListeners.add(listener); return () => { viewerListeners.delete(listener); }; },
    },
    runtimeRef: (sessionId) => (sessionId === SESSION_A ? { projectPath: PROJECT, sessionId } : undefined),
    signedIn: () => options.signedIn ?? true,
    send: (channel, payload) => sent.push({ channel, payload }),
  };
  const ui = new RemoteControlUi(deps);
  const handlers = ui.handlers();
  return {
    ui, handlers, sent, settings, policy, devices, revoked, toggles, attached,
    setStatus(next: any) { status = next; for (const l of statusListeners) l(status); },
    viewersChanged() { for (const l of viewerListeners) l(); },
    call: async (channel: string, ...args: unknown[]) => handlers[channel]!(...args),
  };
}

test('the status says what the window needs and never anything secret', async () => {
  const f = fixture();
  f.setStatus({
    state: 'online', host_id: 'host-secret-ish',
    connections: [{ conn_id: 'c1', device_id: 'dev-1', device_name: 'Pixel', platform: 'android', transport: 'webrtc', approved: true, candidate_type: 'relay' }],
  });
  const status = await f.call(CH_REMOTE_STATUS_GET);
  assert.deepEqual(status, {
    enabled: false, atStartup: false, managedDisabled: false, signedIn: true, state: 'online',
    connections: [{ connId: 'c1', deviceId: 'dev-1', deviceName: 'Pixel', platform: 'android', transport: 'webrtc', approved: true, candidateType: 'relay' }],
  });
  assert.ok(!JSON.stringify(status).includes('host-secret-ish'));
});

test('an error state carries its message, and a connection without a candidate type leaves it out', async () => {
  const f = fixture();
  f.setStatus({ state: 'error', error: 'offline', connections: [{ conn_id: 'c', device_id: 'd', device_name: 'n', platform: 'web', transport: 'tunnel', approved: false }] });
  const status = await f.call(CH_REMOTE_STATUS_GET);
  assert.equal(status.error, 'offline');
  assert.ok(!('candidateType' in status.connections[0]));
});

test('status changes are pushed to the windows, once each', () => {
  const f = fixture();
  f.setStatus({ state: 'starting', connections: [] });
  f.setStatus({ state: 'starting', connections: [] });
  f.setStatus({ state: 'online', connections: [] });
  const events = f.sent.filter((item) => item.channel === CH_REMOTE_STATUS_EVENT);
  assert.deepEqual(events.map((item) => item.payload.state), ['starting', 'online']);
});

test('the host-level switch is validated, applied through the service, and refused under a managed veto', async () => {
  const f = fixture();
  assert.equal((await f.call(CH_REMOTE_ENABLED_SET, true)).enabled, true);
  assert.deepEqual(f.toggles, [true]);
  for (const bad of ['true', 1, null, undefined, {}]) await assert.rejects(f.call(CH_REMOTE_ENABLED_SET, bad), /invalid/, String(bad));
  assert.deepEqual(f.toggles, [true]);
  const managed = fixture({ managedDisabled: true });
  await assert.rejects(managed.call(CH_REMOTE_ENABLED_SET, true), /administrator/);
  assert.equal((await managed.call(CH_REMOTE_STATUS_GET)).managedDisabled, true);
  assert.equal((await managed.call(CH_REMOTE_ENABLED_SET, false)).enabled, false, 'turning it off is always allowed');
});

test('remoteControlAtStartup is read and written, and only booleans are taken', async () => {
  const f = fixture();
  assert.equal(await f.call(CH_REMOTE_AT_STARTUP_GET), false);
  assert.equal(await f.call(CH_REMOTE_AT_STARTUP_SET, true), true);
  assert.equal(f.settings.atStartup, true);
  assert.equal((await f.call(CH_REMOTE_STATUS_GET)).atStartup, true);
  await assert.rejects(f.call(CH_REMOTE_AT_STARTUP_SET, 'yes'), /invalid/);
});

test('devices are listed without their keys, and revoking one returns the new list', async () => {
  const f = fixture();
  const list = await f.call(CH_REMOTE_DEVICES_LIST);
  assert.deepEqual(list, [{ deviceId: 'dev-1', name: 'Pixel', platform: 'android', approvedAt: 10, lastSeen: 20 }]);
  assert.ok(!JSON.stringify(list).includes('SECRET'));
  await assert.rejects(f.call(CH_REMOTE_DEVICE_REVOKE, ''), /invalid/);
  await assert.rejects(f.call(CH_REMOTE_DEVICE_REVOKE, 5), /invalid/);
  await assert.rejects(f.call(CH_REMOTE_DEVICE_REVOKE, 'x'.repeat(500)), /invalid/);
  assert.deepEqual(await f.call(CH_REMOTE_DEVICE_REVOKE, 'dev-1'), []);
  assert.deepEqual(f.revoked, ['dev-1']);
});

test('switching a session on turns Remote Control on the first time, and lists the session with its viewers', async () => {
  const f = fixture();
  f.attached.set(SESSION_A, 2);
  const result = await f.call(CH_REMOTE_SESSION_SET, SESSION_A, true);
  assert.deepEqual(result, { sessions: [{ sessionId: SESSION_A, viewers: 2 }] });
  assert.deepEqual(f.toggles, [true]);
  await f.call(CH_REMOTE_SESSION_SET, SESSION_A, true);
  assert.deepEqual(f.toggles, [true], 'already on: not toggled again');
  assert.deepEqual(await f.call(CH_REMOTE_SESSIONS_GET), result);
  assert.deepEqual(f.settings.sessions, [{ projectPath: PROJECT, sessionId: SESSION_A }]);
});

test('switching a session off, an unknown session, a bad id, a managed veto', async () => {
  const f = fixture();
  await f.call(CH_REMOTE_SESSION_SET, SESSION_A, true);
  assert.deepEqual(await f.call(CH_REMOTE_SESSION_SET, SESSION_A, false), { sessions: [] });
  assert.deepEqual(await f.call(CH_REMOTE_SESSION_SET, SESSION_B, false), { sessions: [] }, 'off for a session that was never on is a no-op');
  await assert.rejects(f.call(CH_REMOTE_SESSION_SET, SESSION_B, true), /unknown session/);
  await assert.rejects(f.call(CH_REMOTE_SESSION_SET, '', true), /invalid/);
  await assert.rejects(f.call(CH_REMOTE_SESSION_SET, SESSION_A, 'yes'), /invalid/);
  const managed = fixture({ managedDisabled: true });
  await assert.rejects(managed.call(CH_REMOTE_SESSION_SET, SESSION_A, true), /administrator/);
  assert.deepEqual(managed.settings.sessions, []);
});

test('a session that is on but not open can be switched off using the policy\'s own record', async () => {
  const f = fixture();
  f.policy.setSessionEnabled({ projectPath: '/elsewhere', sessionId: SESSION_B }, true);
  assert.deepEqual(await f.call(CH_REMOTE_SESSION_SET, SESSION_B, false), { sessions: [] });
});

test('session changes and viewer changes reach the windows when the list differs', async () => {
  const f = fixture();
  await f.call(CH_REMOTE_SESSION_SET, SESSION_A, true);
  const events = () => f.sent.filter((item) => item.channel === CH_REMOTE_SESSIONS_EVENT).map((item) => item.payload);
  assert.deepEqual(events().at(-1), { sessions: [{ sessionId: SESSION_A, viewers: 0 }] });
  const count = events().length;
  f.viewersChanged();
  assert.equal(events().length, count, 'nothing changed');
  f.attached.set(SESSION_A, 1);
  f.viewersChanged();
  assert.deepEqual(events().at(-1), { sessions: [{ sessionId: SESSION_A, viewers: 1 }] });
  f.ui.sessionsChanged();
});

test('a prompt echo goes to the windows as a device row', () => {
  const f = fixture();
  f.ui.echo({ sessionId: SESSION_A, turnId: 5, text: 'hi', images: [], origin: 'device', deviceName: 'Pixel' });
  assert.deepEqual(f.sent.at(-1), { channel: CH_REMOTE_PROMPT_ECHO, payload: { sessionId: SESSION_A, turnId: 5, text: 'hi', images: [], origin: 'device', deviceName: 'Pixel' } });
});

test('after dispose the ui no longer listens', () => {
  const f = fixture();
  f.ui.dispose();
  f.setStatus({ state: 'online', connections: [] });
  assert.equal(f.sent.length, 0);
});

// ─────────────────────────────────────────────────────────────────────────────
// Through the host controller: only the app's own window may call
// ─────────────────────────────────────────────────────────────────────────────

test('the host controller registers the handlers behind its sender check and removes them on dispose', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-remote-ipc-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  const f = fixture();
  host.attachRemoteControl(f.ui);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sent: unknown[] = [];
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send: (...args: unknown[]) => sent.push(args) };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  try {
    for (const channel of [CH_REMOTE_STATUS_GET, CH_REMOTE_ENABLED_SET, CH_REMOTE_AT_STARTUP_GET, CH_REMOTE_AT_STARTUP_SET, CH_REMOTE_DEVICES_LIST, CH_REMOTE_DEVICE_REVOKE, CH_REMOTE_SESSIONS_GET, CH_REMOTE_SESSION_SET]) {
      assert.ok(handlers.has(channel), channel);
    }
    const invoke = handlers.get(CH_REMOTE_DEVICES_LIST)!;
    assert.throws(() => invoke({ sender, senderFrame: { url: frame.url } }), /unauthorized/);
    assert.throws(() => invoke({ sender: { mainFrame: frame }, senderFrame: frame }), /unauthorized/);
    assert.equal((await invoke({ sender, senderFrame: frame })).length, 1);
    assert.equal((await handlers.get(CH_REMOTE_ENABLED_SET)!({ sender, senderFrame: frame }, true)).enabled, true);
    host.sendToWindows('lingxi:test', { a: 1 });
    assert.deepEqual(sent.at(-1), ['lingxi:test', { a: 1 }]);
    frame.url = 'https://attacker.example';
    assert.throws(() => invoke({ sender, senderFrame: frame }), /unauthorized/);
  } finally {
    host.dispose();
    rmSync(root, { recursive: true, force: true });
  }
  assert.ok(![...handlers.keys()].some((name) => name.startsWith('lingxi:remote:')));
});
