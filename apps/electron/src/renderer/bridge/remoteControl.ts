import { tr } from '../i18n/locale';
import { desktopHost } from './bridgeConnection';
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

function remoteIndicatorLabel(): string { return tr('desktop_shell_remote_indicator'); }

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
  if (!status) return { available: false, reason: tr('desktop_shell_remote_loading') };
  if (status.managedDisabled) return { available: false, reason: tr('desktop_shell_remote_disabled') };
  if (!status.signedIn) return { available: false, reason: tr('desktop_shell_remote_signin') };
  return { available: true };
}

/**
 * The same for a Bot's switch, which adds one more condition: Remote Control
 * must already be on for this computer, because the Bot's switch only decides
 * which of the computer's sessions viewers may reach. It is not a way to turn
 * Remote Control on, as the session toggle is.
 */
export function botRemoteAvailability(status: RemoteControlUiStatus | null): RemoteAvailability {
  const general = remoteAvailability(status);
  if (!general.available || !status) return general;
  // A running service with the setting off is the development switch.
  if (!status.enabled && status.state === 'off') return { available: false, reason: tr('bots_remote_computer_off') };
  return { available: true };
}

export type RemoteStateTone = 'ok' | 'warn' | 'danger' | 'muted';

/** The state line of the settings page. */
export function remoteStateSummary(status: RemoteControlUiStatus): { label: string; tone: RemoteStateTone; detail?: string } {
  switch (status.state) {
    case 'online': return { label: tr('desktop_shell_remote_online'), tone: 'ok' };
    case 'starting': return { label: tr('desktop_shell_remote_connecting'), tone: 'warn' };
    case 'error': return { label: tr('cron_error_section'), tone: 'danger', ...(status.error ? { detail: status.error } : {}) };
    default: return { label: tr('desktop_shell_remote_off'), tone: 'muted' };
  }
}

/** 直连 for a peer-to-peer link, 中转 when the bytes go through the relay or the encrypted tunnel. */
export function connectionTransportLabel(connection: Pick<RemoteConnectionUi, 'transport' | 'candidateType'>): string {
  if (connection.transport === 'tunnel') return tr('desktop_shell_remote_relay');
  return connection.candidateType === 'relay' ? tr('desktop_shell_remote_relay') : tr('desktop_shell_remote_direct');
}

export function platformLabel(platform: RemoteConnectionUi['platform']): string {
  switch (platform) {
    case 'android': return 'Android';
    case 'ios': return 'iOS';
    default: return tr('desktop_shell_remote_web');
  }
}

/** The rows of the live-connections list; a device still waiting for approval is not yet a connection to show as connected. */
export function connectionRows(status: RemoteControlUiStatus): Array<{ connId: string; title: string; detail: string; approved: boolean }> {
  return status.connections.map((connection) => ({
    connId: connection.connId,
    title: connection.deviceName || tr('desktop_shell_unnamed_device'),
    detail: connection.approved
      ? `${platformLabel(connection.platform)} · ${connectionTransportLabel(connection)}`
      : `${platformLabel(connection.platform)} · ${tr('desktop_shell_remote_approval_pending')}`,
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
    label: remoteIndicatorLabel(),
    viewers,
    text: viewers > 0 ? `${remoteIndicatorLabel()} · ${viewers}` : remoteIndicatorLabel(),
  };
}

/** The label of the session-menu entry. */
export function remoteMenuLabel(enabled: boolean, availability: RemoteAvailability): string {
  if (enabled) return tr('desktop_shell_remote_turnoff');
  return availability.available ? tr('desktop_shell_remote_on') : tr('desktop_shell_remote_unavailable', availability.reason ?? tr('settings_linux_task_unavailable'));
}

function remoteConnectHint(): string { return tr('desktop_shell_remote_connect_hint'); }

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
  if (!api) return { message: tr('desktop_shell_remote_unsupported'), isError: true };
  if (!sessionId) return { message: tr('desktop_shell_remote_open_session'), isError: true };
  try {
    const [status, sessions] = await Promise.all([api.status(), api.sessions()]);
    const current = isSessionRemote(sessions, sessionId);
    const enable = wanted ?? !current;
    if (enable) {
      const availability = remoteAvailability(status);
      if (!availability.available) return { message: tr('desktop_shell_remote_enable_failed', availability.reason ?? ''), isError: true };
      if (current) return { message: tr('desktop_shell_remote_already_on', remoteConnectHint()), isError: false };
      await api.setSession(sessionId, true);
      return { message: tr('desktop_shell_remote_enabled', remoteConnectHint()), isError: false };
    }
    if (!current) return { message: tr('desktop_shell_remote_already_off'), isError: false };
    await api.setSession(sessionId, false);
    return { message: tr('desktop_shell_remote_turned_off'), isError: false };
  } catch (cause) {
    return { message: tr('desktop_shell_remote_failed', cause instanceof Error ? cause.message : String(cause)), isError: true };
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

const sharedStore = new RemoteControlStore(() => desktopHost()?.remoteControl);

/** Live Remote Control status and the sessions viewers may see; both `null` until first read or when the app has no Remote Control. */
export function useRemoteControl(): RemoteControlSnapshot {
  return useSyncExternalStore(sharedStore.subscribe, sharedStore.getSnapshot, () => EMPTY);
}

/** The app's Remote Control API, when this build has one. */
export function remoteControlApi(): RemoteControlApi | undefined {
  return desktopHost()?.remoteControl;
}

/** Re-read after a change the events do not report. */
export function refreshRemoteControl(): Promise<void> {
  return sharedStore.refresh();
}

/** Keep the store warm while a component is mounted (used by components that only act, never read). */
export function useRemoteControlRefresh(): void {
  useEffect(() => sharedStore.subscribe(() => undefined), []);
}
