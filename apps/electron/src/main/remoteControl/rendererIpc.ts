/**
 * What the desktop window sees of Remote Control: the status, this computer's
 * switch, the approved devices, the per-session switches and the prompts that
 * viewers send. Free of Electron; `HostController` registers the handlers
 * (after checking the sender of every call) and supplies the way to reach the
 * windows. Nothing here returns a token or a key.
 */

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
  type RemoteControlUiStatus,
  type RemoteDeviceUi,
  type RemotePromptEchoUi,
  type RemoteSessionsUi,
} from '../../shared/remoteControl.js';
import type { SessionRef } from '../bridgeTypes.js';
import type { RegisteredDevice } from './deviceRegistry.js';
import type { RemotePolicy } from './policy.js';
import type { RemoteControlStatus } from './RemoteControlService.js';
import type { RemotePromptEcho } from './remoteCommands.js';

export type RemoteIpcHandler = (...args: unknown[]) => Promise<unknown> | unknown;

export interface RemoteControlUiDeps {
  service: {
    status(): RemoteControlStatus;
    onStatus(callback: (status: RemoteControlStatus) => void): () => void;
    setEnabled(enabled: boolean): Promise<void>;
    listDevices(): RegisteredDevice[];
    revokeDevice(deviceId: string): Promise<void>;
  };
  settings: { getRemoteControlEnabled(): boolean };
  policy: Pick<RemotePolicy, 'allows' | 'enabledSessions' | 'find' | 'setSessionEnabled' | 'getAtStartup' | 'setAtStartup' | 'onChange' | 'notifySessionsChanged'>;
  gateway: { attachedViewers(sessionId?: string): number; onViewersChanged(listener: () => void): () => void };
  /** The session's project, if the session is open in this app. */
  runtimeRef(sessionId: string): SessionRef | undefined;
  signedIn(): boolean;
  /** Account availability can change while the remote service remains off. */
  onAccountChanged(callback: () => void): () => void;
  /** Deliver to every window of the app. */
  send(channel: string, payload: unknown): void;
}

const MAX_ID_CHARS = 128;

function boolean(value: unknown, what: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`invalid ${what}`);
  return value;
}

function id(value: unknown, what: string): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_ID_CHARS) throw new Error(`invalid ${what}`);
  return value;
}

export class RemoteControlUi {
  private readonly offs: Array<() => void> = [];
  private lastStatus = '';
  private lastSessions = '';

  constructor(private readonly deps: RemoteControlUiDeps) {
    this.offs.push(
      deps.onAccountChanged(() => this.pushStatus()),
      deps.service.onStatus(() => this.pushStatus()),
      deps.policy.onChange(() => { this.pushStatus(); this.pushSessions(); }),
      deps.gateway.onViewersChanged(() => this.pushSessions()),
    );
  }

  /** The invoke handlers by channel. */
  handlers(): Record<string, RemoteIpcHandler> {
    return {
      [CH_REMOTE_STATUS_GET]: () => this.status(),
      [CH_REMOTE_ENABLED_SET]: (enabled) => this.setEnabled(boolean(enabled, 'Remote Control setting')),
      [CH_REMOTE_AT_STARTUP_GET]: () => this.deps.policy.getAtStartup(),
      [CH_REMOTE_AT_STARTUP_SET]: (enabled) => {
        this.deps.policy.setAtStartup(boolean(enabled, 'Remote Control setting'));
        return this.deps.policy.getAtStartup();
      },
      [CH_REMOTE_DEVICES_LIST]: () => this.devices(),
      [CH_REMOTE_DEVICE_REVOKE]: async (deviceId) => {
        await this.deps.service.revokeDevice(id(deviceId, 'device id'));
        this.pushStatus();
        return this.devices();
      },
      [CH_REMOTE_SESSIONS_GET]: () => this.sessions(),
      [CH_REMOTE_SESSION_SET]: (sessionId, enabled) => this.setSession(id(sessionId, 'session id'), boolean(enabled, 'Remote Control setting')),
    };
  }

  status(): RemoteControlUiStatus {
    const status = this.deps.service.status();
    const result: RemoteControlUiStatus = {
      enabled: this.deps.settings.getRemoteControlEnabled(),
      atStartup: this.deps.policy.getAtStartup(),
      managedDisabled: !this.deps.policy.allows(),
      signedIn: this.deps.signedIn(),
      state: status.state,
      connections: status.connections.map((connection) => ({
        connId: connection.conn_id,
        deviceId: connection.device_id,
        deviceName: connection.device_name,
        platform: connection.platform,
        transport: connection.transport,
        approved: connection.approved,
        ...(connection.candidate_type !== undefined ? { candidateType: connection.candidate_type } : {}),
      })),
    };
    if (status.error !== undefined) result.error = status.error;
    return result;
  }

  devices(): RemoteDeviceUi[] {
    return this.deps.service.listDevices().map((device) => ({
      deviceId: device.device_id,
      name: device.name,
      platform: device.platform,
      approvedAt: device.approved_at,
      lastSeen: device.last_seen,
    }));
  }

  sessions(): RemoteSessionsUi {
    return {
      sessions: this.deps.policy.enabledSessions().map((ref) => ({
        sessionId: ref.sessionId,
        viewers: this.deps.gateway.attachedViewers(ref.sessionId),
      })),
    };
  }

  async setEnabled(enabled: boolean): Promise<RemoteControlUiStatus> {
    if (enabled && !this.deps.policy.allows()) throw new Error('Remote Control is turned off by your administrator.');
    await this.deps.service.setEnabled(enabled);
    this.pushStatus();
    return this.status();
  }

  async setSession(sessionId: string, enabled: boolean): Promise<RemoteSessionsUi> {
    const known = this.deps.policy.find(sessionId);
    if (enabled) {
      if (!this.deps.policy.allows()) throw new Error('Remote Control is turned off by your administrator.');
      const ref = this.deps.runtimeRef(sessionId) ?? known;
      if (ref === undefined) throw new Error('unknown session');
      // The first session someone shares is also the moment Remote Control is switched on for this computer.
      if (!this.deps.settings.getRemoteControlEnabled()) await this.deps.service.setEnabled(true);
      this.deps.policy.setSessionEnabled(ref, true);
    } else if (known !== undefined) {
      this.deps.policy.setSessionEnabled(known, false);
    }
    this.pushStatus();
    return this.sessions();
  }

  /** A session was archived or restored: viewers stop or start seeing it. */
  sessionsChanged(): void {
    this.deps.policy.notifySessionsChanged();
  }

  /** A viewer's prompt was sent; show it in the window. */
  echo(echo: RemotePromptEcho): void {
    const payload: RemotePromptEchoUi = { ...echo };
    this.deps.send(CH_REMOTE_PROMPT_ECHO, payload);
  }

  private pushStatus(): void {
    const status = this.status();
    const serialized = JSON.stringify(status);
    if (serialized === this.lastStatus) return;
    this.lastStatus = serialized;
    this.deps.send(CH_REMOTE_STATUS_EVENT, status);
  }

  private pushSessions(): void {
    const sessions = this.sessions();
    const serialized = JSON.stringify(sessions);
    if (serialized === this.lastSessions) return;
    this.lastSessions = serialized;
    this.deps.send(CH_REMOTE_SESSIONS_EVENT, sessions);
  }

  dispose(): void {
    for (const off of this.offs.splice(0)) off();
  }
}
