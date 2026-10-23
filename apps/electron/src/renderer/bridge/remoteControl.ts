/**
 * What the renderer shows of Remote Control: the pure rules for the settings
 * page, the session toggle, the top-bar indicator and `/remote-control`, and
 * one shared store over `window.lingxi.remoteControl` so that every row of the
 * sidebar, the top bar and the settings page read the same status without each
 * opening its own subscription.
 */
import { useEffect, useSyncExternalStore } from 'react';
import type {
  RemoteConnectionUi,
  RemoteControlApi,
  RemoteControlUiStatus,
  RemoteSessionsUi,
} from '../../shared/remoteControl';

export const REMOTE_INDICATOR_LABEL = '📱 远程中';

/** Whether Remote Control can be turned on from this window, and if not, why (visible copy). */
export interface RemoteAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

/**
 * The switches turn Remote Control on only for a signed-in person whose
 * administrator has not forbidden it. Turning something off is always allowed.
 */
export function remoteAvailability(status: RemoteControlUiStatus | null): RemoteAvailability {
  if (!status) return { available: false, reason: '正在读取远程控制状态…' };
  if (status.managedDisabled) return { available: false, reason: '管理员已禁用远程控制。' };
  if (!status.signedIn) return { available: false, reason: '请先登录灵犀账号，再开启远程控制。' };
  return { available: true };
}

export type RemoteStateTone = 'ok' | 'warn' | 'danger' | 'muted';

/** The state line of the settings page. */
export function remoteStateSummary(status: RemoteControlUiStatus): { label: string; tone: RemoteStateTone; detail?: string } {
  switch (status.state) {
    case 'online': return { label: '在线', tone: 'ok' };
    case 'starting': return { label: '连接中', tone: 'warn' };
    case 'error': return { label: '错误', tone: 'danger', ...(status.error ? { detail: status.error } : {}) };
    default: return { label: '未开启', tone: 'muted' };
  }
}

/** 直连 for a peer-to-peer link, 中转 when the bytes go through the relay or the encrypted tunnel. */
export function connectionTransportLabel(connection: Pick<RemoteConnectionUi, 'transport' | 'candidateType'>): '直连' | '中转' {
  if (connection.transport === 'tunnel') return '中转';
  return connection.candidateType === 'relay' ? '中转' : '直连';
}

export function platformLabel(platform: RemoteConnectionUi['platform']): string {
  switch (platform) {
    case 'android': return 'Android';
    case 'ios': return 'iOS';
    default: return '网页';
  }
}

/** The rows of the live-connections list; a device still waiting for approval is not yet a connection to show as connected. */
export function connectionRows(status: RemoteControlUiStatus): Array<{ connId: string; title: string; detail: string; approved: boolean }> {
  return status.connections.map((connection) => ({
    connId: connection.connId,
    title: connection.deviceName || '未命名设备',
    detail: connection.approved
      ? `${platformLabel(connection.platform)} · ${connectionTransportLabel(connection)}`
      : `${platformLabel(connection.platform)} · 等待批准`,
    approved: connection.approved,
  }));
}

export function isSessionRemote(sessions: RemoteSessionsUi | null, sessionId: string | null | undefined): boolean {
  return Boolean(sessionId) && Boolean(sessions?.sessions.some((entry) => entry.sessionId === sessionId));
}

/** The top-bar indicator of a session, or `null` when viewers cannot see it. */
export function remoteIndicator(
  sessions: RemoteSessionsUi | null,
  sessionId: string | null | undefined,
): { label: string; viewers: number; text: string } | null {
  if (!sessionId) return null;
  const entry = sessions?.sessions.find((candidate) => candidate.sessionId === sessionId);
  if (!entry) return null;
  const viewers = Math.max(0, Math.floor(entry.viewers));
  return {
    label: REMOTE_INDICATOR_LABEL,
    viewers,
    text: viewers > 0 ? `${REMOTE_INDICATOR_LABEL} · ${viewers}` : REMOTE_INDICATOR_LABEL,
  };
}

/** The label of the session-menu entry. */
export function remoteMenuLabel(enabled: boolean, availability: RemoteAvailability): string {
  if (enabled) return '关闭远程控制';
  return availability.available ? '开启远程控制' : `开启远程控制（${availability.reason ?? '暂不可用'}）`;
}

export const REMOTE_CONNECT_HINT = '在手机的灵犀应用里，或用同一账号登录网页 /remote 即可连接；新设备需要在这台电脑上核对 6 位配对码，再点击“允许”。';

export interface RemoteToggleOutcome {
  readonly message: string;
  readonly isError: boolean;
}

/** `/remote-control` takes nothing, `on` or `off`; anything else is a usage error. */
export function parseRemoteToggleArg(args: string): boolean | undefined | 'invalid' {
  const word = args.trim().toLowerCase();
  if (word === '') return undefined;
  if (word === 'on') return true;
  if (word === 'off') return false;
  return 'invalid';
}

/**
 * Run `/remote-control` for one session: toggle it (or set it explicitly) and
 * describe the result. Free of React and `window` so the whole decision is
 * testable with a fake API.
 */
export async function toggleRemoteForSession(
  api: Pick<RemoteControlApi, 'status' | 'sessions' | 'setSession'> | undefined,
  sessionId: string | null | undefined,
  wanted: boolean | undefined,
): Promise<RemoteToggleOutcome> {
  if (!api) return { message: '这个版本的灵犀不支持远程控制。', isError: true };
  if (!sessionId) return { message: '请先打开一个会话，再开启远程控制。', isError: true };
  try {
    const [status, sessions] = await Promise.all([api.status(), api.sessions()]);
    const current = isSessionRemote(sessions, sessionId);
    const enable = wanted ?? !current;
    if (enable) {
      const availability = remoteAvailability(status);
      if (!availability.available) return { message: `无法开启远程控制：${availability.reason}`, isError: true };
      if (current) return { message: `这个会话的远程控制已经开启。${REMOTE_CONNECT_HINT}`, isError: false };
      await api.setSession(sessionId, true);
      return { message: `已为这个会话开启远程控制。${REMOTE_CONNECT_HINT}`, isError: false };
    }
    if (!current) return { message: '这个会话的远程控制本来就是关闭的。', isError: false };
    await api.setSession(sessionId, false);
    return { message: '已关闭这个会话的远程控制，其他设备不再看得到它。', isError: false };
  } catch (cause) {
    return { message: `远程控制操作失败：${cause instanceof Error ? cause.message : String(cause)}`, isError: true };
  }
}

// ─── shared store ────────────────────────────────────────────

export interface RemoteControlSnapshot {
  readonly status: RemoteControlUiStatus | null;
  readonly sessions: RemoteSessionsUi | null;
}

const EMPTY: RemoteControlSnapshot = { status: null, sessions: null };

/** The store behind {@link useRemoteControl}; exported with an injectable API for tests. */
export class RemoteControlStore {
  private snapshot: RemoteControlSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  private offs: Array<() => void> = [];
  private generation = 0;

  constructor(private readonly getApi: () => RemoteControlApi | undefined) {}

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    if (this.listeners.size === 1) this.open();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.close();
    };
  };

  readonly getSnapshot = (): RemoteControlSnapshot => this.snapshot;

  /** Read the status and sessions again, for example after a switch was flipped. */
  refresh(): Promise<void> {
    const api = this.getApi();
    if (!api) return Promise.resolve();
    const generation = this.generation;
    return Promise.all([api.status(), api.sessions()]).then(([status, sessions]) => {
      if (generation === this.generation) this.set({ status, sessions });
    }, () => undefined);
  }

  private open(): void {
    const api = this.getApi();
    if (!api) return;
    this.generation += 1;
    this.offs = [
      api.onStatus((status) => this.set({ ...this.snapshot, status })),
      api.onSessions((sessions) => this.set({ ...this.snapshot, sessions })),
    ];
    void this.refresh();
  }

  private close(): void {
    this.generation += 1;
    for (const off of this.offs.splice(0)) off();
    this.snapshot = EMPTY;
  }

  private set(next: RemoteControlSnapshot): void {
    this.snapshot = next;
    for (const listener of [...this.listeners]) listener();
  }
}

const sharedStore = new RemoteControlStore(() => (typeof window === 'undefined' ? undefined : window.lingxi?.remoteControl));

/** Live Remote Control status and the sessions viewers may see; both `null` until first read or when the app has no Remote Control. */
export function useRemoteControl(): RemoteControlSnapshot {
  return useSyncExternalStore(sharedStore.subscribe, sharedStore.getSnapshot, () => EMPTY);
}

/** The app's Remote Control API, when this build has one. */
export function remoteControlApi(): RemoteControlApi | undefined {
  return typeof window === 'undefined' ? undefined : window.lingxi?.remoteControl;
}

/** Re-read after a change the events do not report. */
export function refreshRemoteControl(): Promise<void> {
  return sharedStore.refresh();
}

/** Keep the store warm while a component is mounted (used by components that only act, never read). */
export function useRemoteControlRefresh(): void {
  useEffect(() => sharedStore.subscribe(() => undefined), []);
}
