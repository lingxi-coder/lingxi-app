import assert from 'node:assert/strict';
import { test } from 'node:test';

import { HostNotifier } from '../src/main/notifications.js';
import { PushRequester, RemotePush, type PushPresence, type RemotePushKind } from '../src/main/remoteControl/pushRequests.js';
import { PERMISSION_PROMPT_NOTIFY_DELAY_MS, defaultNotificationPreferences, type NotificationPreferences } from '../src/shared/notificationPreferences.js';
import { newClock } from './remote-session-helpers.js';

const SESSION = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
const REF = { projectPath: '/tmp/project', sessionId: SESSION };

// ─────────────────────────────────────────────────────────────────────────────
// The notifier's remote sink
// ─────────────────────────────────────────────────────────────────────────────

function notifierHarness(initial: Partial<NotificationPreferences> = {}) {
  const shown: string[] = [];
  const remote: Array<[RemotePushKind, string]> = [];
  let clock = 1_000_000;
  let focused = false;
  let idle = 0;
  let nextId = 1;
  const timers = new Map<number, { at: number; fn: () => void }>();
  const notifier = new HostNotifier({
    show: (title) => { shown.push(title); },
    isWindowFocused: () => focused,
    remote: (kind, sessionId) => { remote.push([kind, sessionId]); },
    systemIdleSeconds: () => idle,
    now: () => clock,
    schedule: ((fn: () => void, ms: number) => { const id = nextId++; timers.set(id, { at: clock + ms, fn }); return id; }) as never,
    unschedule: ((handle: unknown) => { timers.delete(handle as number); }) as never,
  });
  notifier.setPreferences({ ...defaultNotificationPreferences(), ...initial });
  return {
    notifier, shown, remote,
    focus: (value: boolean) => { focused = value; },
    idle: (seconds: number) => { idle = seconds; },
    advance(ms: number) {
      const target = clock + ms;
      for (;;) {
        let due: [number, { at: number; fn: () => void }] | undefined;
        for (const entry of timers) if (entry[1].at <= target && (!due || entry[1].at < due[1].at)) due = entry;
        if (!due) break;
        timers.delete(due[0]);
        clock = due[1].at;
        due[1].fn();
      }
      clock = target;
    },
  };
}

test('each notification kind that needs the user is passed to Remote Control with its remote name', () => {
  const h = notifierHarness();
  h.notifier.permissionRequested(SESSION, 1, 'Bash', REF);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS + 1);
  h.notifier.askUserQuestion(SESSION, 2, REF);
  h.notifier.taskFinished(SESSION, 't', 'x', false, REF);
  assert.deepEqual(h.remote, [['permission', SESSION], ['question', SESSION], ['task', SESSION]]);
});

test('with the window focused and the system not idle, Remote Control hears nothing', () => {
  const h = notifierHarness();
  h.focus(true);
  h.idle(59);
  h.notifier.askUserQuestion(SESSION, 2, REF);
  assert.deepEqual(h.remote, []);
  assert.deepEqual(h.shown, []);
});

test('a focused window with the system idle for 60 s or more still reaches Remote Control, though not the desktop banner', () => {
  const h = notifierHarness();
  h.focus(true);
  h.idle(60);
  h.notifier.askUserQuestion(SESSION, 2, REF);
  assert.deepEqual(h.remote, [['question', SESSION]]);
  assert.deepEqual(h.shown, [], 'the desktop banner is still for an unfocused window');
});

test('an unfocused window reaches both, whatever the idle time', () => {
  const h = notifierHarness();
  h.notifier.askUserQuestion(SESSION, 2, REF);
  assert.equal(h.remote.length, 1);
  assert.equal(h.shown.length, 1);
});

test('a kind the user turned off does not reach Remote Control', () => {
  const h = notifierHarness({ inputNeededNotifEnabled: false, taskCompleteNotifEnabled: false });
  h.notifier.askUserQuestion(SESSION, 2, REF);
  h.notifier.taskFinished(SESSION, 't', 'x', true, REF);
  assert.deepEqual(h.remote, []);
  const all = notifierHarness({ enabled: false });
  all.notifier.askUserQuestion(SESSION, 2, REF);
  assert.deepEqual(all.remote, []);
});

test('the idle prompt is a remote push too, and the same prompt is not pushed twice', () => {
  const h = notifierHarness();
  h.notifier.turnEnded(SESSION, REF);
  h.advance(10 * 60_000);
  assert.deepEqual(h.remote, [['idle', SESSION]]);
  h.notifier.askUserQuestion(SESSION, 9, REF);
  h.notifier.askUserQuestion(SESSION, 9, REF);
  assert.equal(h.remote.length, 2);
});

test('scheduled runs are not pushed to other devices', () => {
  const h = notifierHarness();
  h.notifier.scheduledRun('t', 'b', REF);
  assert.deepEqual(h.remote, []);
});

test('a remote sink that throws changes nothing for the desktop', () => {
  const shown: string[] = [];
  const notifier = new HostNotifier({
    show: (title) => { shown.push(title); },
    isWindowFocused: () => false,
    remote: () => { throw new Error('boom'); },
  });
  notifier.setPreferences(defaultNotificationPreferences());
  notifier.askUserQuestion(SESSION, 1, REF);
  assert.equal(shown.length, 1);
});

// ─────────────────────────────────────────────────────────────────────────────
// RemotePush: what only Remote Control knows
// ─────────────────────────────────────────────────────────────────────────────

function pushHarness() {
  const clock = newClock();
  const requests: Array<{ kind: RemotePushKind; sessionId: string; exclude: string[] }> = [];
  const state = { enabled: new Set([SESSION]), foreground: 0, active: true };
  const presence: PushPresence = {
    isEnabled: (id) => state.enabled.has(id),
    foregroundViewers: () => state.foreground,
  };
  const push = new RemotePush({
    requester: { request: async (kind, sessionId, exclude) => { requests.push({ kind, sessionId, exclude: [...exclude] }); return true; } },
    presence,
    active: () => state.active,
    clock,
  });
  return { push, requests, state, clock };
}

test('a push is requested for all installations when nobody is looking at an enabled session', () => {
  const h = pushHarness();
  h.push.notify('permission', SESSION);
  assert.deepEqual(h.requests, [{ kind: 'permission', sessionId: SESSION, exclude: [] }]);
});

test('no push when Remote Control is not running, the session is off, or a viewer has the session in the foreground', () => {
  const h = pushHarness();
  h.state.active = false;
  h.push.notify('permission', SESSION);
  h.state.active = true;
  h.state.enabled.clear();
  h.push.notify('permission', SESSION);
  h.state.enabled.add(SESSION);
  h.state.foreground = 1;
  h.push.notify('permission', SESSION);
  assert.deepEqual(h.requests, []);
  h.state.foreground = 0;
  h.push.notify('permission', SESSION);
  assert.equal(h.requests.length, 1);
});

test('the same kind for the same session is not repeated within 30 seconds, but another kind is', () => {
  const h = pushHarness();
  h.push.notify('question', SESSION);
  h.push.notify('question', SESSION);
  h.push.notify('task', SESSION);
  assert.deepEqual(h.requests.map((request) => request.kind), ['question', 'task']);
  h.clock.advance(30_000);
  h.push.notify('question', SESSION);
  assert.equal(h.requests.length, 3);
});

// ─────────────────────────────────────────────────────────────────────────────
// PushRequester
// ─────────────────────────────────────────────────────────────────────────────

function requesterHarness(statuses: number[], hostId: string | null = 'host-1') {
  const calls: Array<{ url: string; init: any }> = [];
  const tokens: Array<boolean | undefined> = [];
  const warnings: string[] = [];
  const requester = new PushRequester({
    baseUrl: 'https://push.example.com/api',
    hostId: () => hostId ?? undefined,
    getIdToken: async (forceRefresh) => { tokens.push(forceRefresh); return forceRefresh ? 'fresh-token' : 'old-token'; },
    fetch: async (url, init) => {
      calls.push({ url, init });
      const status = statuses.shift() ?? 200;
      return { ok: status >= 200 && status < 300, status };
    },
    diagnostics: (_level, message) => warnings.push(message),
  });
  return { requester, calls, tokens, warnings };
}

test('the request carries the kind, host and session, a bearer token, and no conversation text', async () => {
  const h = requesterHarness([200]);
  assert.equal(await h.requester.request('permission', SESSION, ['inst-a']), true);
  const [call] = h.calls;
  assert.equal(call!.url, 'https://push.example.com/api/v1/push/self/messages');
  assert.equal(call!.init.method, 'POST');
  assert.equal(call!.init.headers.Authorization, 'Bearer old-token');
  assert.deepEqual(JSON.parse(call!.init.body), { kind: 'permission', host_id: 'host-1', session_id: SESSION, exclude_installation_ids: ['inst-a'] });
});

test('an empty exclusion list is left out, and a long one is capped', async () => {
  const h = requesterHarness([200, 200]);
  await h.requester.request('idle', SESSION, []);
  assert.ok(!('exclude_installation_ids' in JSON.parse(h.calls[0]!.init.body)));
  await h.requester.request('idle', SESSION, Array.from({ length: 40 }, (_, i) => `i${i}`));
  assert.equal(JSON.parse(h.calls[1]!.init.body).exclude_installation_ids.length, 16);
});

test('a 401 is retried once with a fresh token', async () => {
  const h = requesterHarness([401, 200]);
  assert.equal(await h.requester.request('task', SESSION, []), true);
  assert.deepEqual(h.tokens, [false, true]);
  assert.equal(h.calls[1]!.init.headers.Authorization, 'Bearer fresh-token');
});

test('a refusal or a failure is logged, never thrown, and says false', async () => {
  const refused = requesterHarness([500]);
  assert.equal(await refused.requester.request('task', SESSION, []), false);
  assert.match(refused.warnings[0]!, /500/);
  const twice = requesterHarness([401, 401]);
  assert.equal(await twice.requester.request('task', SESSION, []), false);
  assert.equal(twice.calls.length, 2);
  const throwing = new PushRequester({
    baseUrl: 'https://x', hostId: () => 'h', getIdToken: async () => { throw new Error('signed out'); },
    fetch: async () => ({ ok: true, status: 200 }), diagnostics: () => undefined,
  });
  assert.equal(await throwing.request('task', SESSION, []), false);
});

test('without a running host there is no host id and no request', async () => {
  const h = requesterHarness([200], null);
  assert.equal(await h.requester.request('task', SESSION, []), false);
  assert.equal(h.calls.length, 0);
});
