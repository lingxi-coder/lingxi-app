/**
 * OS notifications for the desktop shell.
 *
 * The POLICY here is Claude Code 2.1.270's, copied rather than invented, so
 * that the three GUI clients and the CLI do not end up with four different
 * ideas of when a notification is warranted. Two things follow from that and
 * are worth stating, because both are counter-intuitive:
 *
 * 1. **There is no "the turn finished" notification.** Upstream fires
 *    `idle_prompt` ("waiting for your input") only once the session has sat
 *    idle for `messageIdleNotifThresholdMs` AFTER the turn ended, and only if
 *    the user has not come back in the meantime. A turn that finishes while
 *    you are looking at something else and that you return to within the
 *    threshold notifies nothing, deliberately.
 * 2. **A permission prompt answered quickly notifies nothing.** Upstream arms
 *    a 6s timer when the prompt appears and clears it in a `finally` when the
 *    prompt settles.
 *
 * The one adaptation: upstream's "local" channel is a terminal bell / OSC
 * escape, which is harmless to emit at a terminal you are staring at, so it
 * has no foreground suppression. A banner over the window you are actively
 * using is not harmless, so delivery here also requires the main window to be
 * unfocused. iOS already behaves this way for its own reasons
 * (`AppNotificationDelegate.willPresent` returns `[]` for conversation routes
 * while foregrounded).
 *
 * Everything runs in the MAIN process. It has to: the renderer's session
 * denies every Web permission but `media` (`main/index.ts`'s
 * `setPermissionRequestHandler`), and `scripts/packaged-app-smoke.mjs` pins
 * that by asserting `Notification.requestPermission()` resolves `'denied'`.
 */

import {
  PERMISSION_PROMPT_NOTIFY_DELAY_MS,
  defaultNotificationPreferences,
  isKindEnabled,
  type NotificationKind,
  type NotificationPreferences,
} from '../shared/notificationPreferences.js';
import type { SessionRef } from '../shared/settings.js';

/** Copy, kept in step with `clients/translations/*.json` so all three clients
 * say the same thing. Desktop has no i18n framework (see `renderer/settingsLabel.ts`),
 * so these are literals rather than lookups. */
export const NOTIFICATION_COPY = {
  idleTitle: '灵犀正在等待你',
  idleBody: '后台对话需要你的输入才能继续',
  permissionTitle: '灵犀需要你的许可',
  /** `chat_background_permission_text` — `%@` filled with the tool name. */
  permissionBody: (tool: string): string => `灵犀需要你的许可来使用 ${tool}`,
  completedTitle: '灵犀已完成',
  completedBody: '后台对话已完成，点按查看结果',
  failedTitle: '对话处理失败',
  failedBody: '点按返回对话查看并重试',
} as const;

/** Cap on remembered dedupe keys, matching `scheduled.ts`'s own ring. */
const MAX_DELIVERED_KEYS = 500;

type TimerHandle = ReturnType<typeof setTimeout>;

export interface HostNotifierDeps {
  /** Raises the actual OS notification. Injected so tests never touch Electron. */
  show(title: string, body: string, ref?: SessionRef): void;
  /** True while the main window has OS focus. */
  isWindowFocused(): boolean;
  now?(): number;
  schedule?(fn: () => void, ms: number): TimerHandle;
  unschedule?(handle: TimerHandle): void;
}

interface SessionNotificationState {
  /** Armed after `turnEnded`, disarmed by a new turn or by the user returning. */
  idleTimer?: TimerHandle;
  /** When the last turn ended. Compared against `lastInteractionAt` at fire time. */
  turnEndedAt?: number;
  /** A turn is running; upstream's `isLoading` gate. */
  turnActive: boolean;
  /** Prompts/questionnaires currently on screen; upstream's `isDialogOnScreen()`. */
  dialogsOnScreen: number;
  /** request_id -> armed 6s permission timer. */
  permissionTimers: Map<number, TimerHandle>;
  ref?: SessionRef;
}

export class HostNotifier {
  private prefs: NotificationPreferences = defaultNotificationPreferences();
  private readonly sessions = new Map<string, SessionNotificationState>();
  private readonly delivered = new Set<string>();
  private lastInteractionAt = 0;
  private disposed = false;

  private readonly now: () => number;
  private readonly schedule: (fn: () => void, ms: number) => TimerHandle;
  private readonly unschedule: (handle: TimerHandle) => void;

  constructor(private readonly deps: HostNotifierDeps) {
    this.now = deps.now ?? Date.now;
    this.schedule = deps.schedule ?? ((fn, ms) => {
      const handle = setTimeout(fn, ms);
      // Never hold the event loop open for a notification; upstream calls
      // `t.unref()` on the permission timer for the same reason.
      handle.unref?.();
      return handle;
    });
    this.unschedule = deps.unschedule ?? clearTimeout;
  }

  setPreferences(prefs: NotificationPreferences | undefined): void {
    this.prefs = prefs ?? defaultNotificationPreferences();
    if (!this.prefs.enabled) this.disarmAll();
  }

  /**
   * Upstream's `getLastInteractionTime()`. Anything that proves the user is
   * back at the keyboard: sending a prompt, cancelling, answering a
   * permission prompt or a questionnaire, or the window taking focus.
   */
  noteInteraction(): void { this.lastInteractionAt = this.now(); }

  turnStarted(sessionKey: string, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    state.turnActive = true;
    this.disarmIdle(state);
  }

  /** Arms `idle_prompt`. Upstream re-arms on every turn-state change; a turn
   * that ends, starts and ends again gets one fresh timer, not two. */
  turnEnded(sessionKey: string, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    state.turnActive = false;
    state.turnEndedAt = this.now();
    this.disarmIdle(state);
    if (!isKindEnabled(this.prefs, 'idle_prompt')) return;
    const threshold = this.prefs.messageIdleNotifThresholdMs;
    state.idleTimer = this.schedule(() => {
      state.idleTimer = undefined;
      this.fireIdlePrompt(sessionKey, state, threshold);
    }, threshold);
  }

  sessionEnded(sessionKey: string): void {
    const state = this.sessions.get(sessionKey);
    if (!state) return;
    this.disarmIdle(state);
    for (const timer of state.permissionTimers.values()) this.unschedule(timer);
    this.sessions.delete(sessionKey);
  }

  /** Upstream's `isDialogOnScreen()` input. */
  setDialogsOnScreen(sessionKey: string, count: number, ref?: SessionRef): void {
    this.stateFor(sessionKey, ref).dialogsOnScreen = Math.max(0, count);
  }

  /** Arms the 6s `permission_prompt` timer. Call AFTER the request is actually
   * forwarded to the renderer — a request the bridge drops has no prompt to
   * answer, so notifying about it would send the user to a dialog that is
   * never going to appear. */
  permissionRequested(sessionKey: string, requestId: number, toolName: string, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    if (state.permissionTimers.has(requestId)) return;
    if (!isKindEnabled(this.prefs, 'permission_prompt')) return;
    const timer = this.schedule(() => {
      state.permissionTimers.delete(requestId);
      this.post('permission_prompt', `${sessionKey}:permission:${requestId}`,
        NOTIFICATION_COPY.permissionTitle, NOTIFICATION_COPY.permissionBody(toolName), state.ref);
    }, PERMISSION_PROMPT_NOTIFY_DELAY_MS);
    state.permissionTimers.set(requestId, timer);
  }

  /** Upstream's `clearTimeout` in the `finally`: answered inside 6s, nothing fires. */
  permissionSettled(sessionKey: string, requestId: number): void {
    const state = this.sessions.get(sessionKey);
    const timer = state?.permissionTimers.get(requestId);
    if (timer === undefined || !state) return;
    this.unschedule(timer);
    state.permissionTimers.delete(requestId);
  }

  /** `agent_needs_input`. Upstream fires this on the band transition with no
   * debounce — unlike a permission prompt, a questionnaire has no "answered
   * in the next six seconds" case worth waiting out. */
  askUserQuestion(sessionKey: string, requestId: number, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    this.post('agent_needs_input', `${sessionKey}:question:${requestId}`,
      NOTIFICATION_COPY.idleTitle, NOTIFICATION_COPY.idleBody, state.ref);
  }

  /** `agent_completed`. `label` is the task's human description when the row
   * list supplied one; the bare id is the last resort, not the default. */
  taskFinished(sessionKey: string, taskId: string, label: string | undefined, failed: boolean, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    const name = label?.trim() || taskId;
    this.post('agent_completed', `${sessionKey}:task:${taskId}:${failed ? 'failed' : 'done'}`,
      failed ? NOTIFICATION_COPY.failedTitle : NOTIFICATION_COPY.completedTitle,
      failed ? `${name} 执行失败` : `${name} 已完成`, state.ref);
  }

  /**
   * Scheduled (cron) runs. Port-only; upstream has no equivalent.
   *
   * Two deliberate differences from every other kind here:
   *
   * - **No dedupe ring.** `scheduled.ts` keeps its own delivered set on disk
   *   so that a process restart cannot re-notify; a second in-memory ring
   *   would only be able to drop what that one already let through.
   * - **No focus gate.** The window being focused means the user is looking at
   *   a conversation, which says nothing about a run that fired on a timer
   *   while they were doing so. It also matters mechanically: `notifyOnce`
   *   RESERVES the dedupe key before delivering, so a drop here would burn
   *   the key and the run would never be reported at all. The per-task
   *   `notificationPolicy` (`all` / `failed` / `none`) is the real filter.
   */
  scheduledRun(title: string, body: string, ref?: SessionRef): boolean {
    if (this.disposed) return false;
    if (!isKindEnabled(this.prefs, 'scheduled_run')) return false;
    this.deps.show(title, body, ref);
    return true;
  }

  dispose(): void {
    this.disposed = true;
    this.disarmAll();
    this.sessions.clear();
  }

  // ── internals ────────────────────────────────────────────────────────────

  private stateFor(sessionKey: string, ref?: SessionRef): SessionNotificationState {
    let state = this.sessions.get(sessionKey);
    if (!state) {
      state = { turnActive: false, dialogsOnScreen: 0, permissionTimers: new Map() };
      this.sessions.set(sessionKey, state);
    }
    if (ref) state.ref = ref;
    return state;
  }

  /**
   * Upstream `class See`'s fire-time re-check, clause for clause. Every one of
   * these can become true DURING the countdown, which is exactly why upstream
   * re-checks instead of trusting the timer.
   */
  private fireIdlePrompt(sessionKey: string, state: SessionNotificationState, threshold: number): void {
    if (this.disposed) return;
    if (state.turnActive) return;                                     // !isLoading
    if (state.turnEndedAt === undefined) return;                      // lastQueryCompletionTime === 0
    if (this.lastInteractionAt > state.turnEndedAt) return;           // the user came back
    if (state.dialogsOnScreen > 0) return;                            // !isDialogOnScreen()
    if (this.now() - state.turnEndedAt < threshold) return;           // elapsed >= threshold
    this.post('idle_prompt', `${sessionKey}:idle:${state.turnEndedAt}`,
      NOTIFICATION_COPY.idleTitle, NOTIFICATION_COPY.idleBody, state.ref);
  }

  private canDeliver(kind: NotificationKind): boolean {
    if (this.disposed) return false;
    if (!isKindEnabled(this.prefs, kind)) return false;
    // The GUI adaptation: no banner for the window the user is already in.
    return !this.deps.isWindowFocused();
  }

  private post(kind: NotificationKind, dedupeKey: string, title: string, body: string, ref?: SessionRef): void {
    if (!this.canDeliver(kind)) return;
    if (this.delivered.has(dedupeKey)) return;
    this.delivered.add(dedupeKey);
    if (this.delivered.size > MAX_DELIVERED_KEYS) {
      for (const key of this.delivered) {
        this.delivered.delete(key);
        if (this.delivered.size <= MAX_DELIVERED_KEYS) break;
      }
    }
    this.deps.show(title, body, ref);
  }

  private disarmIdle(state: SessionNotificationState): void {
    if (state.idleTimer === undefined) return;
    this.unschedule(state.idleTimer);
    state.idleTimer = undefined;
  }

  private disarmAll(): void {
    for (const state of this.sessions.values()) {
      this.disarmIdle(state);
      for (const timer of state.permissionTimers.values()) this.unschedule(timer);
      state.permissionTimers.clear();
    }
  }
}
