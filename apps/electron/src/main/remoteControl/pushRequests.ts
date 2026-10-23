/**
 * Asking the push service to tell the account's other devices that a session
 * needs them. The request carries no conversation text: the service sends a
 * generic notification whose route is `remote/<host_id>/<session_id>`.
 *
 * `HostNotifier` decides whether the desktop itself should be heard from (the
 * notification kinds the user enabled, a window out of focus or a system that
 * has been idle). {@link RemotePush} adds what only Remote Control knows: the
 * session has Remote Control on and nobody is looking at it on a viewer.
 * A connected viewer in the background still needs the notification.
 */

import type { Clock } from '@lingxi/bridge-client/remote';

import { describeError, type RemoteDiagnostics } from './diagnostics.js';

export type RemotePushKind = 'permission' | 'question' | 'idle' | 'task';

export interface PushRequesterDeps {
  /** The push API's base address, without a trailing slash. */
  baseUrl: string;
  /** This computer's id; `undefined` while Remote Control is not running. */
  hostId(): string | undefined;
  getIdToken(forceRefresh?: boolean): Promise<string>;
  /** `net.fetch` in production. */
  fetch(url: string, init: { method: string; headers: Record<string, string>; body: string; signal?: AbortSignal }): Promise<{ ok: boolean; status: number }>;
  diagnostics: RemoteDiagnostics;
  timeoutMs?: number;
}

const REQUEST_TIMEOUT_MS = 10_000;

export class PushRequester {
  constructor(private readonly deps: PushRequesterDeps) {}

  /** Ask for the push. Resolves true if the service accepted it; a failure is logged, never thrown. */
  async request(kind: RemotePushKind, sessionId: string, excludeInstallationIds: readonly string[]): Promise<boolean> {
    const hostId = this.deps.hostId();
    if (hostId === undefined) return false;
    const body = JSON.stringify({
      kind,
      host_id: hostId,
      session_id: sessionId,
      ...(excludeInstallationIds.length > 0 ? { exclude_installation_ids: excludeInstallationIds.slice(0, 16) } : {}),
    });
    try {
      let response = await this.post(body, false);
      // An ID token that was refused may only have gone stale.
      if (response.status === 401) response = await this.post(body, true);
      if (!response.ok) this.deps.diagnostics('warn', `the push request was refused (status ${response.status})`);
      return response.ok;
    } catch (error) {
      this.deps.diagnostics('warn', `the push request failed (${describeError(error)})`);
      return false;
    }
  }

  private async post(body: string, forceRefresh: boolean): Promise<{ ok: boolean; status: number }> {
    const token = await this.deps.getIdToken(forceRefresh);
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.deps.timeoutMs ?? REQUEST_TIMEOUT_MS);
    try {
      return await this.deps.fetch(`${this.deps.baseUrl}/v1/push/self/messages`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body,
        signal: controller.signal,
      });
    } finally {
      clearTimeout(timer);
    }
  }
}

/** What {@link RemotePush} needs to know about the viewers. */
export interface PushPresence {
  isEnabled(sessionId: string): boolean;
  /** How many viewers have this session open in the foreground. */
  foregroundViewers(sessionId: string): number;
}

export interface RemotePushDeps {
  requester: Pick<PushRequester, 'request'>;
  presence: PushPresence;
  /** Whether Remote Control is running: signed in, switched on, allowed. */
  active(): boolean;
  clock: Clock;
}

/** The same request for the same session and kind is not repeated within this long. */
const REPEAT_WINDOW_MS = 30_000;
const MAX_RECENT = 200;

export class RemotePush {
  private readonly recent = new Map<string, number>();

  constructor(private readonly deps: RemotePushDeps) {}

  /** The desktop has decided a notification of this kind is due: send one to the other devices if it should be. */
  notify(kind: RemotePushKind, sessionId: string): void {
    const { presence, clock } = this.deps;
    if (!this.deps.active() || !presence.isEnabled(sessionId)) return;
    if (presence.foregroundViewers(sessionId) > 0) return;
    const key = `${sessionId}:${kind}`;
    const now = clock.now();
    const last = this.recent.get(key);
    if (last !== undefined && now - last < REPEAT_WINDOW_MS) return;
    this.recent.set(key, now);
    if (this.recent.size > MAX_RECENT) {
      for (const [other, at] of this.recent) if (now - at >= REPEAT_WINDOW_MS) this.recent.delete(other);
    }
    // No viewer is in the foreground. Merely being attached does not mean a
    // suspended phone can display an incoming request; it must receive push.
    void this.deps.requester.request(kind, sessionId, []);
  }
}
