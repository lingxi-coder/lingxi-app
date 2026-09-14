import assert from 'node:assert/strict';
import { test } from 'node:test';

import { HostNotifier, NOTIFICATION_COPY } from '../src/main/notifications';
import {
  DEFAULT_IDLE_NOTIF_THRESHOLD_MS,
  PERMISSION_PROMPT_NOTIFY_DELAY_MS,
  defaultNotificationPreferences,
  isKindEnabled,
  parseNotificationPreferences,
  type NotificationPreferences,
} from '../src/shared/notificationPreferences';
import type { SessionRef } from '../src/shared/settings';

const SESSION = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
const REF: SessionRef = { projectPath: '/tmp/project', sessionId: SESSION };

interface Harness {
  notifier: HostNotifier;
  shown: { title: string; body: string; ref?: SessionRef }[];
  /** Advance the fake clock and run every timer that comes due. */
  advance(ms: number): void;
  focus(focused: boolean): void;
  prefs(patch: Partial<NotificationPreferences>): void;
}

function harness(initial: Partial<NotificationPreferences> = {}): Harness {
  const shown: { title: string; body: string; ref?: SessionRef }[] = [];
  let clock = 1_000_000;
  let focused = false;
  let nextId = 1;
  const timers = new Map<number, { at: number; fn: () => void }>();

  const notifier = new HostNotifier({
    show: (title, body, ref) => { shown.push({ title, body, ref }); },
    isWindowFocused: () => focused,
    now: () => clock,
    schedule: ((fn: () => void, ms: number) => {
      const id = nextId++;
      timers.set(id, { at: clock + ms, fn });
      return id as unknown as ReturnType<typeof setTimeout>;
    }) as never,
    unschedule: ((handle: unknown) => { timers.delete(handle as number); }) as never,
  });
  notifier.setPreferences({ ...defaultNotificationPreferences(), ...initial });

  return {
    notifier,
    shown,
    advance(ms) {
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
    focus(value) { focused = value; },
    prefs(patch) { notifier.setPreferences({ ...defaultNotificationPreferences(), ...initial, ...patch }); },
  };
}

// ── the gate can actually fire ───────────────────────────────────────────────
// Without this, every "nothing was shown" assertion below could pass because
// the harness never delivers anything at all.

test('idle_prompt fires once the threshold elapses with no interaction', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS - 1);
  assert.deepEqual(h.shown, [], 'nothing may fire before the threshold');
  h.advance(2);
  assert.equal(h.shown.length, 1);
  assert.equal(h.shown[0]?.title, NOTIFICATION_COPY.idleTitle);
  assert.deepEqual(h.shown[0]?.ref, REF, 'the notification must carry the ref its click restores');
});

// ── upstream `class See`'s fire-time re-check, clause by clause ──────────────

test('a turn ending does not itself notify — there is no "turn finished" notification', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.advance(1_000);
  assert.deepEqual(h.shown, []);
});

test('coming back before the threshold suppresses idle_prompt', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS / 2);
  h.notifier.noteInteraction();
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS);
  assert.deepEqual(h.shown, []);
});

test('a new turn during the countdown suppresses idle_prompt', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS / 2);
  h.notifier.turnStarted(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS * 2);
  assert.deepEqual(h.shown, []);
});

test('a dialog on screen at fire time suppresses idle_prompt', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.notifier.setDialogsOnScreen(SESSION, 1, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS * 2);
  assert.deepEqual(h.shown, []);
});

test('a focused window suppresses idle_prompt', () => {
  const h = harness();
  h.focus(true);
  h.notifier.turnEnded(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS * 2);
  assert.deepEqual(h.shown, []);
});

test('the configured threshold, not the default, is what is waited out', () => {
  const h = harness({ messageIdleNotifThresholdMs: 15_000 });
  h.notifier.turnEnded(SESSION, REF);
  h.advance(14_000);
  assert.deepEqual(h.shown, []);
  h.advance(2_000);
  assert.equal(h.shown.length, 1);
});

// ── permission_prompt: upstream's 6s arm/clear ───────────────────────────────

test('a permission answered inside 6s notifies nothing', () => {
  const h = harness();
  h.notifier.permissionRequested(SESSION, 7, 'Bash', REF);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS - 1_000);
  h.notifier.permissionSettled(SESSION, 7);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS * 4);
  assert.deepEqual(h.shown, []);
});

test('a permission left unanswered past 6s notifies, naming the tool', () => {
  const h = harness();
  h.notifier.permissionRequested(SESSION, 7, 'Bash', REF);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS + 1);
  assert.equal(h.shown.length, 1);
  assert.equal(h.shown[0]?.title, NOTIFICATION_COPY.permissionTitle);
  assert.match(h.shown[0]?.body ?? '', /Bash/);
});

test('two pending permissions each get their own timer', () => {
  const h = harness();
  h.notifier.permissionRequested(SESSION, 1, 'Bash', REF);
  h.notifier.permissionRequested(SESSION, 2, 'Write', REF);
  h.notifier.permissionSettled(SESSION, 1);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS + 1);
  assert.equal(h.shown.length, 1);
  assert.match(h.shown[0]?.body ?? '', /Write/);
});

// ── immediate kinds ──────────────────────────────────────────────────────────

test('AskUserQuestion notifies immediately and only once per request', () => {
  const h = harness();
  h.notifier.askUserQuestion(SESSION, 3, REF);
  h.notifier.askUserQuestion(SESSION, 3, REF);
  assert.equal(h.shown.length, 1);
});

test('a finished task is named by its label, and failure reads differently', () => {
  const h = harness();
  h.notifier.taskFinished(SESSION, 'task-1', 'Reindex the docs', false, REF);
  h.notifier.taskFinished(SESSION, 'task-2', undefined, true, REF);
  assert.equal(h.shown.length, 2);
  assert.match(h.shown[0]?.body ?? '', /Reindex the docs/);
  assert.match(h.shown[1]?.body ?? '', /task-2/, 'the bare id is the fallback, not a blank');
  assert.equal(h.shown[1]?.title, NOTIFICATION_COPY.failedTitle);
});

// ── preference gates ─────────────────────────────────────────────────────────

test('the master switch silences every kind and disarms timers already counting', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.notifier.permissionRequested(SESSION, 1, 'Bash', REF);
  h.prefs({ enabled: false });
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS * 4);
  h.notifier.askUserQuestion(SESSION, 1, REF);
  h.notifier.taskFinished(SESSION, 'task-1', 'x', false, REF);
  assert.deepEqual(h.shown, []);
});

test('inputNeededNotifEnabled gates permission prompts and questions but not idle', () => {
  const h = harness({ inputNeededNotifEnabled: false });
  h.notifier.permissionRequested(SESSION, 1, 'Bash', REF);
  h.notifier.askUserQuestion(SESSION, 2, REF);
  h.advance(PERMISSION_PROMPT_NOTIFY_DELAY_MS * 2);
  assert.deepEqual(h.shown, [], 'both input-needed kinds are off');
  h.notifier.turnEnded(SESSION, REF);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS + 1);
  assert.equal(h.shown.length, 1, 'idle_prompt has its own gate and stays on');
});

test('taskCompleteNotifEnabled gates only finished tasks', () => {
  const h = harness({ taskCompleteNotifEnabled: false });
  h.notifier.taskFinished(SESSION, 'task-1', 'x', false, REF);
  assert.deepEqual(h.shown, []);
  h.notifier.askUserQuestion(SESSION, 1, REF);
  assert.equal(h.shown.length, 1);
});

// ── session teardown ─────────────────────────────────────────────────────────

test('ending the session disarms its pending timers', () => {
  const h = harness();
  h.notifier.turnEnded(SESSION, REF);
  h.notifier.permissionRequested(SESSION, 1, 'Bash', REF);
  h.notifier.sessionEnded(SESSION);
  h.advance(DEFAULT_IDLE_NOTIF_THRESHOLD_MS * 4);
  assert.deepEqual(h.shown, []);
});

// ── the preferences module itself ────────────────────────────────────────────

test('a malformed preference falls back to its default rather than silencing the gate', () => {
  const parsed = parseNotificationPreferences({ enabled: 'yes', inputNeededNotifEnabled: null, messageIdleNotifThresholdMs: 'soon' });
  assert.equal(parsed.enabled, true);
  assert.equal(parsed.inputNeededNotifEnabled, true);
  assert.equal(parsed.messageIdleNotifThresholdMs, DEFAULT_IDLE_NOTIF_THRESHOLD_MS);
});

test('the idle threshold is clamped, not trusted', () => {
  assert.equal(parseNotificationPreferences({ messageIdleNotifThresholdMs: 1 }).messageIdleNotifThresholdMs, 5_000);
  assert.equal(parseNotificationPreferences({ messageIdleNotifThresholdMs: 1e12 }).messageIdleNotifThresholdMs, 3_600_000);
});

test('isKindEnabled maps every kind to a gate', () => {
  const off = { ...defaultNotificationPreferences(), enabled: false };
  for (const kind of ['idle_prompt', 'permission_prompt', 'agent_needs_input', 'agent_completed'] as const) {
    assert.equal(isKindEnabled(off, kind), false, `${kind} must respect the master switch`);
    assert.equal(isKindEnabled(defaultNotificationPreferences(), kind), true, `${kind} defaults on`);
  }
});
