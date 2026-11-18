/** Notification delivery policy for the Desktop main process. */
import type { DeliveryLedger } from './inbox/deliveryLedger.js';
import { nativeTr } from '../shared/nativeLocale.js';
import {
  PERMISSION_PROMPT_NOTIFY_DELAY_MS,
  defaultNotificationPreferences,
  isKindEnabled,
  type NotificationKind,
  type NotificationPreferences,
} from '../shared/notificationPreferences.js';
import type { SessionRef } from '../shared/settings.js';
import type { RemotePushKind } from './remoteControl/pushRequests.js';

/** Copy, kept in step with `resources/translations/*.json` so all three clients
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

/** The system counts as idle (the user is away from the computer) after this long without input. */
export const REMOTE_PUSH_IDLE_SECONDS = 60;

/** Cap on remembered dedupe keys, matching `scheduled.ts`'s own ring. */
const MAX_DELIVERED_KEYS = 500;

type TimerHandle = ReturnType<typeof setTimeout>;

/** The notification kinds that reach other devices, and what Remote Control calls them. */
const REMOTE_PUSH_KINDS: Partial<Record<NotificationKind, RemotePushKind>> = {
  permission_prompt: 'permission',
  agent_needs_input: 'question',
  idle_prompt: 'idle',
  agent_completed: 'task',
};

/** The two Bots of a task, by name. */
export interface DelegatedTask {
  /** The Bot that does the task. */
  receiver: string;
  /** The Bot that handed it over. */
  sender: string;
}

export interface HostNotifierDeps {
  /** Raises the actual OS notification. Injected so tests never touch Electron. */
  show(title: string, body: string, ref?: SessionRef): void;
  /** The foreground renderer reports the session whose content is actually visible. */
  isConversationVisible?(sessionKey: string, ref?: SessionRef): boolean;
  inApp?(title: string, body: string, ref?: SessionRef): void;
  ledger?: DeliveryLedger;
  conversationNotificationsEnabled?(sessionKey: string,kind:NotificationKind): boolean;
  hasCommittedActivity?(sessionKey: string): boolean;
  /** True while the main window has OS focus. */
  isWindowFocused(): boolean;
  /**
   * Remote Control's push: called for the kinds that need the user (a
   * permission prompt, a question, the session going idle, a task finishing)
   * when the user enabled that kind and is not at the computer. Whether the
   * session has Remote Control on and whether a viewer is already looking is
   * for the callee to decide.
   */
  remote?(kind: RemotePushKind, sessionId: string): void;
  /** Seconds since the last keyboard or mouse input (`powerMonitor.getSystemIdleTime`). */
  systemIdleSeconds?(): number;
  /**
   * The task a session is doing for another Bot, if it is doing one. A request
   * for an answer in such a session says whose task it is. Asked when the
   * notification is about to go out, not when the request came.
   */
  delegation?(sessionKey: string): DelegatedTask | undefined;
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
  questionTimers: Map<number, TimerHandle>;
  completions: Set<string>;
  completionTimer?: TimerHandle;
  ref?: SessionRef;
}

export class HostNotifier {
  private prefs: NotificationPreferences = defaultNotificationPreferences();
  private readonly sessions = new Map<string, SessionNotificationState>();
  private readonly delivered = new Set<string>();
  private readonly deliveredRemote = new Set<string>();
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
   * that ends, starts and ends again gets one fresh timer, not two. A turn the
   * host ran on its own (`idle: false`) is one nobody is waiting to come back
   * to, so it only ends: no timer is armed for it. */
  turnEnded(sessionKey: string, ref?: SessionRef, options: { idle?: boolean } = {}): void {
    const state = this.stateFor(sessionKey, ref);
    state.turnActive = false;
    state.turnEndedAt = this.now();
    this.disarmIdle(state);
    if (options.idle === false || !isKindEnabled(this.prefs, 'idle_prompt')) return;
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
    for (const timer of state.questionTimers.values()) this.unschedule(timer);
    if (state.completionTimer !== undefined) this.unschedule(state.completionTimer);
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
      const task = this.deps.delegation?.(sessionKey);
      this.post('permission_prompt', sessionKey, [`${sessionKey}:permission:${requestId}`],
        task ? nativeTr('native_task_input_title', task.receiver, task.sender) : NOTIFICATION_COPY.permissionTitle,
        task ? nativeTr('native_task_input_permission', toolName) : NOTIFICATION_COPY.permissionBody(toolName), state.ref);
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

  askUserQuestion(sessionKey: string, requestId: number, ref?: SessionRef): void {
    const state = this.stateFor(sessionKey, ref);
    if (!isKindEnabled(this.prefs, 'agent_needs_input') || state.questionTimers.has(requestId)) return;
    const timer = this.schedule(() => {
      state.questionTimers.delete(requestId);
      const task = this.deps.delegation?.(sessionKey);
      this.post('agent_needs_input', sessionKey, [`${sessionKey}:question:${requestId}`],
        task ? nativeTr('native_task_input_title', task.receiver, task.sender) : NOTIFICATION_COPY.idleTitle,
        task ? nativeTr('native_task_input_question') : NOTIFICATION_COPY.idleBody, state.ref);
    }, PERMISSION_PROMPT_NOTIFY_DELAY_MS);
    state.questionTimers.set(requestId, timer);
  }

  questionSettled(sessionKey: string, requestId: number): void {
    const timers = this.sessions.get(sessionKey)?.questionTimers;
    const timer = timers?.get(requestId);
    if (timer !== undefined) { this.unschedule(timer); timers?.delete(requestId); }
  }

  taskFinished(sessionKey: string, taskId: string, _label: string | undefined, failed: boolean, ref?: SessionRef): void {
    if (!isKindEnabled(this.prefs, 'agent_completed') || this.deps.hasCommittedActivity?.(sessionKey)) return;
    const state = this.stateFor(sessionKey, ref);
    const key = `${sessionKey}:task:${taskId}:${failed ? 'failed' : 'done'}`;
    if (failed) {
      this.post('agent_completed', sessionKey, [key], NOTIFICATION_COPY.failedTitle, NOTIFICATION_COPY.failedBody, state.ref);
      return;
    }
    state.completions.add(key);
    if (state.completionTimer !== undefined) return;
    state.completionTimer = this.schedule(() => {
      state.completionTimer = undefined;
      const keys = [...state.completions];
      state.completions.clear();
      this.post('agent_completed', sessionKey, keys, NOTIFICATION_COPY.completedTitle, NOTIFICATION_COPY.completedBody, state.ref);
    }, 5_000);
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

  accountChanged(): void {
    this.disarmAll();
    this.sessions.clear();
    this.delivered.clear();
    this.deliveredRemote.clear();
    this.lastInteractionAt = 0;
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
      state = { turnActive: false, dialogsOnScreen: 0, permissionTimers: new Map(), questionTimers: new Map(), completions: new Set() };
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
    this.post('idle_prompt', sessionKey, [`${sessionKey}:idle:${state.turnEndedAt}`],
      NOTIFICATION_COPY.idleTitle, NOTIFICATION_COPY.idleBody, state.ref);
  }

  private fresh(prefix: string, keys: string[], memory: Set<string>): string[] {
    return keys.map(key => prefix + key).filter(key => this.deps.ledger
      ? !this.deps.ledger.has(key, this.now()) : !memory.has(key));
  }

  private remember(keys: string[], memory: Set<string>): void {
    if (this.deps.ledger) { this.deps.ledger.remember(keys, this.now()); return; }
    for (const key of keys) memory.add(key);
    while (memory.size > MAX_DELIVERED_KEYS) memory.delete(memory.values().next().value!);
  }

  private post(kind: NotificationKind, sessionKey: string, keys: string[], title: string, body: string, ref?: SessionRef): void {
    if (this.disposed || !isKindEnabled(this.prefs, kind) || this.deps.conversationNotificationsEnabled?.(sessionKey,kind) === false) return;
    const focused = this.deps.isWindowFocused();
    const remoteKind = REMOTE_PUSH_KINDS[kind];
    if (remoteKind && this.deps.remote && (!focused || (this.deps.systemIdleSeconds?.() ?? 0) >= REMOTE_PUSH_IDLE_SECONDS)) {
      const remote = this.fresh('remote:', keys, this.deliveredRemote);
      if (remote.length) {
        try { this.deps.remote(remoteKind, sessionKey); this.remember(remote, this.deliveredRemote); }
        catch { /* A remote push failure never prevents local delivery. */ }
      }
    }
    if (focused && (this.deps.isConversationVisible?.(sessionKey, ref) || !this.deps.inApp)) return;
    const fresh = this.fresh('local:', keys, this.delivered);
    if (!fresh.length) return;
    if (focused) this.deps.inApp!(title, body, ref);
    else if (!this.prefs.showPreviews && (kind === 'permission_prompt' || kind === 'agent_needs_input')) {
      this.deps.show(kind === 'permission_prompt' ? NOTIFICATION_COPY.permissionTitle : NOTIFICATION_COPY.idleTitle, NOTIFICATION_COPY.idleBody, ref);
    } else this.deps.show(title, body, ref);
    this.remember(fresh, this.delivered);
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
      for (const timer of state.questionTimers.values()) this.unschedule(timer);
      state.questionTimers.clear();
      if (state.completionTimer !== undefined) this.unschedule(state.completionTimer);
      state.completionTimer = undefined;
      state.completions.clear();
    }
  }
}
