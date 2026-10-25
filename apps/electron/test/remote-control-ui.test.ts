import { LANGUAGE_STORAGE_KEY } from '../src/renderer/i18n/locale';

// These behavior assertions pin Chinese copy; locale matching has separate coverage.
Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? 'zh-Hans' : null } });
import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  RemoteControlStore,
  connectionRows,
  connectionTransportLabel,
  isSessionRemote,
  parseRemoteToggleArg,
  remoteAvailability,
  remoteIndicator,
  remoteMenuLabel,
  remoteStateSummary,
  toggleRemoteForSession,
} from '../src/renderer/bridge/remoteControl';
import { ALL_DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';
import { resolveDesktopCommand } from '../src/renderer/bridge/slashDispatch';
import { appendRemoteUserPrompt, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import {
  deviceRows,
  enabledSwitchDisabled,
  enabledSwitchNote,
  lastSeenLabel,
} from '../src/renderer/components/settings/pages/RemoteControl';
import { SETTINGS_NAV, searchNav } from '../src/renderer/components/settings/nav';
import type {
  RemoteControlApi,
  RemoteControlUiStatus,
  RemoteSessionsUi,
} from '../src/shared/remoteControl';

const status = (patch: Partial<RemoteControlUiStatus> = {}): RemoteControlUiStatus => ({
  enabled: false, atStartup: false, managedDisabled: false, signedIn: true, state: 'off', connections: [], ...patch,
});

// ─── settings page rules ─────────────────────────────────────

test('the switch is locked by the administrator and by being signed out, but a signed-out person can still switch it off', () => {
  assert.equal(enabledSwitchDisabled(status()), false);
  assert.equal(enabledSwitchDisabled(status({ managedDisabled: true })), true);
  assert.equal(enabledSwitchDisabled(status({ managedDisabled: true, enabled: true })), true);
  assert.equal(enabledSwitchDisabled(status({ signedIn: false })), true);
  assert.equal(enabledSwitchDisabled(status({ signedIn: false, enabled: true })), false);
});

test('a locked switch always says why', () => {
  assert.match(enabledSwitchNote(status({ managedDisabled: true }))!, /管理员/);
  assert.match(enabledSwitchNote(status({ signedIn: false }))!, /登录/);
  assert.equal(enabledSwitchNote(status()), undefined);
});

test('state summary names the state and carries the error only for an error', () => {
  assert.deepEqual(remoteStateSummary(status({ state: 'online' })), { label: '在线', tone: 'ok' });
  assert.deepEqual(remoteStateSummary(status({ state: 'starting' })), { label: '连接中', tone: 'warn' });
  assert.deepEqual(remoteStateSummary(status({ state: 'error', error: 'offline' })), { label: '错误', tone: 'danger', detail: 'offline' });
  assert.equal(remoteStateSummary(status({ state: 'off', error: 'stale' })).detail, undefined);
});

test('connections are labelled 直连 or 中转, and an unapproved device says it is waiting', () => {
  assert.equal(connectionTransportLabel({ transport: 'webrtc', candidateType: 'host' }), '直连');
  assert.equal(connectionTransportLabel({ transport: 'webrtc', candidateType: 'srflx' }), '直连');
  assert.equal(connectionTransportLabel({ transport: 'webrtc', candidateType: 'relay' }), '中转');
  assert.equal(connectionTransportLabel({ transport: 'tunnel' }), '中转');
  const rows = connectionRows(status({
    connections: [
      { connId: 'a', deviceId: 'd1', deviceName: 'Pixel', platform: 'android', transport: 'webrtc', approved: true, candidateType: 'host' },
      { connId: 'b', deviceId: 'd2', deviceName: '', platform: 'web', transport: 'tunnel', approved: false },
    ],
  }));
  assert.deepEqual(rows.map((r) => r.detail), ['Android · 直连', '网页 · 等待批准']);
  assert.equal(rows[1]!.title, '未命名设备');
});

test('approved devices list the most recently seen first', () => {
  const rows = deviceRows([
    { deviceId: 'a', name: 'Old', platform: 'ios', approvedAt: 1, lastSeen: 10 },
    { deviceId: 'b', name: 'New', platform: 'web', approvedAt: 2, lastSeen: 20 },
  ]);
  assert.deepEqual(rows.map((r) => r.deviceId), ['b', 'a']);
  assert.deepEqual(rows.map((r) => r.platformName), ['网页', 'iOS']);
});

test('last-seen is human readable', () => {
  const now = 1_000_000_000;
  assert.equal(lastSeenLabel(0, now), '从未连接');
  assert.equal(lastSeenLabel(now - 5_000, now), '刚刚');
  assert.equal(lastSeenLabel(now - 5 * 60_000, now), '5分钟前');
  assert.equal(lastSeenLabel(now - 3 * 3_600_000, now), '3小时前');
});

test('the settings nav lists Remote Control and finds it by remote, 远程, rc and 设备', () => {
  const page = SETTINGS_NAV.find((p) => p.id === 'remote-control');
  assert.ok(page);
  assert.equal(page.group, '个人');
  assert.equal(page.needsEngine, false);
  for (const query of ['remote', '远程', 'rc', '设备', 'device']) {
    assert.ok(searchNav(query).some((p) => p.id === 'remote-control'), `searching ${query}`);
  }
});

// ─── indicator and menu ──────────────────────────────────────

const sessions = (...entries: Array<[string, number]>): RemoteSessionsUi => ({
  sessions: entries.map(([sessionId, viewers]) => ({ sessionId, viewers })),
});

test('the indicator shows only for a remote-enabled session, with the viewer count when there is one', () => {
  assert.equal(remoteIndicator(null, 's1'), null);
  assert.equal(remoteIndicator(sessions(['s2', 1]), 's1'), null);
  assert.equal(remoteIndicator(sessions(['s1', 0]), null), null);
  assert.equal(remoteIndicator(sessions(['s1', 0]), 's1')?.text, '📱 远程中');
  assert.equal(remoteIndicator(sessions(['s1', 2]), 's1')?.text, '📱 远程中 · 2');
  assert.equal(remoteIndicator(sessions(['s1', 2]), 's1')?.viewers, 2);
});

test('session membership and the menu label follow the sessions list and availability', () => {
  assert.equal(isSessionRemote(sessions(['s1', 0]), 's1'), true);
  assert.equal(isSessionRemote(sessions(['s1', 0]), 's2'), false);
  assert.equal(isSessionRemote(null, 's1'), false);
  assert.equal(remoteMenuLabel(true, { available: false, reason: 'x' }), '关闭远程控制');
  assert.equal(remoteMenuLabel(false, { available: true }), '开启远程控制');
  assert.match(remoteMenuLabel(false, remoteAvailability(status({ signedIn: false }))), /开启远程控制（.*登录/);
  assert.match(remoteMenuLabel(false, remoteAvailability(status({ managedDisabled: true }))), /管理员/);
});

// ─── the shared store ────────────────────────────────────────

function fakeApi(initial: { status: RemoteControlUiStatus; sessions: RemoteSessionsUi }) {
  let statusListener: ((s: RemoteControlUiStatus) => void) | undefined;
  let sessionsListener: ((s: RemoteSessionsUi) => void) | undefined;
  const calls: string[] = [];
  const api: RemoteControlApi = {
    status: async () => initial.status,
    onStatus: (cb) => { statusListener = cb; return () => { statusListener = undefined; calls.push('offStatus'); }; },
    setEnabled: async () => initial.status,
    atStartup: async () => false,
    setAtStartup: async () => false,
    devices: async () => [],
    revokeDevice: async () => [],
    sessions: async () => initial.sessions,
    setSession: async (id, enabled) => {
      calls.push(`setSession:${id}:${enabled}`);
      initial.sessions = enabled ? sessions(...initial.sessions.sessions.map((e): [string, number] => [e.sessionId, e.viewers]), [id, 0])
        : sessions(...initial.sessions.sessions.filter((e) => e.sessionId !== id).map((e): [string, number] => [e.sessionId, e.viewers]));
      return initial.sessions;
    },
    onSessions: (cb) => { sessionsListener = cb; return () => { sessionsListener = undefined; calls.push('offSessions'); }; },
    onPromptEcho: () => () => undefined,
  };
  return { api, calls, pushStatus: (s: RemoteControlUiStatus) => statusListener?.(s), pushSessions: (s: RemoteSessionsUi) => sessionsListener?.(s) };
}

test('the store reads once, follows both events, and lets go of them with the last subscriber', async () => {
  const fake = fakeApi({ status: status({ enabled: true }), sessions: sessions(['s1', 0]) });
  const store = new RemoteControlStore(() => fake.api);
  let notified = 0;
  const off = store.subscribe(() => { notified += 1; });
  const off2 = store.subscribe(() => undefined);
  await store.refresh();
  assert.equal(store.getSnapshot().status?.enabled, true);
  assert.equal(isSessionRemote(store.getSnapshot().sessions, 's1'), true);

  fake.pushSessions(sessions(['s1', 3]));
  assert.equal(remoteIndicator(store.getSnapshot().sessions, 's1')?.viewers, 3);
  fake.pushStatus(status({ state: 'online' }));
  assert.equal(store.getSnapshot().status?.state, 'online');
  assert.ok(notified >= 2);

  off();
  assert.deepEqual(fake.calls, []);
  off2();
  assert.deepEqual(fake.calls.sort(), ['offSessions', 'offStatus']);
  assert.equal(store.getSnapshot().status, null);
});

test('a store with no Remote Control API stays empty', async () => {
  const store = new RemoteControlStore(() => undefined);
  const off = store.subscribe(() => undefined);
  await store.refresh();
  assert.deepEqual(store.getSnapshot(), { status: null, sessions: null });
  off();
});

// ─── /remote-control ─────────────────────────────────────────

test('/remote-control accepts nothing, on or off', () => {
  assert.equal(parseRemoteToggleArg(''), undefined);
  assert.equal(parseRemoteToggleArg(' ON '), true);
  assert.equal(parseRemoteToggleArg('off'), false);
  assert.equal(parseRemoteToggleArg('maybe'), 'invalid');
});

test('toggling turns an off session on, and an on session off', async () => {
  const fake = fakeApi({ status: status(), sessions: sessions() });
  const on = await toggleRemoteForSession(fake.api, 's1', undefined);
  assert.equal(on.isError, false);
  assert.match(on.message, /已为这个会话开启远程控制/);
  assert.match(on.message, /\/remote/);
  assert.match(on.message, /6 位配对码/);
  const off = await toggleRemoteForSession(fake.api, 's1', undefined);
  assert.equal(off.isError, false);
  assert.match(off.message, /已关闭/);
  assert.deepEqual(fake.calls, ['setSession:s1:true', 'setSession:s1:false']);
});

test('explicit on/off is idempotent and never flips the other way', async () => {
  const fake = fakeApi({ status: status(), sessions: sessions(['s1', 0]) });
  const again = await toggleRemoteForSession(fake.api, 's1', true);
  assert.match(again.message, /已经开启/);
  const already = await toggleRemoteForSession(fake.api, 's2', false);
  assert.match(already.message, /已经关闭/);
  assert.deepEqual(fake.calls, []);
});

test('it explains why it cannot turn a session on, and still turns one off', async () => {
  const signedOut = fakeApi({ status: status({ signedIn: false }), sessions: sessions() });
  const refusal = await toggleRemoteForSession(signedOut.api, 's1', undefined);
  assert.equal(refusal.isError, true);
  assert.match(refusal.message, /登录/);

  const vetoed = fakeApi({ status: status({ managedDisabled: true }), sessions: sessions(['s1', 0]) });
  const blocked = await toggleRemoteForSession(vetoed.api, 's2', undefined);
  assert.equal(blocked.isError, true);
  assert.match(blocked.message, /管理员/);
  const off = await toggleRemoteForSession(vetoed.api, 's1', undefined);
  assert.equal(off.isError, false);
  assert.deepEqual(vetoed.calls, ['setSession:s1:false']);
  assert.deepEqual(signedOut.calls, []);
});

test('it reports a missing API, a missing session and a failing host', async () => {
  assert.equal((await toggleRemoteForSession(undefined, 's1', undefined)).isError, true);
  const fake = fakeApi({ status: status(), sessions: sessions() });
  assert.match((await toggleRemoteForSession(fake.api, null, undefined)).message, /打开一个会话/);
  fake.api.setSession = async () => { throw new Error('unknown session'); };
  const failed = await toggleRemoteForSession(fake.api, 's1', true);
  assert.equal(failed.isError, true);
  assert.match(failed.message, /unknown session/);
});

function commandContext(outcome: { message: string; isError: boolean }) {
  const calls: string[] = [];
  const ctx = {
    toggleRemoteControl: async (wanted: boolean | undefined) => { calls.push(`toggle:${wanted}`); return outcome; },
    emit: (output: string, isError?: boolean) => { calls.push(`emit:${isError ? 'error' : 'ok'}:${output}`); },
  };
  return { ctx, calls };
}

for (const [line, wanted] of [['/remote-control', 'undefined'], ['/rc', 'undefined'], ['/rc on', 'true'], ['/remote-control off', 'false']] as const) {
  test(`${line} is a Desktop-local command that toggles and shows its result`, async () => {
    const resolved = resolveDesktopCommand(line, ALL_DESKTOP_COMMANDS);
    assert.ok(resolved, `${line} should be handled locally, never sent to the engine`);
    const { ctx, calls } = commandContext({ message: 'done', isError: false });
    await resolved.command.run(resolved.args, ctx as never);
    assert.deepEqual(calls, [`toggle:${wanted}`, 'emit:ok:done']);
  });
}

test('a bad argument is reported without touching Remote Control, and a refusal is shown as an error', async () => {
  const bad = resolveDesktopCommand('/rc sideways', ALL_DESKTOP_COMMANDS)!;
  const first = commandContext({ message: 'unused', isError: false });
  await bad.command.run(bad.args, first.ctx as never);
  assert.deepEqual(first.calls, ['emit:error:/remote-control takes on or off, not: sideways']);

  const refused = resolveDesktopCommand('/rc', ALL_DESKTOP_COMMANDS)!;
  const second = commandContext({ message: '请先登录', isError: true });
  await refused.command.run(refused.args, second.ctx as never);
  assert.deepEqual(second.calls, ['toggle:undefined', 'emit:error:请先登录']);
});

test('/remote stays the engine alias of /session: Desktop does not intercept it', () => {
  assert.equal(resolveDesktopCommand('/remote', ALL_DESKTOP_COMMANDS), null);
  assert.equal(resolveDesktopCommand('/remote-control', ALL_DESKTOP_COMMANDS)?.command.name, 'remote-control');
  assert.equal(resolveDesktopCommand('/rc', ALL_DESKTOP_COMMANDS)?.command.name, 'remote-control');
});

// ─── prompt echo ─────────────────────────────────────────────

const echo = (turnId: number, text = 'run the tests', deviceName = 'Pixel') => ({ turnId, text, images: [], deviceName });

test('a device prompt becomes a labelled user row and does not claim the turn', () => {
  const next = appendRemoteUserPrompt(emptyConversation(), echo(7), 1234);
  assert.equal(next.items.length, 1);
  const row = next.items[0]!;
  assert.equal(row.type, 'narration');
  if (row.type !== 'narration') return;
  assert.equal(row.role, 'user');
  assert.equal(row.text, 'run the tests');
  assert.equal(row.sentAt, 1234);
  assert.equal(row.delivery, undefined);
  assert.deepEqual(row.origin, { kind: 'device', deviceName: 'Pixel', turnId: 7 });
  assert.equal(next.running, false);
});

test('the same turn echoed twice adds one row; a different turn adds another', () => {
  const once = appendRemoteUserPrompt(emptyConversation(), echo(7));
  assert.equal(appendRemoteUserPrompt(once, echo(7)), once);
  const twice = appendRemoteUserPrompt(once, echo(8, 'and again'));
  assert.equal(twice.items.length, 2);
});

test('a blank prompt adds nothing, and the reply that follows lands after the row', () => {
  const blank = emptyConversation();
  assert.equal(appendRemoteUserPrompt(blank, echo(1, '   ')), blank);
  let state = appendRemoteUserPrompt(emptyConversation(), echo(1));
  state = reduceEvent(state, { type: 'turn_started', turn_id: 1 } as never);
  assert.equal(state.running, true);
  assert.equal(state.items[0]!.type, 'narration');
});
